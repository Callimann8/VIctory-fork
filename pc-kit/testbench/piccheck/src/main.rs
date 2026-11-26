//! piccheck: compares the frames a test run saved with the frames a
//! reference run (the main build) saved, and flags any visual difference.
//!
//! Frames come from the emulator's own capture (`VICTORY_CAPTURE` with
//! `VICTORY_CAPTURE_WORLD`: `frame-<n>.png`, half size) or from the test
//! runner's window capture (`win-<seconds>.png`). A folder may hold
//! `world.txt` with `first=<frame>`: the first world frame, so frame numbers
//! are counted from where the game world starts (menus and loading take a
//! different number of frames each run).
//!
//! Two runs never show exactly the same picture at the same frame (people
//! move, ray-traced light is noisy, timing differs), so every test picture
//! is compared with the most similar reference picture near the same point,
//! and the verdict looks for *systematic* changes over the whole run:
//! less fine detail (lower resolution, blurrier textures), a light or
//! colour shift (missing bounce light, shadows), and local damage (black
//! patches, missing objects, broken tiles).
//!
//!   piccheck compare --ref <dir> --test <dir> --out <dir> [--noise <noise.txt>] [--window <n>]
//!   piccheck calibrate --ref <dir> --test <dir> --out <noise.txt>
//!
//! `calibrate` takes two runs of the *same* build and writes how far apart
//! they are by chance; `compare --noise` then widens its limits to that.
//! Exit code of `compare`: 0 PASS, 1 CHECK, 2 FAIL, 3 nothing to compare.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- images

/// A picture as linear arrays of 8-bit R, G, B.
#[derive(Clone)]
struct Pic {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Pic {
    fn load(path: &Path) -> Result<Pic, String> {
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        let (w, h) = (info.width as usize, info.height as usize);
        let ch = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return Err(format!("{}: indexed colour not expanded", path.display())),
        };
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = &buf[y * info.line_size..][..w * ch];
            for x in 0..w {
                let p = &row[x * ch..][..ch];
                if ch < 3 {
                    rgb.extend([p[0], p[0], p[0]]);
                } else {
                    rgb.extend_from_slice(&p[..3]);
                }
            }
        }
        Ok(Pic { w, h, rgb })
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&self.rgb).map_err(|e| e.to_string())
    }

    /// Box-filtered to `w` by `h`.
    fn resized(&self, w: usize, h: usize) -> Pic {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            let (y0, y1) = (y * self.h / h, ((y + 1) * self.h / h).max(y * self.h / h + 1).min(self.h));
            for x in 0..w {
                let (x0, x1) = (x * self.w / w, ((x + 1) * self.w / w).max(x * self.w / w + 1).min(self.w));
                let mut sum = [0u32; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        for c in 0..3 {
                            sum[c] += u32::from(self.rgb[(sy * self.w + sx) * 3 + c]);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                for c in 0..3 {
                    rgb[(y * w + x) * 3 + c] = (sum[c] / n) as u8;
                }
            }
        }
        Pic { w, h, rgb }
    }

    /// Luma (BT.709), 0-255.
    fn luma(&self) -> Gray {
        let v = self
            .rgb
            .chunks_exact(3)
            .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
            .collect();
        Gray { w: self.w, h: self.h, v }
    }
}

struct Gray {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Gray {
    fn half(&self) -> Gray {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut v = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let at = |dx: usize, dy: usize| self.v[((2 * y + dy).min(self.h - 1)) * self.w + (2 * x + dx).min(self.w - 1)];
                v[y * w + x] = 0.25 * (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1));
            }
        }
        Gray { w, h, v }
    }
}

// ---------------------------------------------------------------- metrics

/// SSIM over 8x8 blocks of the rectangle `[x0,x1) x [y0,y1)`: the mean.
fn ssim_rect(a: &Gray, b: &Gray, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let (mut total, mut n) = (0.0, 0u32);
    let mut by = y0;
    while by + 8 <= y1 {
        let mut bx = x0;
        while bx + 8 <= x1 {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
            for y in by..by + 8 {
                for x in bx..bx + 8 {
                    let (p, q) = (f64::from(a.v[y * a.w + x]), f64::from(b.v[y * b.w + x]));
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let m = 64.0;
            let (ma, mb) = (sa / m, sb / m);
            let (va, vb, cov) = (saa / m - ma * ma, sbb / m - mb * mb, sab / m - ma * mb);
            total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            n += 1;
            bx += 8;
        }
        by += 8;
    }
    if n == 0 { 1.0 } else { total / f64::from(n) }
}

fn ssim(a: &Gray, b: &Gray) -> f64 {
    ssim_rect(a, b, 0, 0, a.w, a.h)
}

/// Mean absolute Laplacian over mean luma: how much fine detail there is,
/// apart from how bright the picture is (a darker picture alone does not
/// count as a blurrier one). Pixels where
/// either picture is almost black are left out (dark areas have no detail
/// to lose, and noise there would swing the ratio).
fn detail(g: &Gray, mask: &Gray) -> f64 {
    let (mut sum, mut lum, mut n) = (0.0f64, 0.0f64, 0u64);
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            if mask.v[y * g.w + x] < 12.0 || g.v[y * g.w + x] < 12.0 {
                continue;
            }
            let c = g.v[y * g.w + x];
            let l = 4.0 * c - g.v[y * g.w + x - 1] - g.v[y * g.w + x + 1] - g.v[(y - 1) * g.w + x] - g.v[(y + 1) * g.w + x];
            sum += f64::from(l.abs());
            lum += f64::from(c);
            n += 1;
        }
    }
    if n == 0 || lum <= 0.0 { 0.0 } else { sum / lum }
}

const TILES_X: usize = 8;
const TILES_Y: usize = 6;

#[derive(Clone, Default)]
struct Pair {
    test: String,
    reference: String,
    /// Frame (or second) of each, counted from the world's start.
    at_test: i64,
    at_ref: i64,
    ssim: f64,
    worst_tile: f64,
    worst_tile_at: (usize, usize),
    /// Tiles whose SSIM is far below the picture's own.
    bad_tiles: usize,
    /// Fine detail, test over reference (1 = same, below 1 = blurrier).
    detail: f64,
    /// Mean luma, test against reference, in percent (-5 = 5% darker).
    light: f64,
    /// Mean of R, G, B, test against reference, in percent.
    colour: [f64; 3],
    /// The largest change of a tile's mean luma (0-255, either way).
    tile_light: f64,
    /// Almost-black share of the picture, test minus reference (0-1).
    black: f64,
    size_differs: bool,
    verdict: Verdict,
    why: Vec<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Verdict {
    #[default]
    Pass,
    Check,
    Fail,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Check => "CHECK",
            Verdict::Fail => "FAIL",
        }
    }
}

/// How far apart two runs of the same build may be by chance.
#[derive(Clone, Copy)]
struct Limits {
    /// Run median of `detail` below 1 - this: CHECK; below 1 - 2x: FAIL.
    detail: f64,
    /// Run median of `|light|` (percent) above this: CHECK; above 2x: FAIL.
    light: f64,
    /// Run median of `|colour|` (percent, worst channel) above this: CHECK; 2x: FAIL.
    colour: f64,
    /// A tile is bad when its SSIM is below the picture's by more than this,
    tile_drop: f64,
    /// or its mean luma moved by more than this (0-255): a lamp or a
    /// shadow that is missing, a black or white patch.
    tile_light: f64,
    /// Run median of `black` above this: CHECK; 2x: FAIL.
    black: f64,
    /// Pictures with bad tiles: share of pairs above this: CHECK; 2x: FAIL.
    damaged_share: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { detail: 0.03, light: 2.0, colour: 2.5, tile_drop: 0.25, tile_light: 6.0, black: 0.003, damaged_share: 0.15 }
    }
}

impl Limits {
    fn read(path: &Path) -> Result<Limits, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut l = Limits::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let Ok(v) = v.trim().parse::<f64>() else { continue };
            // Calibrated limits replace the defaults, but never go below a
            // floor (a few pictures of two quiet runs can look closer than
            // runs usually are).
            match k.trim() {
                "detail" => l.detail = v.max(0.015),
                "light" => l.light = v.max(1.0),
                "colour" => l.colour = v.max(1.25),
                "tile_drop" => l.tile_drop = v.max(0.12),
                "tile_light" => l.tile_light = v.max(3.0),
                "black" => l.black = v.max(0.0015),
                "damaged_share" => l.damaged_share = v.max(0.1),
                _ => {}
            }
        }
        Ok(l)
    }
}

fn measure(t: &Pic, r: &Pic, limits: &Limits) -> Pair {
    let size_differs = (t.w, t.h) != (r.w, r.h);
    let t = t.resized(r.w, r.h);
    let (tl, rl) = (t.luma(), r.luma());
    let ssim_all = ssim(&tl, &rl);
    let mut worst = (f64::MAX, (0, 0));
    let mut bad = 0;
    let mut tile_light = 0.0f64;
    for ty in 0..TILES_Y {
        for tx in 0..TILES_X {
            let (x0, x1) = (tx * r.w / TILES_X, (tx + 1) * r.w / TILES_X);
            let (y0, y1) = (ty * r.h / TILES_Y, (ty + 1) * r.h / TILES_Y);
            let s = ssim_rect(&tl, &rl, x0, y0, x1, y1);
            let tile_mean = |g: &Gray| {
                let mut sum = 0.0f64;
                for y in y0..y1 {
                    sum += g.v[y * g.w + x0..y * g.w + x1].iter().map(|&x| f64::from(x)).sum::<f64>();
                }
                sum / ((y1 - y0) * (x1 - x0)).max(1) as f64
            };
            let moved = (tile_mean(&tl) - tile_mean(&rl)).abs();
            tile_light = tile_light.max(moved);
            if s < ssim_all - limits.tile_drop || moved > limits.tile_light {
                bad += 1;
                if s >= worst.0 {
                    // A tile flagged for its light: point at it all the same.
                    worst = (s.min(worst.0), (tx, ty));
                }
            }
            if s < worst.0 {
                worst = (s, (tx, ty));
            }
        }
    }
    let (dt, dr) = (detail(&tl, &rl), detail(&rl, &tl));
    let mean = |g: &Gray| g.v.iter().map(|&x| f64::from(x)).sum::<f64>() / g.v.len() as f64;
    let black = |g: &Gray| g.v.iter().filter(|&&x| x < 6.0).count() as f64 / g.v.len() as f64;
    let rel = |a: f64, b: f64| 100.0 * (a - b) / b.max(8.0);
    let mut colour = [0.0; 3];
    for c in 0..3 {
        let sum = |p: &Pic| p.rgb.iter().skip(c).step_by(3).map(|&x| f64::from(x)).sum::<f64>() / (p.w * p.h) as f64;
        colour[c] = rel(sum(&t), sum(r));
    }
    Pair {
        ssim: ssim_all,
        worst_tile: worst.0,
        worst_tile_at: worst.1,
        bad_tiles: bad,
        detail: if dr > 0.0 { dt / dr } else { 1.0 },
        light: rel(mean(&tl), mean(&rl)),
        colour,
        tile_light,
        black: black(&tl) - black(&rl),
        size_differs,
        ..Pair::default()
    }
}

/// A heat map of where `t` and `r` differ: the reference in grey, the
/// difference in red on top.
fn diff_map(t: &Pic, r: &Pic) -> Pic {
    let t = t.resized(r.w, r.h);
    let mut rgb = Vec::with_capacity(r.rgb.len());
    for (a, b) in t.rgb.chunks_exact(3).zip(r.rgb.chunks_exact(3)) {
        let d = (0..3).map(|c| (i32::from(a[c]) - i32::from(b[c])).abs()).max().unwrap_or(0);
        let g = ((u32::from(b[0]) + u32::from(b[1]) + u32::from(b[2])) / 9) as u8;
        let red = (i32::from(g) + d * 4).min(255) as u8;
        rgb.extend([red, g, g]);
    }
    Pic { w: r.w, h: r.h, rgb }
}

// ---------------------------------------------------------------- runs

struct Frame {
    path: PathBuf,
    name: String,
    /// Counted from the world's start (frames, or seconds for window captures).
    at: i64,
}

fn frames(dir: &Path) -> Result<Vec<Frame>, String> {
    let first = fs::read_to_string(dir.join("world.txt"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("first=").and_then(|v| v.trim().parse::<i64>().ok())));
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".png") else { continue };
        if let Some(n) = stem.strip_prefix("frame-").and_then(|n| n.parse::<i64>().ok()) {
            // Without the world's start every frame counts as it is; with
            // it, only frames inside the world (menus and loading screens
            // differ from run to run by design).
            match first {
                Some(f) if n > f => out.push(Frame { path: e.path(), name, at: n - f }),
                Some(_) => {}
                None => out.push(Frame { path: e.path(), name, at: n }),
            }
        } else if let Some(s) = stem.strip_prefix("win-").and_then(|n| n.parse::<i64>().ok()) {
            out.push(Frame { path: e.path(), name, at: s });
        }
    }
    out.sort_by_key(|f| f.at);
    Ok(out)
}

/// Every test frame with its most similar reference frame within `window`.
fn pair_up(test: &[Frame], refs: &[Frame], window: i64, limits: &Limits, out: Option<&Path>) -> Result<Vec<Pair>, String> {
    // Reference pictures, loaded once, with a small luma copy for matching.
    let mut cache: Vec<Option<(Pic, Gray)>> = (0..refs.len()).map(|_| None).collect();
    let mut pairs = Vec::new();
    for (ti, tf) in test.iter().enumerate() {
        let tp = match Pic::load(&tf.path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipped: {e}");
                continue;
            }
        };
        let mut best: Option<(f64, usize)> = None;
        for (ri, rf) in refs.iter().enumerate() {
            if (rf.at - tf.at).abs() > window {
                continue;
            }
            if cache[ri].is_none() {
                match Pic::load(&rf.path) {
                    Ok(p) => {
                        let small = p.luma().half().half();
                        cache[ri] = Some((p, small));
                    }
                    Err(e) => {
                        eprintln!("skipped: {e}");
                        continue;
                    }
                }
            }
            let (rp, rs) = cache[ri].as_ref().unwrap();
            let ts = tp.resized(rp.w, rp.h).luma().half().half();
            let s = ssim(&ts, rs);
            if best.is_none_or(|b| s > b.0) {
                best = Some((s, ri));
            }
        }
        let Some((_, ri)) = best else { continue };
        let (rp, _) = cache[ri].as_ref().unwrap();
        let mut p = measure(&tp, rp, limits);
        p.test = tf.name.clone();
        p.reference = refs[ri].name.clone();
        p.at_test = tf.at;
        p.at_ref = refs[ri].at;
        // Per picture: what stands out in this pair alone.
        if p.size_differs {
            p.why.push("picture size differs from the main build".into());
            p.verdict = Verdict::Fail;
        }
        if p.detail < 1.0 - 2.0 * limits.detail {
            p.why.push(format!("detail {:.2}", p.detail));
            p.verdict = p.verdict.max(Verdict::Check);
        }
        if p.light.abs() > 2.0 * limits.light {
            p.why.push(format!("light {:+.1}%", p.light));
            p.verdict = p.verdict.max(Verdict::Check);
        }
        if p.bad_tiles > 0 && p.ssim > 0.5 {
            p.why.push(format!("{} tile(s) differ (at column {} row {}, largest light change {:.0}/255)", p.bad_tiles, p.worst_tile_at.0 + 1, p.worst_tile_at.1 + 1, p.tile_light));
            p.verdict = p.verdict.max(Verdict::Check);
        }
        if p.black > 2.0 * limits.black {
            p.why.push(format!("black area {:+.1}%", p.black * 100.0));
            p.verdict = p.verdict.max(Verdict::Check);
        }
        if let Some(out) = out {
            let pics = out.join("pics");
            let _ = fs::create_dir_all(&pics);
            let tag = format!("{ti:03}");
            let small = |p: &Pic| p.resized((p.w / 2).max(1), (p.h / 2).max(1));
            small(&tp).save(&pics.join(format!("{tag}-test.png")))?;
            small(rp).save(&pics.join(format!("{tag}-ref.png")))?;
            small(&diff_map(&tp, rp)).save(&pics.join(format!("{tag}-diff.png")))?;
        }
        pairs.push(p);
    }
    Ok(pairs)
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

fn percentile(mut v: Vec<f64>, q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * q).round() as usize]
}

struct RunVerdict {
    verdict: Verdict,
    why: Vec<String>,
    detail: f64,
    light: f64,
    colour: f64,
    black: f64,
    damaged: f64,
    ssim: f64,
}

fn judge(pairs: &[Pair], limits: &Limits) -> RunVerdict {
    let detail = median(pairs.iter().map(|p| p.detail).collect());
    let light = median(pairs.iter().map(|p| p.light).collect());
    let colour = (0..3).map(|c| median(pairs.iter().map(|p| p.colour[c]).collect())).fold(0.0f64, |m, x| if x.abs() > m.abs() { x } else { m });
    let black = median(pairs.iter().map(|p| p.black).collect());
    let damaged = pairs.iter().filter(|p| p.bad_tiles > 0 && p.ssim > 0.5).count() as f64 / pairs.len().max(1) as f64;
    let ssim = median(pairs.iter().map(|p| p.ssim).collect());
    let mut v = RunVerdict { verdict: Verdict::Pass, why: Vec::new(), detail, light, colour, black, damaged, ssim };
    let mut rule = |value: f64, limit: f64, what: String| {
        if value > 2.0 * limit {
            v.verdict = v.verdict.max(Verdict::Fail);
            v.why.push(what);
        } else if value > limit {
            v.verdict = v.verdict.max(Verdict::Check);
            v.why.push(what);
        }
    };
    rule(1.0 - detail, limits.detail, format!("less fine detail than main (ratio {detail:.3}): lower resolution or blurrier textures?"));
    rule(-(1.0 - detail), limits.detail, format!("more fine detail/noise than main (ratio {detail:.3}): sharpening or noisier light?"));
    rule(light.abs(), limits.light, format!("overall light differs from main ({light:+.1}%)"));
    rule(colour.abs(), limits.colour, format!("colour differs from main (worst channel {colour:+.1}%)"));
    rule(black, limits.black, format!("more almost-black area than main ({:+.1}% of the picture)", black * 100.0));
    rule(damaged, limits.damaged_share, format!("{:.0}% of pictures have a region that differs from main (see the red areas)", damaged * 100.0));
    if pairs.iter().any(|p| p.size_differs) {
        v.verdict = Verdict::Fail;
        v.why.push("picture size differs from main".into());
    }
    v
}

// ---------------------------------------------------------------- report

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn write_report(out: &Path, title: &str, pairs: &[Pair], v: &RunVerdict, limits: &Limits) -> Result<(), String> {
    let mut h = String::new();
    let colour = |x: Verdict| match x {
        Verdict::Pass => "#2e7d32",
        Verdict::Check => "#b26a00",
        Verdict::Fail => "#c62828",
    };
    let _ = write!(
        h,
        "<!doctype html><meta charset=utf-8><title>Picture check</title><style>body{{font:14px system-ui;margin:16px;background:#111;color:#ddd}}\
         table{{border-collapse:collapse}}td,th{{padding:4px 8px;border-bottom:1px solid #333;vertical-align:top}}img{{width:320px;display:block}}\
         .v{{font-weight:700}}</style><h1>Picture check: {}</h1><p class=v style='color:{}'>{}</p><ul>",
        esc(title),
        colour(v.verdict),
        v.verdict.word()
    );
    for w in &v.why {
        let _ = write!(h, "<li>{}</li>", esc(w));
    }
    let _ = write!(
        h,
        "</ul><p>{} pictures compared. Medians: detail ratio {:.3}, light {:+.1}%, colour {:+.1}%, black area {:+.2}%, similarity {:.3}. \
         Limits: detail &plusmn;{:.3}, light {:.1}%, colour {:.1}%, tile {:.2} / {:.0}, black {:.2}%, damaged share {:.0}% (CHECK above, FAIL above twice).</p>",
        pairs.len(),
        v.detail,
        v.light,
        v.colour,
        v.black * 100.0,
        v.ssim,
        limits.detail,
        limits.light,
        limits.colour,
        limits.tile_drop,
        limits.tile_light,
        limits.black * 100.0,
        limits.damaged_share * 100.0
    );
    h.push_str("<table><tr><th>#</th><th>main</th><th>test</th><th>difference</th><th>numbers</th></tr>");
    // Flagged pairs first, then the rest.
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(pairs[i].verdict), pairs[i].at_test));
    for i in order {
        let p = &pairs[i];
        let _ = write!(
            h,
            "<tr><td>{i}<br><span class=v style='color:{}'>{}</span></td><td><img src='pics/{i:03}-ref.png'>{} (at {})</td>\
             <td><img src='pics/{i:03}-test.png'>{} (at {})</td><td><img src='pics/{i:03}-diff.png'></td>\
             <td>similarity {:.3}<br>worst tile {:.3}<br>detail {:.3}<br>light {:+.1}%<br>tile light {:.0}<br>black {:+.2}%<br>{}</td></tr>",
            colour(p.verdict),
            p.verdict.word(),
            esc(&p.reference),
            p.at_ref,
            esc(&p.test),
            p.at_test,
            p.ssim,
            p.worst_tile,
            p.detail,
            p.light,
            p.tile_light,
            p.black * 100.0,
            esc(&p.why.join("; "))
        );
    }
    h.push_str("</table>");
    fs::write(out.join("report.html"), h).map_err(|e| e.to_string())?;

    let mut csv = String::from("index,verdict,ref,test,at_ref,at_test,ssim,worst_tile,bad_tiles,tile_light,detail,light_pct,r_pct,g_pct,b_pct,black,why\n");
    for (i, p) in pairs.iter().enumerate() {
        let _ = writeln!(
            csv,
            "{i},{},{},{},{},{},{:.4},{:.4},{},{:.1},{:.4},{:.2},{:.2},{:.2},{:.2},{:.4},\"{}\"",
            p.verdict.word(),
            p.reference,
            p.test,
            p.at_ref,
            p.at_test,
            p.ssim,
            p.worst_tile,
            p.bad_tiles,
            p.tile_light,
            p.detail,
            p.light,
            p.colour[0],
            p.colour[1],
            p.colour[2],
            p.black,
            p.why.join("; ").replace('"', "'")
        );
    }
    fs::write(out.join("pairs.csv"), csv).map_err(|e| e.to_string())?;
    let line = format!(
        "{} pictures={} detail={:.3} light={:+.1}% colour={:+.1}% black={:+.2}% damaged={:.0}% ssim={:.3}{}{}\n",
        v.verdict.word(),
        pairs.len(),
        v.detail,
        v.light,
        v.colour,
        v.black * 100.0,
        v.damaged * 100.0,
        v.ssim,
        if v.why.is_empty() { "" } else { " | " },
        v.why.join(" | ")
    );
    fs::write(out.join("summary.txt"), &line).map_err(|e| e.to_string())?;
    print!("{line}");
    Ok(())
}

// ---------------------------------------------------------------- main

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn run() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: piccheck compare --ref <dir> --test <dir> --out <dir> [--noise <file>] [--window <n>] [--title <text>]\n       piccheck calibrate --ref <dir> --test <dir> --out <file>";
    let cmd = args.get(1).ok_or(usage)?.as_str();
    let ref_dir = PathBuf::from(arg(&args, "--ref").ok_or(usage)?);
    let test_dir = PathBuf::from(arg(&args, "--test").ok_or(usage)?);
    let out = PathBuf::from(arg(&args, "--out").ok_or(usage)?);
    let refs = frames(&ref_dir)?;
    let tests = frames(&test_dir)?;
    // Window captures are named by second, the emulator's by frame.
    let seconds = tests.first().is_some_and(|f| f.name.starts_with("win-"));
    let window = arg(&args, "--window").and_then(|w| w.parse().ok()).unwrap_or(if seconds { 8 } else { 48 });
    match cmd {
        "compare" => {
            let limits = match arg(&args, "--noise") {
                Some(n) if Path::new(&n).exists() => Limits::read(Path::new(&n))?,
                _ => Limits::default(),
            };
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let pairs = pair_up(&tests, &refs, window, &limits, Some(&out))?;
            if pairs.is_empty() {
                let line = format!("NONE no test picture had a main picture within {window} of it ({} test, {} main)\n", tests.len(), refs.len());
                fs::write(out.join("summary.txt"), &line).map_err(|e| e.to_string())?;
                print!("{line}");
                return Ok(3);
            }
            let v = judge(&pairs, &limits);
            let title = arg(&args, "--title").unwrap_or_else(|| test_dir.display().to_string());
            write_report(&out, &title, &pairs, &v, &limits)?;
            Ok(v.verdict as i32)
        }
        "calibrate" => {
            let base = Limits::default();
            let pairs = pair_up(&tests, &refs, window, &base, None)?;
            if pairs.is_empty() {
                return Err("nothing to calibrate with: no pictures near each other".into());
            }
            // The spread of the same build against itself, with a margin:
            // the 90th percentile of each per-picture number, halved where
            // the verdict uses run medians (those vary less than one picture).
            let detail = percentile(pairs.iter().map(|p| (1.0 - p.detail).abs()).collect(), 0.9) * 0.75;
            let light = percentile(pairs.iter().map(|p| p.light.abs()).collect(), 0.9) * 0.75;
            let colour = percentile(pairs.iter().map(|p| p.colour.iter().fold(0.0f64, |m, &x| m.max(x.abs()))).collect(), 0.9) * 0.75;
            let black = percentile(pairs.iter().map(|p| p.black.max(0.0)).collect(), 0.9) * 0.75;
            let drops = percentile(pairs.iter().map(|p| p.ssim - p.worst_tile).collect(), 0.9) + 0.05;
            let tile_light = percentile(pairs.iter().map(|p| p.tile_light).collect(), 0.9) * 1.25 + 1.0;
            let v = judge(&pairs, &base);
            let text = format!(
                "# piccheck noise: {} vs {}, {} pairs (same build twice)\n# medians: detail {:.3} light {:+.2}% colour {:+.2}% black {:+.3}% damaged {:.0}%\n\
                 detail={detail:.4}\nlight={light:.3}\ncolour={colour:.3}\ntile_drop={drops:.3}\ntile_light={tile_light:.2}\nblack={black:.5}\ndamaged_share={:.3}\n",
                ref_dir.display(),
                test_dir.display(),
                pairs.len(),
                v.detail,
                v.light,
                v.colour,
                v.black * 100.0,
                v.damaged * 100.0,
                (v.damaged * 1.5).max(base.damaged_share)
            );
            fs::write(&out, &text).map_err(|e| e.to_string())?;
            print!("{text}");
            Ok(0)
        }
        _ => Err(usage.into()),
    }
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("piccheck: {e}");
            std::process::exit(4);
        }
    }
}
