//! Diagnostics: what the card's time goes to, by kind of list and by
//! pipeline, in a run like any other (`VICTORY_CARD_METER=1`; with
//! `VICTORY_CARD_METER_PIPELINES=1` pipeline by pipeline too).
//!
//! The stamps around submissions (`spans`) say how long the card worked on
//! each, not on what. The timing run (`VICTORY_GPU_TIMING`) names every
//! command, but sends each list on its own and waits for it, without the
//! submissions sent ahead: another frame than the one asked about. Run A5's
//! opening had the card working 140 ms a frame with single submissions of
//! 1.2 to 1.5 s, and nothing said which lists those were.
//!
//! Here a list gets one timestamp where it begins and one where it ends,
//! both resolved into one read-back buffer by the list itself: the title's
//! lists, the state preludes in front of them (with the texture uploads they
//! carry, or without), the list that brings copies in the card's memory up
//! to date and the one that compares them. Inside the title's lists the same
//! from where a pipeline is bound to where the next one is. A pair is read
//! some seconds after its list was closed, so the table is of the frames of
//! a moment ago; a single span of a tenth of a second or more is logged by
//! itself, with how long ago its list was closed.
//!
//! Off unless asked for: two queries and a resolve a list, and as many at
//! every change of pipeline.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::*;

use super::{HOST, Host};

/// What a timed span is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// A host list of the title's commands, or one of our present copies.
    Lists,
    /// A state prelude without texture uploads: barriers and twin copies.
    States,
    /// A state prelude that carries texture uploads.
    Uploads,
    /// Ours: changed bytes go up to copies in the card's memory.
    Sync,
    /// Ours: the compare of what a submission's shaders may have written.
    Check,
    /// Inside a list: from where a pipeline is bound to where the next one
    /// is, or the list ends.
    Pipeline,
}

/// The kinds that are whole lists, as the table names them.
const WHOLE: [(Kind, &str); 5] = [
    (Kind::Lists, "the title's lists"),
    (Kind::States, "state preludes without uploads"),
    (Kind::Uploads, "state preludes with texture uploads"),
    (Kind::Sync, "lists that bring copies in the card's memory up to date"),
    (Kind::Check, "lists that compare copies in the card's memory"),
];

/// The most pairs of timestamps in the ring (fewer if the device refuses a
/// heap of that many).
const PAIRS: u32 = 1 << 18;
/// A pair is read this long after its list was closed, and that many times
/// again if the card has not written it by then.
const AFTER: Duration = Duration::from_secs(4);
const TRIES: u8 = 3;
/// A single span of this many microseconds or more is logged by itself, at
/// most [`LONG_LINES`] of them a run.
const LONG_US: u64 = 100_000;
const LONG_LINES: u32 = 400;

/// A pair being recorded: its place in the ring and the round of the ring
/// it was taken in, or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Span(u64);

impl Span {
    pub(crate) const NONE: Span = Span(u64::MAX);

    fn of(pair: u32, round: u32) -> Span {
        Span(u64::from(pair) | u64::from(round) << 32)
    }

    fn pair(self) -> u32 {
        self.0 as u32
    }

    fn round(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// For keeping in an atomic.
    pub(crate) fn raw(self) -> u64 {
        self.0
    }

    pub(crate) fn from_raw(raw: u64) -> Span {
        Span(raw)
    }
}

/// Spans of one name: how many, their microseconds, the longest.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Sum {
    count: u64,
    us: u64,
    longest: u64,
}

impl Sum {
    fn add(&mut self, us: u64) {
        self.count += 1;
        self.us += us;
        self.longest = self.longest.max(us);
    }
}

/// A pair whose list was closed, not read yet.
struct Pending {
    pair: u32,
    round: u32,
    kind: Kind,
    /// When the list was closed, and when the pair is read next.
    closed: Instant,
    due: Instant,
    tries: u8,
}

struct Meter {
    heap: ID3D12QueryHeap,
    back: ID3D12Resource,
    /// The read-back buffer's bytes, mapped for good.
    mapped: usize,
    /// Ticks a second of the queue's clock.
    frequency: u64,
    pairs: u32,
    next: u32,
    /// How often each place was taken: a span of an earlier round is
    /// another's by now.
    rounds: Vec<u32>,
    begun: u64,
    pending: VecDeque<Pending>,
    /// A span's name by place, until the pair is read.
    names: HashMap<u32, String>,
    /// Since the last table: by kind ([`WHOLE`]), by what a title list
    /// binds, by pipeline.
    whole: [Sum; 5],
    by_name: HashMap<String, Sum>,
    by_pipeline: HashMap<String, Sum>,
    /// What each place held when it was last read: one the card has not
    /// written since still holds that.
    seen: Vec<(u64, u64)>,
    /// Pairs the card never wrote (their lists did not run), and spans not
    /// taken because the ring was full.
    lost: u64,
    refused: u64,
    long_lines: u32,
}

// SAFETY: host objects (free-threaded) and a mapping only read, behind a lock.
unsafe impl Send for Meter {}

static METER: Mutex<Option<Option<Meter>>> = Mutex::new(None);

/// Whether lists are timed.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CARD_METER").is_some_and(|v| v == "1"))
}

/// Whether the card's time is also measured pipeline by pipeline inside the
/// title's lists (`VICTORY_CARD_METER_PIPELINES=1`, with the meter on).
pub(crate) fn pipelines() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var_os("VICTORY_CARD_METER_PIPELINES").is_some_and(|v| v == "1"))
}

fn make(host: &Host) -> Option<Meter> {
    // SAFETY: plain object creation; the buffer is mapped once and stays so.
    unsafe {
        let mut pairs = PAIRS;
        let heap = loop {
            let mut heap: Option<ID3D12QueryHeap> = None;
            let made = host.device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: pairs * 2, NodeMask: 0 },
                &mut heap,
            );
            match (made, heap) {
                (Ok(()), Some(heap)) => break heap,
                _ if pairs > 8192 => pairs /= 2,
                (made, _) => {
                    tracing::warn!(error = ?made.err(), "the card's time is not measured by list: no heap of timestamps");
                    return None;
                }
            }
        };
        let back = host.read_back_buffer(u64::from(pairs) * 16)?;
        let mut mapped = std::ptr::null_mut();
        back.Map(0, None, Some(&mut mapped)).ok()?;
        let frequency = host.queue.GetTimestampFrequency().unwrap_or(0);
        if mapped.is_null() || frequency == 0 {
            tracing::warn!("the card's time is not measured by list: no mapping or no clock");
            return None;
        }
        tracing::info!(
            pairs,
            pipelines = pipelines(),
            "the card's time is measured by kind of list, a timestamp where each list begins and ends (VICTORY_CARD_METER=1)"
        );
        Some(Meter {
            heap,
            back,
            mapped: mapped as usize,
            frequency,
            pairs,
            next: 0,
            rounds: vec![0; pairs as usize],
            begun: 0,
            pending: VecDeque::new(),
            names: HashMap::new(),
            whole: [Sum::default(); 5],
            by_name: HashMap::new(),
            by_pipeline: HashMap::new(),
            seen: vec![(0, 0); pairs as usize],
            lost: 0,
            refused: 0,
            long_lines: 0,
        })
    }
}

/// A span begins here on `list`: the first timestamp of a pair.
pub(crate) fn begin(list: &ID3D12GraphicsCommandList) -> Span {
    begin_as(list, None)
}

/// The pipeline `name` is bound on `list` here: the first timestamp of a
/// pair that [`end`] closes where the next pipeline is bound or the list
/// ends. What the card does between the two (the draws or dispatches of
/// that pipeline, and what the layer records around them) is that
/// pipeline's time in the table.
pub(crate) fn begin_named(list: &ID3D12GraphicsCommandList, name: &str) -> Span {
    begin_as(list, Some(name))
}

fn begin_as(list: &ID3D12GraphicsCommandList, name: Option<&str>) -> Span {
    if !on() {
        return Span::NONE;
    }
    let mut meter = METER.lock().unwrap_or_else(|e| e.into_inner());
    if meter.is_none() {
        // (A list made before the host is: not timed, and asked again. Not
        // `host()`, which would make it from inside its own making.)
        let Some(host) = HOST.get().and_then(Option::as_ref) else {
            return Span::NONE;
        };
        *meter = Some(make(host));
    }
    let Some(Some(m)) = meter.as_mut() else {
        return Span::NONE;
    };
    m.begun += 1;
    if m.begun.is_multiple_of(2048) {
        m.collect();
    }
    // The ring must not come round to pairs that were not read yet.
    if m.pending.len() as u32 >= m.pairs - 64 {
        m.refused += 1;
        return Span::NONE;
    }
    let pair = m.next;
    m.next = (m.next + 1) % m.pairs;
    let round = m.rounds[pair as usize].wrapping_add(1);
    m.rounds[pair as usize] = round;
    match name {
        Some(name) => m.names.insert(pair, name.to_owned()),
        None => m.names.remove(&pair),
    };
    // SAFETY: an open list and our query heap.
    unsafe { list.EndQuery(&m.heap, D3D12_QUERY_TYPE_TIMESTAMP, pair * 2) };
    Span::of(pair, round)
}

/// The span ends here, as one of `kind`: the pair's second timestamp, and
/// both go to the read-back buffer when the card has run the list.
pub(crate) fn end(list: &ID3D12GraphicsCommandList, span: Span, kind: Kind) {
    end_as(list, span, kind, None);
}

/// [`end`] with what the span is called in the table.
pub(crate) fn end_named(list: &ID3D12GraphicsCommandList, span: Span, kind: Kind, name: String) {
    end_as(list, span, kind, Some(name));
}

fn end_as(list: &ID3D12GraphicsCommandList, span: Span, kind: Kind, name: Option<String>) {
    if span == Span::NONE {
        return;
    }
    let mut meter = METER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(m)) = meter.as_mut() else {
        return;
    };
    let pair = span.pair();
    // The ring came round while the list stood open: the place is another's.
    if pair >= m.pairs || m.rounds[pair as usize] != span.round() {
        m.lost += 1;
        return;
    }
    // SAFETY: an open list, our heap and our read-back buffer, the pair's
    // own sixteen bytes of it.
    unsafe {
        list.EndQuery(&m.heap, D3D12_QUERY_TYPE_TIMESTAMP, pair * 2 + 1);
        list.ResolveQueryData(&m.heap, D3D12_QUERY_TYPE_TIMESTAMP, pair * 2, 2, &m.back, u64::from(pair) * 16);
    }
    if let Some(name) = name {
        m.names.insert(pair, name);
    }
    let closed = Instant::now();
    m.pending.push_back(Pending { pair, round: span.round(), kind, closed, due: closed + AFTER, tries: 0 });
}

/// A title list's name in the table: how many pipelines it binds and the
/// first two of them (the set is in the order of the names, so a pass has
/// the same name every frame).
pub(crate) fn name_of(names: &BTreeSet<String>) -> String {
    let short = |n: &String| n.chars().take(60).collect::<String>();
    let mut first = names.iter();
    match (first.next(), first.next()) {
        (None, _) => "no pipeline (copies, clears, barriers)".to_string(),
        (Some(a), None) => format!("1 pipeline: {}", short(a)),
        (Some(a), Some(b)) => format!("{} pipelines: {} / {}", names.len(), short(a), short(b)),
    }
}

/// Microseconds between two timestamps of a clock of `frequency` ticks a
/// second; 0 if the second is not after the first (a pair the card has not
/// written, or a list that never ran).
fn micros(begin: u64, end: u64, frequency: u64) -> u64 {
    if end <= begin || frequency == 0 {
        return 0;
    }
    ((end - begin) as u128 * 1_000_000 / frequency as u128).min(10_000_000) as u64
}

impl Meter {
    /// The pairs whose lists were closed long enough ago are read and added
    /// to the sums.
    fn collect(&mut self) {
        let now = Instant::now();
        let mut again = Vec::new();
        while let Some(p) = self.pending.front() {
            if now < p.due {
                break;
            }
            let Some(p) = self.pending.pop_front() else {
                break;
            };
            let at = p.pair as usize;
            if self.rounds[at] != p.round {
                self.lost += 1;
                continue;
            }
            // SAFETY: the pair's sixteen bytes of the mapped read-back
            // buffer; the card wrote them a while ago or never.
            let read = unsafe {
                let words = (self.mapped as *const u64).add(at * 2);
                (words.read_volatile(), words.add(1).read_volatile())
            };
            if self.seen[at] == read {
                // Not written since the place was last read.
                if p.tries < TRIES {
                    again.push(Pending { due: now + AFTER, tries: p.tries + 1, ..p });
                } else {
                    self.lost += 1;
                    self.names.remove(&p.pair);
                }
                continue;
            }
            self.seen[at] = read;
            let us = micros(read.0, read.1, self.frequency);
            let name = self.names.remove(&p.pair);
            if us >= LONG_US && self.long_lines < LONG_LINES {
                self.long_lines += 1;
                let what = match p.kind {
                    Kind::Pipeline => "one binding of a pipeline",
                    kind => WHOLE.iter().find(|w| w.0 == kind).map_or("a list", |w| w.1),
                };
                tracing::info!(
                    "the card took {:.0} ms for one span of: {what}, closed {:.1} s ago: {}",
                    us as f64 / 1e3,
                    now.duration_since(p.closed).as_secs_f64(),
                    name.as_deref().unwrap_or("-")
                );
            }
            match p.kind {
                Kind::Pipeline => self.by_pipeline.entry(name.unwrap_or_default()).or_default().add(us),
                kind => {
                    if let Some(i) = WHOLE.iter().position(|w| w.0 == kind) {
                        self.whole[i].add(us);
                    }
                    if let (Kind::Lists, Some(name)) = (kind, name) {
                        self.by_name.entry(name).or_default().add(us);
                    }
                }
            }
        }
        // (Due later than everything still waiting: the order holds.)
        self.pending.extend(again);
    }
}

/// The largest of `sums` by time, at most `most` and none under `least_ms` a
/// frame: (name, milliseconds a frame, spans a frame, the longest in
/// milliseconds).
fn largest(sums: &mut HashMap<String, Sum>, frames: f64, most: usize, least_ms: f64) -> (f64, usize, Vec<(String, f64, f64, f64)>) {
    let mut table: Vec<(String, Sum)> = sums.drain().collect();
    table.sort_by(|a, b| b.1.us.cmp(&a.1.us).then_with(|| a.0.cmp(&b.0)));
    let all = table.iter().map(|t| t.1.us).sum::<u64>() as f64 / 1e3 / frames;
    let names = table.len();
    let rows = table
        .into_iter()
        .map(|(name, s)| (name, s.us as f64 / 1e3 / frames, s.count as f64 / frames, s.longest as f64 / 1e3))
        .filter(|row| row.1 >= least_ms)
        .take(most)
        .collect();
    (all, names, rows)
}

/// For the frame-rate line: the card's time by kind of list, by what the
/// title's lists bind and by pipeline, per frame, of the lists read since
/// the last one (closed [`AFTER`] ago or more).
pub(super) fn line(frames: f64) {
    if !on() || frames <= 0.0 {
        return;
    }
    let mut meter = METER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(m)) = meter.as_mut() else {
        return;
    };
    m.collect();
    let whole = std::mem::take(&mut m.whole);
    let (lost, refused) = (std::mem::take(&mut m.lost), std::mem::take(&mut m.refused));
    if whole.iter().all(|s| s.count == 0) && m.by_pipeline.is_empty() {
        return;
    }
    let kinds: Vec<String> = WHOLE
        .iter()
        .zip(&whole)
        .filter(|(_, s)| s.count > 0)
        .map(|((_, what), s)| {
            format!(
                "{what} {:.1} ms in {:.1} (the longest {:.1} ms)",
                s.us as f64 / 1e3 / frames,
                s.count as f64 / frames,
                s.longest as f64 / 1e3
            )
        })
        .collect();
    tracing::info!(
        "card time by kind of list, per frame, of lists closed {} s ago or more: {}; {lost} spans never written, {refused} not taken",
        AFTER.as_secs(),
        kinds.join("; ")
    );
    let (all, names, rows) = largest(&mut m.by_name, frames, 8, 1.0);
    if names > 0 {
        tracing::info!("card time of the title's lists by what they bind, per frame: {all:.1} ms in {names} kinds; the largest follow");
        for (name, ms, count, longest) in rows {
            tracing::info!("  card {ms:7.2} ms a frame in {count:5.1} lists, the longest {longest:7.1} ms: {name}");
        }
    }
    let (all, names, rows) = largest(&mut m.by_pipeline, frames, 20, 0.5);
    if names > 0 {
        tracing::info!(
            "card time by pipeline (from where it is bound to where the next is), per frame: {all:.1} ms of {names} pipelines; the largest follow"
        );
        for (name, ms, count, longest) in rows {
            tracing::info!("  pipeline {ms:7.2} ms a frame in {count:6.1} bindings, the longest {longest:7.1} ms: {name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_named_by_the_pipelines_it_binds() {
        let mut names = BTreeSet::new();
        assert_eq!(name_of(&names), "no pipeline (copies, clears, barriers)");
        names.insert("compute CS_Blur".to_string());
        assert_eq!(name_of(&names), "1 pipeline: compute CS_Blur");
        names.insert("mesh VS_A, pixel PS_B".to_string());
        names.insert("a".repeat(100));
        assert_eq!(name_of(&names), format!("3 pipelines: {} / compute CS_Blur", "a".repeat(60)));
    }

    #[test]
    fn a_pairs_length_is_in_microseconds_of_the_queues_clock() {
        assert_eq!(micros(1_000, 11_000, 10_000_000), 1_000);
        assert_eq!(micros(0, 25_000_000, 25_000_000), 1_000_000);
        // Not written, or the wrong way round: nothing.
        assert_eq!(micros(5, 5, 1_000), 0);
        assert_eq!(micros(9, 5, 1_000), 0);
        assert_eq!(micros(1, 2, 0), 0);
        // A stale second stamp from the ring's last round cannot add hours.
        assert_eq!(micros(1, u64::MAX, 1), 10_000_000);
    }

    #[test]
    fn a_span_keeps_its_place_and_its_round() {
        let span = Span::of(262_143, 7);
        assert_eq!((span.pair(), span.round()), (262_143, 7));
        assert_eq!(Span::from_raw(span.raw()), span);
        // No place of the ring is the empty span, in any round.
        assert_ne!(Span::of(PAIRS - 1, u32::MAX), Span::NONE);
    }

    #[test]
    fn the_table_has_the_largest_first_and_leaves_out_the_small() {
        let mut sums = HashMap::new();
        for (name, us) in [("a", 4_000), ("b", 30_000), ("b", 10_000), ("c", 100)] {
            sums.entry(name.to_string()).or_insert_with(Sum::default).add(us);
        }
        // Over two frames; under 1 ms a frame is left out of the rows, not
        // of the total.
        let (all, names, rows) = largest(&mut sums, 2.0, 8, 1.0);
        assert!((all - 22.05).abs() < 1e-9);
        assert_eq!(names, 3);
        assert_eq!(rows, vec![("b".to_string(), 20.0, 1.0, 30.0), ("a".to_string(), 2.0, 0.5, 4.0)]);
        assert!(sums.is_empty());
    }
}
