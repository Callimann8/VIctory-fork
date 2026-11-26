# Test bench settings for Carlos's PC. Paths may use %VARIABLES%.
# A plan file can override any of these for its own runs.
@{
    # Where each plan run's folder goes (logs, pictures, results).
    RunsRoot = 'C:\xcl\runs\testbench'

    # The builds a plan can use, by name. Each is started through its own
    # launcher, exactly as by hand; the runner only adds the run's switches
    # (and its own data folder and picture folder) right before the game starts.
    Builds = @{
        main = @{ Launcher = 'C:\xcl\play-victory-gta6.ps1' }
        # The redesign test build: a copy of the main launcher pointing at the
        # test worktree and its own shader cache folder.
        test = @{ Launcher = 'C:\xcl\play-victory-test.ps1' }
        # Optional per build: Args = @('...') for the game start,
        # WarmupArgs = @('-Warmup') for the warm-up (that is the default).
    }

    # The test save, copied fresh for every run. Either a data folder
    # (holding <title id>\storage) or a storage folder on its own.
    SaveSource = 'C:\xcl\victory-data\save-backup-v54-testrun'

    # Data folders holding the real save. Never used by a run; checked after
    # every run, and the plan stops if anything in them changed.
    RealDataDirs = @('C:\xcl\victory-data\data-new', 'C:\xcl\data-new', '%LOCALAPPDATA%\victory\data')

    # Where the launcher's own log may be, if it does not print to its window.
    LogSearch = @('C:\xcl\runs')

    RunSeconds = 240          # measured time per run, from the first world frame
    SkipSeconds = 60          # frame-log reports of the first minute are left out
    StartTimeoutSeconds = 900 # a run whose world does not start by then is stopped
    NoiseMarginPct = 3        # results closer than this count as "close"

    # Pictures: 'engine' = the build's own capture (diagnostics builds),
    # 'window' = screenshots of the game window, 'off'.
    Pictures = 'engine'
    PictureEvery = 64         # a picture every 64th world frame (each costs ~0.25 s)

    # Marked noisy when, averaged over a run, other programs use more than this.
    NoisyLimits = @{ VideoPct = 1.0; GpuPct = 4.0; CpuPct = 5.0 }

    # Switches the shader translator reads: a run that sets one gets its own warm-up.
    # (every VICTORY_* the translator in crates/victory-shader reads, as of usi0eu 7f5a49c, minus debug-only ones)
    ShaderSwitches = @('VICTORY_CODE_TABLE_AFTER', 'VICTORY_TRACER_GUARDS', 'VICTORY_LDS_PRUNE', 'VICTORY_THREAD_LIMIT',
        'VICTORY_WAVE_WIDE', 'VICTORY_WAVE_WIDE_GROUP', 'VICTORY_WAVE_LOCAL', 'VICTORY_WAVE_OWN_BITS', 'VICTORY_WAVE_DEEP_SKIPS',
        'VICTORY_MEMORY_MARKS', 'VICTORY_MEMORY_READ_MARKS', 'VICTORY_GROUP_SYNC', 'VICTORY_SHADER_OPT', 'VICTORY_SHADER_LOOP_LIMIT',
        'VICTORY_LDS_BORROW', 'VICTORY_FIRSTLANE_OWN', 'VICTORY_FIRST_HALF', 'VICTORY_F16_NEAREST', 'VICTORY_STRICT_JOINS',
        'VICTORY_MESH_OUTPUT_STRICT', 'VICTORY_BVH_BUILDER', 'VICTORY_BVH_GUARD_SPLIT', 'VICTORY_BVH_NO_HIT')

    # Key presses to get from the title screen into the save, if a run needs
    # them (SendKeys syntax), e.g. @(@{ AfterSeconds = 90; Keys = '{ENTER}' }).
    StartKeys = @()

    # Where crash dumps may appear (Windows Error Reporting LocalDumps for victory.exe
    # writes to C:\xcl\runs\dumps); files written since a run began go into its crash details.
    CrashDirs = @('C:\xcl\runs\dumps', 'C:\xcl\victory-data', '%LOCALAPPDATA%\CrashDumps', '%LOCALAPPDATA%\victory')

    # Keep each run's save copy after the run (off: only pictures and logs stay).
    KeepRunData = $false
}
