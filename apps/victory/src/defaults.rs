//! The main build's switches, built in.
//!
//! Until 2026-11-27 the launcher (`play-victory-gta6.ps1`) set some fifty
//! `VICTORY_*` variables before it started the game, and a run without that
//! exact list was a different, slower and partly broken build. They are set
//! here instead, at the very start of the process and only where the
//! environment does not already say something else, so:
//!
//! - a plain `victory run ...` is the main build;
//! - a test still overrides any of them by setting the variable itself;
//! - the shader warm-up (`translate --warm`) sees the same values as the
//!   game, so both file translations under the same shader cache key (the
//!   launcher set `VICTORY_SHADER_LOOP_LIMIT` only after its warm-up);
//! - `VICTORY_DEFAULTS=0` leaves everything unset (the code's own defaults).
//!
//! Each value is the one the main launcher set for build 4c05359 (run W1:
//! 110 ms a frame in the lit cell on an RTX 4070 at 1080p with ray tracing).
//! Change a value here only with a measured run behind it.

/// Name, value, what it does.
pub(crate) const MAIN: &[(&str, &str, &str)] = &[
    // Ray tracing (always on).
    ("VICTORY_BVH_BUILDER", "1", "the title's BVH builder programs run"),
    ("VICTORY_FIRSTLANE_OWN", "1", "first-lane ownership in split wave programs (shader cache key)"),
    ("VICTORY_MEMORY_UNIT_CONSTANTS", "0", "RT memory by address: no constant naming"),
    ("VICTORY_MEMORY_NEXT_CELL", "0", "RT memory by address: no next-cell naming"),
    ("VICTORY_MEMORY_READ_MARKS", "1", "RT windows carry read marks (shader cache key)"),
    ("VICTORY_MEMORY_RELEASE_FRAMES", "20", "RT windows nothing reached for 20 frames are let go"),
    ("VICTORY_MEMORY_REFRESH_IDLE", "8", "RT windows no shader read for 8 frames are not refreshed"),
    ("VICTORY_MEMORY_REFRESH_ONCE", "1", "RT windows refreshed once per frame"),
    ("VICTORY_MEMORY_BUDGET_MB", "6144", "RT windows past 6 GiB are let go, oldest first"),
    (
        "VICTORY_WAVE_WIDE",
        "2431bbe7,581dab2c,24126c14,7099a57f,50cf2832,b5b83854,b85d78dd",
        "wave-wide branches for the seven 64-lane programs a 32-lane card refused (shader cache key)",
    ),
    ("VICTORY_SELF_COPY_SHRINK", "1", "the title's c42a self-copy runs as one group (~240 ms of GPU less)"),
    ("VICTORY_SKIP_WC_WINDOWS", "1", "RT shader writes to write-combining windows stay on the card"),
    // Shaders.
    ("VICTORY_SHADER_LOOP_LIMIT", "4096", "translated loops stop after 4096 rounds (one world pixel shader timed the GPU out)"),
    // Frame: submission, waits and copies between title memory and the card.
    ("VICTORY_LATE_WAIT", "3", "late waits, relay mode"),
    ("VICTORY_RELAY", "2", "the relay lands what shaders wrote, exact mode"),
    ("VICTORY_ASYNC_SUBMIT", "1", "submissions go out on the layer's own thread"),
    ("VICTORY_COPY_QUEUE_FETCH", "1", "the relay fetches on a copy queue of its own"),
    ("VICTORY_WIDE_BACK", "1", "copy-back merges on several threads"),
    ("VICTORY_WIDE_REFRESH", "1", "refreshes on several threads"),
    ("VICTORY_PREP_WIDE", "1", "a submission's steps between title writes run on several threads"),
    ("VICTORY_REFRESH_ONCE", "1", "a copy range is refreshed once per submission"),
    ("VICTORY_UNWATCHED_FRAMES", "2", "memory without write watch is copied every 2nd frame in the heap pass"),
    ("VICTORY_FILL_WAIT_PIECES", "1", "a missed fill waits per 64 KiB piece, not for the whole range"),
    ("VICTORY_MERGE_SCATTER", "1", "merges under shader writes go up as one scatter pass per copy"),
    ("VICTORY_MERGE_PAGES", "1", "merges under shader writes by whole pages"),
    ("VICTORY_SYNC_SNAPSHOT", "2", "the scatter pass reads a per-submission snapshot"),
    ("VICTORY_COPY_ON_CARD", "1", "title buffer copies whose source the card wrote are done on the card"),
    ("VICTORY_COPY_PLAIN", "1", "plain copies"),
    ("VICTORY_CBV_DIRECT", "1", "table constant views on the host copy"),
    ("VICTORY_CARD_BUFFERS_MIN_MB", "1", "buffers of 1 MiB and more get a copy in the card's memory"),
    ("VICTORY_CARD_BUFFERS_MB", "2048", "at most 2 GiB of such copies"),
    ("VICTORY_SKIP_WC_BACK", "1", "shader writes to write-combining title memory stay on the card ..."),
    ("VICTORY_SKIP_WC_BACK_MIN_KB", "8192", "... except copies under 8 MiB, which the title's CPU reads (1 MiB: black crowd)"),
    // Indirect draws.
    ("VICTORY_INDIRECT_WINDOW", "1", "indirect arguments read through windows"),
    ("VICTORY_INDIRECT_GATHER", "1", "indirect draw clamps gathered"),
    ("VICTORY_GATHER_SEGMENTS", "1", "draws that miss their window share one clamp pass per stretch"),
    // Textures.
    ("VICTORY_TEXTURE_MIP_DROP", "1", "big sampled textures without their most detailed mip"),
    ("VICTORY_TEXTURE_BUDGET_MB", "5500", "texture copies on the card"),
    ("VICTORY_TEXTURE_SWEEP_PCT", "100", "the texture sweep waits for a full card"),
    ("VICTORY_TEXTURE_SWEEP_FULL", "1", "idle texture copies are let go while the texture budget is full"),
    ("VICTORY_SWEEP_SETTLE_MS", "6000", "the texture sweep settles at most every 6 s"),
    ("VICTORY_TEXTURE_STAGE_MS", "120", "texture pass time per frame"),
    ("VICTORY_TEXTURE_NO_WAIT", "1", "texture uploads do not wait"),
    ("VICTORY_TEXTURE_BACK", "1", "textures shaders write that the title reads on the CPU are copied back"),
    ("VICTORY_HEAP_WORKER", "1", "textures picked by index are uploaded on their own thread"),
    // Picture.
    ("VICTORY_RENDER_SCALE", "0.5", "the title renders 1920x1080 and stretches to 3840x2160 itself"),
    ("VICTORY_FPS_COUNTER", "0", "no frame-rate box in the picture"),
    ("VICTORY_CAPTURE_WORLD_EVERY", "64", "with frame captures on: one world frame in 64"),
];

/// Sets every [`MAIN`] switch the environment leaves unset. Returns the
/// names the environment had already (a test's overrides), or `None` with
/// `VICTORY_DEFAULTS=0`.
///
/// # Safety
/// No other thread of the process exists yet: the environment is written.
pub(crate) unsafe fn apply() -> Option<Vec<&'static str>> {
    if std::env::var_os("VICTORY_DEFAULTS").is_some_and(|v| v == "0") {
        return None;
    }
    let mut given = Vec::new();
    for &(name, value, _) in MAIN {
        if std::env::var_os(name).is_some() {
            given.push(name);
        } else {
            // SAFETY: forwarded from this function's contract.
            unsafe { std::env::set_var(name, value) };
        }
    }
    Some(given)
}

#[cfg(test)]
mod tests {
    use super::MAIN;

    #[test]
    fn each_switch_is_listed_once_with_a_value() {
        let mut names: Vec<&str> = MAIN.iter().map(|s| s.0).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a switch is listed twice");
        assert!(MAIN.iter().all(|s| s.0.starts_with("VICTORY_") && !s.1.is_empty() && !s.2.is_empty()));
    }
}
