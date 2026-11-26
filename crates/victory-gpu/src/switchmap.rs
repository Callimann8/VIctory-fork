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
    SelfCopyShrink = "VICTORY_SELF_COPY_SHRINK", "dispatches shrunk to one group", "";
    RefreshOnce = "VICTORY_MEMORY_REFRESH_ONCE", "refreshes", "copies skipped";
    Inplace = "VICTORY_INPLACE", "windows placed", "bytes";
    PoolCommit = "VICTORY_POOL_COMMIT", "windows placed on sections", "bytes";
    InplaceRuns = "VICTORY_INPLACE_RUNS", "windows cut to a run", "bytes";
    GuardPrivate = "VICTORY_GUARD_PRIVATE", "blocks taken", "faults";
    WrittenFrames = "VICTORY_MEMORY_WRITTEN_FRAMES", "pages kept marked", "pages dropped";
    CopyThreads = "VICTORY_COPY_THREADS", "copies split", "bytes";
    FetchJoin = "VICTORY_FETCH_JOIN", "fetch copies", "pieces joined";
    MapRuns = "VICTORY_MAP_RUNS", "remaps in runs", "calls saved";
    WatchFixed = "VICTORY_WATCH_FIXED", "reservations watched", "bytes";
    LookupCache = "VICTORY_LOOKUP_CACHE", "lookups answered", "lookups missed";
    HeapWorker = "VICTORY_HEAP_WORKER", "texture passes on the worker", "worker microseconds";
    LightRefresh = "VICTORY_LIGHT_REFRESH", "askings answered by bits", "askings to the guards";
    AbsentOnce = "VICTORY_ABSENT_ONCE", "blocks answered from their note", "questions to Windows";
    StepIndex = "VICTORY_STEP_INDEX", "submissions indexed", "";
    FillsPlain = "VICTORY_FILLS_PLAIN", "fills copied plainly", "bytes";
    FillWaitPieces = "VICTORY_FILL_WAIT_PIECES", "early fills noted instead of settling", "";
    MergeRuns = "VICTORY_MERGE_RUNS", "merges under writes", "runs merged";
    SettleRoom = "VICTORY_SETTLE_ROOM", "submissions past the old caps", "";
    Queue2 = "VICTORY_QUEUE2", "jobs on the second queue", "bytes";
    CardBuffersMinKb = "VICTORY_CARD_BUFFERS_MIN_KB", "buffers put on the card", "bytes";
    CardBuffersMb = "VICTORY_CARD_BUFFERS_MB", "buffers kept on the card", "bytes";
    CardSyncBatch = "VICTORY_CARD_SYNC_BATCH", "batched sync lists", "copies hashed together";
    CardHashWide = "VICTORY_CARD_HASH_WIDE", "compare passes", "bytes compared";
    CardHashLen = "VICTORY_CARD_HASH_LEN", "hashes cut to length", "bytes not hashed";
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
    ReleaseFrames = "VICTORY_MEMORY_RELEASE_FRAMES", "idle windows let go", "";
    RefreshIdle = "VICTORY_MEMORY_REFRESH_IDLE", "refreshes of idle windows left out", "";
    MemoryBudget = "VICTORY_MEMORY_BUDGET_MB", "windows let go over the budget", "bytes";
    PrepWide = "VICTORY_PREP_WIDE", "stretches cut into pieces", "pieces";
    SweepAtRoom = "VICTORY_SWEEP_AT_ROOM", "idle texture copies let go for room", "bytes";
    RoomKeptSecs = "VICTORY_ROOM_KEPT_SECS", "room questions answered without kept room", "";
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
        // Set, or on by default and counting.
        let value = match std::env::var(env) {
            Ok(v) => v.trim().to_owned(),
            Err(_) if now[i] != [0; 3] => "(default)".to_owned(),
            Err(_) => continue,
        };
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

/// With the switch map: why textures may be missing or late, per frame
/// since the last line ("texture problems: ..."). Passes that found the
/// card without room for new host copies and the slots they refused (fxceless
/// run 295: black surfaces were exactly textures without room on the card),
/// slots still waiting now, host copies the sweep let go and made again,
/// host textures the card refused to make, uploads skipped for a format the
/// upload does not know or an XOR mode (with the formats), levels left
/// without tiles, mips dropped, and the card's memory now. `VICTORY_CARD_ACCOUNTING=1`
/// adds the five-second line of what the waiting textures need and how much
/// of their memory the title has mapped yet (streaming).
#[cfg(windows)]
pub(crate) fn texture_line(frames: f64) {
    if frames <= 0.0 {
        return;
    }
    let k = &crate::resource::KINDS;
    let now = [
        crate::draw::HEAP_TEXTURES_FULL.load(Relaxed),
        crate::draw::HEAP_TEXTURES_REFUSED_ROOM.load(Relaxed),
        k[2].load(Relaxed),
        k[4].load(Relaxed),
        crate::resource::NO_HOST_TEXTURE.load(Relaxed),
        crate::resource::NOT_UPLOADED[0].load(Relaxed),
        crate::resource::NOT_UPLOADED[1].load(Relaxed),
        crate::tiles::STATS[9].load(Relaxed),
    ];
    static BEFORE: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    let d: Vec<f64> = (0..8).map(|i| (now[i] - BEFORE[i].swap(now[i], Relaxed)) as f64 / frames).collect();
    let formats: Vec<String> = crate::resource::NOT_UPLOADED_FORMATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|&(f, why)| format!("f{f}{}", if why == 1 { " xor" } else { "" }))
        .collect();
    let card = crate::host::host().and_then(|h| h.video_memory()).map_or(String::new(), |v| {
        format!(
            "; the card {} MB in use of {} MB budget, room kept from textures {} MB for buffer copies and {} MB for targets (VICTORY_ROOM_KEPT_SECS drops it)",
            v[1] >> 20,
            v[0] >> 20,
            crate::shadow::card_room_kept() >> 20,
            crate::resource::targets_room_kept() >> 20
        )
    });
    tracing::info!(
        frames = frames as u64,
        "texture problems: card full for new textures {:.1} passes/f, {:.1} slots refused/f (since start {}), {} slots waiting now; \
         sweep let go {:.1}/f, made again {:.1}/f; card refused to make {:.2}/f (since start {}); upload attempts skipped: \
         {:.1}/f unknown layout, {:.1}/f XOR mode (since start {} / {}{}); levels left without tiles {:.1}/f; mips dropped on {} textures{}",
        d[0],
        d[1],
        now[1],
        crate::draw::HEAP_TEXTURES_WAITING.load(Relaxed),
        d[2],
        d[3],
        d[4],
        now[4],
        d[5],
        d[6],
        now[5],
        now[6],
        if formats.is_empty() { String::new() } else { format!("; formats {}", formats.join(" ")) },
        d[7],
        crate::resource::MIP_DROPPED[0].load(Relaxed),
        card,
    );
}
