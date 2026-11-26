//! The CPU's waits on the card's results, one line with the frame line
//! ("card waits: ..."): for every wait of the preparing thread on flights
//! in the air (a settle, or the relay's wait for landings), what made it
//! wait (which read of ours or of the title, which memory range, which
//! range a flight may write there), how long, how old the oldest flight
//! was, and how often the same 64 KiB page made it wait within a second.
//! The data for running the CPU and the card side by side: which results
//! to copy back early, which reads need not block.
//!
//! Always on and cheap: a thread-local note per trigger, a lock per wait
//! (about two waits a frame). `VICTORY_WAIT_MAP=1` also notes, per 64 KiB
//! page, the pipeline whose dispatch or draw last declared a GPU write
//! there (one lock per declaration), so the line can name the producer;
//! `VICTORY_WAIT_LOG=1` logs every wait on its own line (at most 50 a
//! second).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

const PAGE: usize = 1 << 16;

/// What made a wait, noted by the code that found it must wait.
#[derive(Clone, Copy)]
pub(crate) struct Trigger {
    pub kind: &'static str,
    /// The memory read (0, 0: none known).
    pub at: usize,
    pub len: usize,
    /// The range flights may write that it touches (0, 0: none known).
    pub written: (usize, usize),
}

thread_local! {
    static TRIGGER: Cell<Option<Trigger>> = const { Cell::new(None) };
}

/// Notes what will make the next wait on this thread.
pub(crate) fn trigger(kind: &'static str, at: usize, len: usize, written: (usize, usize)) {
    TRIGGER.set(Some(Trigger { kind, at, len, written }));
}

/// Forgets a trigger that led to no wait.
pub(crate) fn clear() {
    TRIGGER.set(None);
}

/// Notes a trigger with no range (presents, fences, caps), unless one is
/// noted already.
pub(crate) fn trigger_plain(kind: &'static str) {
    if TRIGGER.get().is_none() {
        TRIGGER.set(Some(Trigger { kind, at: 0, len: 0, written: (0, 0) }));
    }
}

/// The first range of `written` (sorted or not) that touches `[at, at + len)`.
pub(crate) fn touching(written: &[(usize, usize)], at: usize, len: usize) -> (usize, usize) {
    let end = at.saturating_add(len.max(1));
    written.iter().copied().find(|&(a, b)| a < end && at < b).unwrap_or((0, 0))
}

/// When the line of flights last went from empty to holding one (ms).
static FIRST_FLIGHT: AtomicU64 = AtomicU64::new(0);

/// A flight joins the line; `was_empty`: it was the first in the air.
pub(crate) fn flight_sent(was_empty: bool) {
    if was_empty {
        FIRST_FLIGHT.store(crate::host::now_ms().max(1), Relaxed);
    }
}

struct Range {
    kind: &'static str,
    why: &'static str,
    at: usize,
    len: usize,
    n: u64,
    us: u64,
    ages: Vec<u32>,
    repeats: u64,
    writer: Option<std::sync::Arc<str>>,
}

#[derive(Default)]
struct Stats {
    /// By trigger kind: waits, microseconds.
    kinds: HashMap<&'static str, (u64, u64)>,
    /// By the read's first page: the waits there.
    ranges: HashMap<(usize, &'static str), Range>,
    /// Per page: when it last made a wait (ms), for the repeats.
    last: HashMap<usize, u64>,
    waits: u64,
    us: u64,
}

static STATS: Mutex<Option<Stats>> = Mutex::new(None);

/// With `VICTORY_WAIT_MAP=1`: per 64 KiB page, the pipeline (title
/// address) whose work last declared a write there, and when (ms).
static WRITERS: Mutex<Option<HashMap<usize, (std::sync::Arc<str>, u64)>>> = Mutex::new(None);

thread_local! {
    /// The pipeline bound on this thread's list now, and its label.
    static BOUND: std::cell::RefCell<(usize, Option<std::sync::Arc<str>>)> = const { std::cell::RefCell::new((0, None)) };
}

/// The title's pipeline `pipeline` is bound for the work recorded next on
/// this thread (with `VICTORY_WAIT_MAP=1`; nothing otherwise).
pub(crate) fn bound(pipeline: usize) {
    if map_on() {
        BOUND.with(|b| {
            let mut b = b.borrow_mut();
            if b.0 != pipeline {
                *b = (pipeline, None);
            }
        });
    }
}

fn map_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WAIT_MAP").is_some_and(|v| v == "1"))
}

fn log_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WAIT_LOG").is_some_and(|v| v == "1"))
}

/// Work of the pipeline bound on this thread ([`bound`]) declared that its
/// shaders may write `[at, at + len)` (with `VICTORY_WAIT_MAP=1`; nothing
/// otherwise). Ranges over 64 MiB note their first and last page only.
pub(crate) fn declared(at: usize, len: usize) {
    if !map_on() || len == 0 {
        return;
    }
    let Some(pipeline) = BOUND.with(|b| {
        let mut b = b.borrow_mut();
        if b.0 == 0 {
            return None;
        }
        if b.1.is_none() {
            b.1 = Some(crate::pipeline::label(b.0 as crate::object::This).into());
        }
        b.1.clone()
    }) else {
        return;
    };
    let now = crate::host::now_ms();
    let (first, last) = (at / PAGE, (at + len - 1) / PAGE);
    let mut w = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
    let w = w.get_or_insert_with(HashMap::new);
    if w.len() > 1 << 20 {
        w.clear();
    }
    if last - first > 1024 {
        w.insert(first, (pipeline.clone(), now));
        w.insert(last, (pipeline, now));
    } else {
        for page in first..=last {
            w.insert(page, (pipeline.clone(), now));
        }
    }
}

/// The preparing thread waited `us` for flights, for reason `why`.
pub(crate) fn waited(why: &'static str, us: u64) {
    let t = TRIGGER.take().unwrap_or(Trigger { kind: why, at: 0, len: 0, written: (0, 0) });
    let now = crate::host::now_ms();
    let first = FIRST_FLIGHT.load(Relaxed);
    let age = if first == 0 { 0 } else { now.saturating_sub(first) } as u32;
    let page = t.at / PAGE;
    let writer = if map_on() && t.len > 0 {
        let w = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        w.as_ref().and_then(|w| w.get(&page).or_else(|| w.get(&(t.written.0 / PAGE)))).map(|(p, _)| p.clone())
    } else {
        None
    };
    let mut held = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let s = held.get_or_insert_with(Stats::default);
    s.waits += 1;
    s.us += us;
    let k = s.kinds.entry(t.kind).or_insert((0, 0));
    k.0 += 1;
    k.1 += us;
    let repeat = t.len > 0 && s.last.get(&page).is_some_and(|&at| now.saturating_sub(at) < 1000);
    if t.len > 0 {
        s.last.insert(page, now);
        if s.last.len() > 1 << 16 {
            s.last.retain(|_, &mut at| now.saturating_sub(at) < 1000);
        }
    }
    let r = s.ranges.entry((page, t.kind)).or_insert(Range {
        kind: t.kind,
        why,
        at: t.at,
        len: t.len,
        n: 0,
        us: 0,
        ages: Vec::new(),
        repeats: 0,
        writer: writer.clone(),
    });
    r.n += 1;
    r.us += us;
    if r.ages.len() < 64 {
        r.ages.push(age);
    }
    r.repeats += u64::from(repeat);
    if writer.is_some() {
        r.writer = writer.clone();
    }
    drop(held);
    if log_on() {
        static SECOND: AtomicU64 = AtomicU64::new(0);
        static IN_IT: AtomicU64 = AtomicU64::new(0);
        let sec = now / 1000;
        if SECOND.swap(sec, Relaxed) != sec {
            IN_IT.store(0, Relaxed);
        }
        if IN_IT.fetch_add(1, Relaxed) < 50 {
            tracing::info!(
                "card wait: {:.2} ms, {} ({}), read {:#x}+{:#x}, flights may write {:#x}..{:#x}, oldest flight {} ms{}{}",
                us as f64 / 1000.0,
                t.kind,
                why,
                t.at,
                t.len,
                t.written.0,
                t.written.1,
                age,
                if repeat { ", same page within 1 s" } else { "" },
                writer.as_ref().map_or(String::new(), |w| format!(", last writer {w}"))
            );
        }
    }
}

/// With the frame line: the waits since the last line.
pub(crate) fn line(frames: f64) {
    if frames <= 0.0 {
        return;
    }
    let Some(s) = STATS.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        tracing::info!(frames = frames as u64, "card waits: none");
        return;
    };
    let mut kinds: Vec<_> = s.kinds.iter().collect();
    kinds.sort_by(|a, b| b.1.1.cmp(&a.1.1));
    let kinds: Vec<String> = kinds
        .iter()
        .map(|&(k, &(n, us))| format!("{k} {:.1}/f {:.1} ms/f", n as f64 / frames, us as f64 / 1000.0 / frames))
        .collect();
    let mut ranges: Vec<&Range> = s.ranges.values().filter(|r| r.len > 0).collect();
    ranges.sort_by(|a, b| b.us.cmp(&a.us));
    let top: Vec<String> = ranges
        .iter()
        .take(8)
        .map(|r| {
            let mut ages = r.ages.clone();
            ages.sort_unstable();
            let med = ages.get(ages.len() / 2).copied().unwrap_or(0);
            format!(
                "{:#x}+{:#x} {} ({}): {} waits {:.1} ms, oldest flight median {} ms, {} again within 1 s{}",
                r.at,
                r.len,
                r.kind,
                r.why,
                r.n,
                r.us as f64 / 1000.0,
                med,
                r.repeats,
                r.writer.as_ref().map_or(String::new(), |w| format!(", last writer {w}"))
            )
        })
        .collect();
    tracing::info!(
        frames = frames as u64,
        "card waits: {:.1}/f, {:.1} ms/f; by trigger: {}; top ranges: {}",
        s.waits as f64 / frames,
        s.us as f64 / 1000.0 / frames,
        kinds.join(" | "),
        if top.is_empty() { "none with a range".to_owned() } else { top.join(" | ") }
    );
}
