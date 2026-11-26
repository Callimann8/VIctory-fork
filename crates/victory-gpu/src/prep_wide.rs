//! A submission's CPU work spread over several threads
//! (`VICTORY_PREP_WIDE=1`; off by default).
//!
//! # Where the time goes
//!
//! The title records its command lists on its own threads; each list call
//! records the host list at once (`crate::list`), on that thread. What our
//! one submitting thread does for a frame is the lists' deferred steps
//! (`Host::run_deferred`): bring the host copies of the title buffers the
//! lists read up to date ("refreshes": change checks, guards, moves,
//! sparse reads), copy constants into upload memory ("fills"), bring
//! textures' staging up to date, and keep the bytes of the ranges shaders
//! will write ("snapshots"). In the lit cell with ray tracing on that is
//! all of the frame line's "preparing" (155 ms of which 154 ms copying,
//! profile of 90fea82). So the work to share out is those steps, not the
//! recording of lists.
//!
//! # What may run beside what
//!
//! The steps of a submission run in recorded order. Some of them write
//! title memory or anything at all, and nothing may pass them:
//!
//! - a title buffer copy (`Step::Copy`): later steps must read its bytes;
//! - work that may write title memory (`Step::Run`);
//! - work that writes host memory (`Step::Host`): where is not known.
//!
//! Between two of those barriers the steps only READ title memory, and each
//! writes a place of its own:
//!
//! - a fill writes the list's own upload memory;
//! - a texture's refresh writes its staging buffer (each texture once per
//!   stretch, as before: `Batch::texture_due`);
//! - a refresh of a host copy writes that copy; the snapshot of a range
//!   shaders will write reads that copy and writes its snapshot, and from
//!   then on refreshes of that copy leave the range alone.
//!
//! So one stretch between barriers (a [`Segment`]) is cut into independent
//! pieces: the fills, each texture, and per host copy a chain of its
//! refreshes and snapshots in their recorded order. Pieces of different
//! copies share nothing but the write-watch journal and the guards, which
//! have their own locks (as for `VICTORY_WIDE_REFRESH`, which ran one
//! flush's refreshes this way). The pieces run on the kept helper threads
//! (`crate::shadow::wide_run`) and on the submitting thread, and the
//! barrier runs once all of them are done, on the submitting thread, as
//! before. The lists go to the card only after the last stretch, so no list
//! can read a byte before its step has written it.
//!
//! What changes in order, against the steps one by one: a fill or a
//! texture is done at the end of its stretch, not at its place in it, and
//! refreshes of a copy that a snapshot did not need yet are done there too
//! (as they already were). Both only read title memory, which no step of
//! ours writes inside a stretch; the title's other threads writing while
//! the submission is prepared are seen a little later, as with
//! `VICTORY_ASK_ONCE`. Nothing about submission order, fences, descriptor
//! heaps or the card changes: that all happens after the stretches, on the
//! one thread, as before.
//!
//! `VICTORY_PREP_WIDE=serial` does the same cutting but runs the pieces one
//! after the other on the submitting thread, each timed: it shows whether
//! the new order alone changes anything, and how much faster the stretches
//! could be with more threads (the log line's "estimate").
//! `VICTORY_PREP_WIDE_MIN` (default 4): a stretch with fewer pieces runs on
//! the submitting thread (waking the helpers costs more than it saves).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// What [`mode`] says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Steps one by one, as before.
    Off,
    /// Stretches cut into pieces, run one after the other here, timed.
    Serial,
    /// Stretches cut into pieces, run on several threads.
    Wide,
}

fn parse(v: Option<&str>) -> Mode {
    match v.map(str::trim) {
        Some("1" | "wide") => Mode::Wide,
        Some("serial") => Mode::Serial,
        _ => Mode::Off,
    }
}

/// `VICTORY_PREP_WIDE`: unset or 0 off, `1` on, `serial` cut but not shared.
pub(crate) fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = parse(std::env::var("VICTORY_PREP_WIDE").ok().as_deref());
        if mode != Mode::Off {
            tracing::info!(
                ?mode,
                least = least(),
                "a submission's steps between title writes are cut into independent pieces (VICTORY_PREP_WIDE)"
            );
        }
        mode
    })
}

/// The fewest pieces a stretch is shared out from (`VICTORY_PREP_WIDE_MIN`).
pub(crate) fn least() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VICTORY_PREP_WIDE_MIN").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4).max(2)
    })
}

/// One step of a host copy's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op<S> {
    /// Bring `[from, to)` of the copy up to date.
    Update(S, usize, usize),
    /// Shaders will write `[address, address + len)`: keep its bytes.
    Writing(S, usize, usize),
}

/// A fill: destination, source, bytes.
pub(crate) type Fill = (usize, usize, usize);

/// One piece of a stretch.
pub(crate) enum Piece<S, T> {
    /// A host copy's steps, in recorded order.
    Chain(Vec<Op<S>>),
    /// Some fills.
    Fills(Vec<Fill>),
    /// A texture's staging to bring up to date.
    Texture(T),
}

/// The steps of a stretch between two barriers, sorted into pieces.
pub(crate) struct Segment<S, T> {
    /// Per host copy (by its address in `index`), its steps in order.
    chains: Vec<Vec<Op<S>>>,
    index: HashMap<usize, usize>,
    fills: Vec<Fill>,
    textures: Vec<T>,
}

impl<S, T> Default for Segment<S, T> {
    fn default() -> Self {
        Self {
            chains: Vec::new(),
            index: HashMap::new(),
            fills: Vec::new(),
            textures: Vec::new(),
        }
    }
}

/// Fills to a piece: about this many bytes, or this many fills.
const FILL_BYTES: usize = 256 << 10;
const FILL_COUNT: usize = 256;

impl<S, T> Segment<S, T> {
    pub(crate) fn is_empty(&self) -> bool {
        self.chains.is_empty() && self.fills.is_empty() && self.textures.is_empty()
    }

    fn chain(&mut self, key: usize) -> &mut Vec<Op<S>> {
        let at = *self.index.entry(key).or_insert_with(|| {
            self.chains.push(Vec::new());
            self.chains.len() - 1
        });
        &mut self.chains[at]
    }

    /// The copy `key` (its address) is to be brought up to date in `[from, to)`.
    pub(crate) fn update(&mut self, key: usize, shadow: S, from: usize, to: usize) {
        if from < to {
            self.chain(key).push(Op::Update(shadow, from, to));
        }
    }

    /// Shaders will write `[address, address + len)` of the copy `key`.
    pub(crate) fn writing(&mut self, key: usize, shadow: S, address: usize, len: usize) {
        self.chain(key).push(Op::Writing(shadow, address, len));
    }

    pub(crate) fn fill(&mut self, dst: usize, src: usize, len: usize) {
        self.fills.push((dst, src, len));
    }

    pub(crate) fn texture(&mut self, staging: T) {
        self.textures.push(staging);
    }

    /// The ranges the stretch brings up to date (for one write-watch asking).
    pub(crate) fn update_ranges(&self, watched: impl Fn(&S) -> bool) -> Vec<(usize, usize)> {
        self.chains
            .iter()
            .flatten()
            .filter_map(|op| match op {
                Op::Update(s, from, to) if watched(s) => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    /// The stretch as independent pieces: each chain, each texture, the
    /// fills in groups. Taken, the stretch is empty again.
    pub(crate) fn take(&mut self) -> Vec<Piece<S, T>> {
        self.index.clear();
        let mut pieces: Vec<Piece<S, T>> = self.chains.drain(..).map(Piece::Chain).collect();
        pieces.extend(self.textures.drain(..).map(Piece::Texture));
        let mut group = Vec::new();
        let mut bytes = 0;
        for fill in self.fills.drain(..) {
            bytes += fill.2;
            group.push(fill);
            if bytes >= FILL_BYTES || group.len() >= FILL_COUNT {
                pieces.push(Piece::Fills(std::mem::take(&mut group)));
                bytes = 0;
            }
        }
        if !group.is_empty() {
            pieces.push(Piece::Fills(group));
        }
        pieces
    }
}

/// A rough cost of a piece, to give the threads like loads: bytes moved
/// or looked at, plus something for each step.
pub(crate) fn cost<S, T>(piece: &Piece<S, T>) -> u64 {
    match piece {
        Piece::Chain(ops) => ops
            .iter()
            .map(|op| match op {
                // Most of a refresh is asking what changed; ~1/16 of the
                // range is a fair guess of what it copies.
                Op::Update(_, from, to) => 4096 + ((to - from) / 16) as u64,
                Op::Writing(_, _, len) => 1024 + *len as u64,
            })
            .sum(),
        Piece::Fills(fills) => fills.iter().map(|f| 64 + f.2 as u64).sum(),
        Piece::Texture(_) => 64 << 10,
    }
}

/// Sorts pieces into `bins` groups of like cost (longest first, each to the
/// lightest group). Groups that would be empty are left out.
pub(crate) fn pack<P>(pieces: Vec<P>, bins: usize, cost: impl Fn(&P) -> u64) -> Vec<Vec<P>> {
    let bins = bins.max(1).min(pieces.len().max(1));
    let mut costed: Vec<(u64, P)> = pieces.into_iter().map(|p| (cost(&p), p)).collect();
    costed.sort_by_key(|c| std::cmp::Reverse(c.0));
    let mut groups: Vec<(u64, Vec<P>)> = (0..bins).map(|_| (0, Vec::new())).collect();
    for (c, p) in costed {
        let lightest = groups.iter_mut().min_by_key(|g| g.0).expect("bins >= 1");
        lightest.0 += c;
        lightest.1.push(p);
    }
    groups.into_iter().map(|g| g.1).filter(|g| !g.is_empty()).collect()
}

/// What the stretches cost, since the last log line: `[stretches, of them
/// shared out, pieces, pieces' summed ns, sum over stretches of the
/// longest piece ns, sum over stretches of the wall ns, estimate ns on the
/// threads there are, barriers: copies, runs, host work]`.
static STATS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
const S_STRETCHES: usize = 0;
const S_WIDE: usize = 1;
const S_PIECES: usize = 2;
const S_SUMMED: usize = 3;
const S_LONGEST: usize = 4;
const S_WALL: usize = 5;
const S_ESTIMATE: usize = 6;
pub(crate) const B_COPY: usize = 7;
pub(crate) const B_RUN: usize = 8;
pub(crate) const B_HOST: usize = 9;

/// The time a stretch would take on `threads`, from its pieces' times: no
/// less than its longest piece, no less than its share of the sum.
pub(crate) fn estimate(times: &[u64], threads: u64) -> u64 {
    let longest = times.iter().copied().max().unwrap_or(0);
    let sum: u64 = times.iter().sum();
    longest.max(sum.div_ceil(threads.max(1)))
}

/// A stretch ran: `times`, its pieces' ns if they were timed one by one
/// (serial), else empty; `wall`, its ns; `wide`, whether it was shared out;
/// `threads`, the threads it could have had (helpers and this one).
pub(crate) fn ran(pieces: usize, times: &[u64], wall: u64, wide: bool, threads: u64) {
    THREADS.store(threads, Relaxed);
    STATS[S_STRETCHES].fetch_add(1, Relaxed);
    STATS[S_WIDE].fetch_add(u64::from(wide), Relaxed);
    STATS[S_PIECES].fetch_add(pieces as u64, Relaxed);
    STATS[S_WALL].fetch_add(wall, Relaxed);
    if !times.is_empty() {
        STATS[S_SUMMED].fetch_add(times.iter().sum(), Relaxed);
        STATS[S_LONGEST].fetch_add(times.iter().copied().max().unwrap_or(0), Relaxed);
        STATS[S_ESTIMATE].fetch_add(estimate(times, threads), Relaxed);
    }
}

/// The threads of the last stretch, for the log line.
static THREADS: AtomicU64 = AtomicU64::new(1);

/// A stretch ended at a barrier of kind `B_COPY`, `B_RUN` or `B_HOST`.
pub(crate) fn barrier(kind: usize) {
    STATS[kind].fetch_add(1, Relaxed);
}

/// A submission is prepared: every 512 of them a log line.
pub(crate) fn submitted() {
    static N: AtomicU64 = AtomicU64::new(0);
    if mode() == Mode::Off || !(N.fetch_add(1, Relaxed) + 1).is_multiple_of(512) {
        return;
    }
    let s: Vec<u64> = STATS.iter().map(|n| n.swap(0, Relaxed)).collect();
    let per = |n: u64| n as f64 / 512.0;
    let threads = THREADS.load(Relaxed);
    if s[S_SUMMED] > 0 {
        tracing::info!(
            "prepared in stretches (VICTORY_PREP_WIDE=serial), per submission: {:.1} stretches, {:.1} pieces, {:.2} ms one after the other; \
             longest pieces {:.2} ms; estimate on {threads} threads {:.2} ms; barriers {:.1} copies {:.1} runs {:.1} host work",
            per(s[S_STRETCHES]),
            per(s[S_PIECES]),
            per(s[S_SUMMED]) / 1e6,
            per(s[S_LONGEST]) / 1e6,
            per(s[S_ESTIMATE]) / 1e6,
            per(s[B_COPY]),
            per(s[B_RUN]),
            per(s[B_HOST]),
        );
    } else {
        tracing::info!(
            "prepared in stretches (VICTORY_PREP_WIDE=1), per submission: {:.1} stretches ({:.1} shared out), {:.1} pieces, {:.2} ms; \
             barriers {:.1} copies {:.1} runs {:.1} host work",
            per(s[S_STRETCHES]),
            per(s[S_WIDE]),
            per(s[S_PIECES]),
            per(s[S_WALL]) / 1e6,
            per(s[B_COPY]),
            per(s[B_RUN]),
            per(s[B_HOST]),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Seg = Segment<char, u32>;

    #[test]
    fn switch_values() {
        assert_eq!(parse(None), Mode::Off);
        assert_eq!(parse(Some("0")), Mode::Off);
        assert_eq!(parse(Some("1")), Mode::Wide);
        assert_eq!(parse(Some(" serial ")), Mode::Serial);
        assert_eq!(parse(Some("yes")), Mode::Off);
    }

    #[test]
    fn a_copys_steps_keep_their_order_and_copies_are_apart() {
        let mut seg = Seg::default();
        seg.update(1, 'a', 0, 100);
        seg.update(2, 'b', 0, 50);
        seg.writing(1, 'a', 10, 20);
        seg.fill(7, 8, 9);
        seg.update(1, 'a', 0, 100);
        seg.texture(42);
        seg.update(3, 'c', 5, 5); // empty: nothing to do
        let pieces = seg.take();
        assert!(seg.is_empty());
        let chains: Vec<&Vec<Op<char>>> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Chain(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(
            chains,
            [
                &vec![Op::Update('a', 0, 100), Op::Writing('a', 10, 20), Op::Update('a', 0, 100)],
                &vec![Op::Update('b', 0, 50)],
            ]
        );
        assert_eq!(pieces.iter().filter(|p| matches!(p, Piece::Texture(42))).count(), 1);
        assert_eq!(pieces.iter().filter(|p| matches!(p, Piece::Fills(f) if f == &[(7, 8, 9)])).count(), 1);
    }

    #[test]
    fn fills_go_in_groups_every_fill_once() {
        let mut seg = Seg::default();
        for i in 0..1000 {
            seg.fill(i, i, 100);
        }
        seg.fill(5000, 5000, 1 << 20);
        let pieces = seg.take();
        let mut seen: Vec<usize> = pieces
            .iter()
            .flat_map(|p| match p {
                Piece::Fills(f) => f.iter().map(|f| f.0).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert!(pieces.len() >= 4 && pieces.len() <= 6, "{} groups", pieces.len());
        seen.sort_unstable();
        let mut want: Vec<usize> = (0..1000).collect();
        want.push(5000);
        assert_eq!(seen, want);
    }

    #[test]
    fn update_ranges_leave_out_unwatched_copies() {
        let mut seg = Seg::default();
        seg.update(1, 'a', 0, 10);
        seg.writing(1, 'a', 0, 4);
        seg.update(2, 'u', 20, 30);
        seg.update(1, 'a', 40, 50);
        assert_eq!(seg.update_ranges(|s| *s != 'u'), [(0, 10), (40, 50)]);
    }

    #[test]
    fn packing_keeps_every_piece_and_evens_the_load() {
        let groups = pack(vec![9u64, 1, 1, 1, 8, 2, 7, 3], 3, |&c| c);
        let mut all: Vec<u64> = groups.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, [1, 1, 1, 2, 3, 7, 8, 9]);
        let loads: Vec<u64> = groups.iter().map(|g| g.iter().sum()).collect();
        assert_eq!(loads.len(), 3);
        assert!(loads.iter().max().unwrap() - loads.iter().min().unwrap() <= 2, "{loads:?}");
        // More bins than pieces: one piece a group, none empty.
        assert_eq!(pack(vec![1u64, 2], 8, |&c| c).len(), 2);
        assert!(pack(Vec::<u64>::new(), 4, |&c| c).is_empty());
    }

    #[test]
    fn estimate_is_the_longest_piece_or_the_share() {
        assert_eq!(estimate(&[], 7), 0);
        assert_eq!(estimate(&[100, 1, 1], 7), 100);
        assert_eq!(estimate(&[10; 14], 7), 20);
        assert_eq!(estimate(&[5, 5], 1), 10);
    }
}
