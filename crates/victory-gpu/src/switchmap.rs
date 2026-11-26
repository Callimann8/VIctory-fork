//! The switch map: for every switch of the test plans, what it did in the
//! last stretch of frames, said in one log line with the frame line
//! ("switch map: ..."). Each switch counts where it acts ([`act`]: how
//! often, and an amount it saved or moved, in its own unit) and where it
//! turns something down ([`refuse`], with the reason). A switch that is set
//! but never acted says so (NEVER ACTED), and every `VICTORY_*` variable
//! that is set but has no counter here is listed as unmapped, so a switch
//! without gain shows whether it was idle, refused, or acted and the cost
//! went elsewhere.
//!
//! Always on and cheap (relaxed atomics; a lock only for a refusal's
//! reason). The line is also what the test runner reads.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

macro_rules! switches {
    ($($id:ident = $env:literal, $acts:literal, $unit:literal;)*) => {
        /// The switches the map counts.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        #[allow(dead_code)]
        pub enum S { $($id,)* }
        /// Per switch: its variable, what an act is, the unit of its amount
        /// ("" for none; "bytes" is shown in MB).
        const NAMES: &[(&str, &str, &str)] = &[$(($env, $acts, $unit),)*];
    };
}

switches! {
    SelfCopyShrink = "VICTORY_SELF_COPY_SHRINK", "groups shrunk", "bytes left out";
    RefreshOnce = "VICTORY_MEMORY_REFRESH_ONCE", "refreshes", "copies skipped";
    Inplace = "VICTORY_INPLACE", "windows placed", "bytes";
    PoolCommit = "VICTORY_POOL_COMMIT", "sections committed whole", "bytes";
    InplaceRuns = "VICTORY_INPLACE_RUNS", "windows cut to a run", "bytes";
    GuardPrivate = "VICTORY_GUARD_PRIVATE", "blocks taken", "faults";
    WrittenFrames = "VICTORY_MEMORY_WRITTEN_FRAMES", "pages kept marked", "pages dropped";
    CopyThreads = "VICTORY_COPY_THREADS", "copies split", "bytes";
    FetchJoin = "VICTORY_FETCH_JOIN", "fetch copies", "pieces joined";
    MapRuns = "VICTORY_MAP_RUNS", "remaps in runs", "calls saved";
    WatchFixed = "VICTORY_WATCH_FIXED", "reservations watched", "bytes";
    LookupCache = "VICTORY_LOOKUP_CACHE", "lookups answered", "lookups missed";
    HeapWorker = "VICTORY_HEAP_WORKER", "heap passes on the worker", "";
    LightRefresh = "VICTORY_LIGHT_REFRESH", "askings answered by bits", "askings to the guards";
    AbsentOnce = "VICTORY_ABSENT_ONCE", "blocks answered from their note", "questions to Windows";
    StepIndex = "VICTORY_STEP_INDEX", "copies indexed", "steps";
    FillsPlain = "VICTORY_FILLS_PLAIN", "fills copied plainly", "bytes";
    FillWaitPieces = "VICTORY_FILL_WAIT_PIECES", "pieces whose fills wait", "";
    MergeRuns = "VICTORY_MERGE_RUNS", "merges under writes", "runs merged";
    SettleRoom = "VICTORY_SETTLE_ROOM", "settles avoided", "";
    Queue2 = "VICTORY_QUEUE2", "lists on the second queue", "";
    CardBuffersMinKb = "VICTORY_CARD_BUFFERS_MIN_KB", "buffers put on the card", "bytes";
    CardBuffersMb = "VICTORY_CARD_BUFFERS_MB", "buffers kept on the card", "bytes";
    CardSyncBatch = "VICTORY_CARD_SYNC_BATCH", "batched syncs", "copies";
    CardHashWide = "VICTORY_CARD_HASH_WIDE", "wide hashes", "bytes";
    CardHashLen = "VICTORY_CARD_HASH_LEN", "hashes cut to length", "bytes";
    CodeTableAfter = "VICTORY_CODE_TABLE_AFTER", "shaders translated with it", "";
    TracerGuards = "VICTORY_TRACER_GUARDS", "shaders translated with it", "";
    LdsPrune = "VICTORY_LDS_PRUNE", "shaders translated with it", "barriers removed";
    PipelineOnce = "VICTORY_PIPELINE_ONCE", "binds", "keeps skipped";
    TableQuickHash = "VICTORY_TABLE_QUICK_HASH", "tables found again", "collisions";
    GpuWriteOnce = "VICTORY_GPU_WRITE_ONCE", "declarations", "repeats dropped";
    UploadPooled = "VICTORY_UPLOAD_POOLED", "staging from the pool", "bytes";
    FillOnce = "VICTORY_FILL_ONCE", "fills", "fills shared";
    LandedTables = "VICTORY_LANDED_TABLES", "flights whose tables went early", "lists";
    DstorageStaging = "VICTORY_DSTORAGE_STAGING_MB", "staging sizes raised", "bytes";
    RemapKeep = "VICTORY_REMAP_KEEP", "remainders kept", "bytes";
    CardRehashRuns = "VICTORY_CARD_REHASH_RUNS", "rehash runs", "";
    PrepWide = "VICTORY_PREP_WIDE", "stretches cut into pieces", "pieces";
}

const N: usize = NAMES.len();

/// Per switch since the start: acts, amount, refusals.
static COUNTS: [[AtomicU64; 3]; N] = [const { [const { AtomicU64::new(0) }; 3] }; N];

/// Refusals by switch and reason, since the start.
static REASONS: Mutex<BTreeMap<(usize, &'static str), u64>> = Mutex::new(BTreeMap::new());

/// What the last line had counted: [`line`] says the difference.
static LAST: Mutex<Option<(Vec<[u64; 3]>, BTreeMap<(usize, &'static str), u64>)>> = Mutex::new(None);

/// Switch `s` acted once, with `amount` in its unit (0 if it has none).
#[inline]
pub fn act(s: S, amount: u64) {
    let c = &COUNTS[s as usize];
    c[0].fetch_add(1, Relaxed);
    if amount != 0 {
        c[1].fetch_add(amount, Relaxed);
    }
}

/// Switch `s` acted `n` times at once, with `amount` in all.
#[inline]
pub fn acts(s: S, n: u64, amount: u64) {
    if n == 0 && amount == 0 {
        return;
    }
    let c = &COUNTS[s as usize];
    c[0].fetch_add(n, Relaxed);
    c[1].fetch_add(amount, Relaxed);
}

/// Only an amount for switch `s` (its second count).
#[inline]
pub fn amount(s: S, amount: u64) {
    if amount != 0 {
        COUNTS[s as usize][1].fetch_add(amount, Relaxed);
    }
}

/// Switch `s` turned something down, for `why`.
pub fn refuse(s: S, why: &'static str) {
    COUNTS[s as usize][2].fetch_add(1, Relaxed);
    *REASONS.lock().unwrap_or_else(|e| e.into_inner()).entry((s as usize, why)).or_insert(0) += 1;
}

fn shown(unit: &str, per_frame: f64) -> String {
    if unit.contains("bytes") {
        format!("{:.2} MB/f", per_frame / (1 << 20) as f64)
    } else {
        format!("{per_frame:.1}/f")
    }
}

/// With the frame line, over its `frames`: for each switch that is set, what
/// it did per frame since the last line; then the set `VICTORY_*`
/// variables the map has no counter for.
pub(crate) fn line(frames: f64) {
    if frames <= 0.0 {
        return;
    }
    let now: Vec<[u64; 3]> = COUNTS.iter().map(|c| [c[0].load(Relaxed), c[1].load(Relaxed), c[2].load(Relaxed)]).collect();
    let reasons = REASONS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let first = last.is_none();
    let (before, reasons_before) = last.take().unwrap_or_else(|| (vec![[0; 3]; N], BTreeMap::new()));
    let mut parts = Vec::new();
    for (i, &(env, acts, unit)) in NAMES.iter().enumerate() {
        let Ok(value) = std::env::var(env) else { continue };
        let value = value.trim().to_owned();
        let d = [now[i][0] - before[i][0], now[i][1] - before[i][1], now[i][2] - before[i][2]];
        let mut part = format!("{env}={value}: ");
        if now[i] == [0; 3] {
            part.push_str("NEVER ACTED");
        } else {
            part.push_str(&format!("{acts} {:.1}/f", d[0] as f64 / frames));
            if !unit.is_empty() {
                part.push_str(&format!(", {unit} {}", shown(unit, d[1] as f64 / frames)));
            }
            part.push_str(&format!(" (since start {} acts)", now[i][0]));
        }
        if d[2] != 0 {
            let why: Vec<String> = reasons
                .iter()
                .filter(|((s, _), _)| *s == i)
                .map(|(&(s, w), &n)| (w, n - reasons_before.get(&(s, w)).copied().unwrap_or(0)))
                .filter(|&(_, n)| n != 0)
                .map(|(w, n)| format!("{w}: {n}"))
                .collect();
            part.push_str(&format!(", refused {} ({})", d[2], why.join(", ")));
        }
        parts.push(part);
    }
    // The set variables without a counter: on the first line only (the
    // environment does not change).
    let unmapped: Vec<String> = std::env::vars()
        .filter(|_| first)
        .filter(|(k, _)| k.starts_with("VICTORY_") && !NAMES.iter().any(|(env, _, _)| env == k))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    *last = Some((now, reasons));
    drop(last);
    tracing::info!(
        frames = frames as u64,
        "switch map: {}{}",
        if parts.is_empty() { "no mapped switch set".to_owned() } else { parts.join(" | ") },
        if unmapped.is_empty() { String::new() } else { format!(" || set, no counter: {}", unmapped.join(" ")) }
    );
}
