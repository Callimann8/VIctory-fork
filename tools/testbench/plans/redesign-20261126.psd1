# Memory-redesign A/B plan for 2026-11-26 evening (branch claude/project-thread-usi0eu).
# Sets = switch sets (read from run-switches.ps1); Runs = what runs, in order.
#   Use     sets to stack, in order; 'best' = the sets of earlier Try runs that helped
#   Try     $true: kept in 'best' if it is more than NoiseMarginPct faster than the stack before it
#           (and its pictures do not FAIL)
#   OnlyIfCrashed  run only if that run did not finish
#   Prompt  wait for Enter before measuring (e.g. walk to the crowded area)
#   Env     extra switches for this run only
@{
    Name = 'redesign'
    Baseline = 'R0'            # 'vs base' column; also where 'best' starts
    PictureReference = 'MAIN'  # pictures of every run are compared with this run's
    Calibrate = @()  # MAIN2 dropped tonight (Carlos 13:06: shorter plan); runner uses its fixed picture threshold

    # The switch sets come from the redesign thread's run-switches.ps1 (read, not run),
    # so this plan follows it when it changes. Sets here would win over the file.
    # Shorter plan tonight (Carlos 2026-11-26 13:06, tonight only): Seconds per run (240 big switches, 120 the rest);
    # stop a run after EarlyStopSeconds of measuring when it crashed or is EarlyStopSlowerPct slower than best;
    # at the end rerun at RerunSeconds every Try run within CloseCallPct of the stack before it.
    EarlyStopSeconds = 60
    EarlyStopSlowerPct = 15
    CloseCallPct = 5
    RerunSeconds = 240
    SetsFile = 'C:\xcl\run-switches.ps1'
    Sets = @{}

    Runs = @(
        @{ Name = 'MAIN'; Seconds = 120;  Build = 'main' }                       # picture reference (main build, launcher as is)
        @{ Name = 'R0'; Seconds = 240;    Build = 'test'; Use = 'R0' }           # this build with main behaviour
        @{ Name = 'R1'; Seconds = 240;    Build = 'test'; Use = 'R1'; Try = $true }
        @{ Name = 'R2'; Seconds = 240;    Build = 'test'; Use = 'best', 'R2'; Try = $true }
        @{ Name = 'R2b'; Seconds = 240;   Build = 'test'; Use = 'best', 'R3'; Env = @{ VICTORY_INPLACE_MB = '0' }; Try = $true; OnlyIfCrashed = 'R2' }   # fallback: sections only
        @{ Name = 'R3'; Seconds = 240;    Build = 'test'; Use = 'best', 'R2', 'R3'; Try = $true }           # POOL_COMMIT only means something with INPLACE
        @{ Name = 'R3b'; Seconds = 240;   Build = 'test'; Use = 'best', 'R2', 'R3', 'R3b'; Try = $true }    # INPLACE_RUNS on top of in-place + pool
        @{ Name = 'R3c'; Seconds = 240;   Build = 'test'; Use = 'best', 'R3c'; Try = $true }
        @{ Name = 'R3d'; Seconds = 240;   Build = 'test'; Use = 'best', 'R3d' }                # compared only, not stacked (R4 must not sit on R3d)
        @{ Name = 'R4'; Seconds = 120;    Build = 'test'; Use = 'best', 'R4'; Try = $true; Prompt = 'Walk out of the cell and back, then check RT shadows and reflections; Enter to measure' }
        @{ Name = 'R4b'; Seconds = 120;   Build = 'test'; Use = 'best', 'R3d', 'R4b'; Prompt = 'Load the test save and walk outside; Enter to measure' }   # compared only: WATCH_FIXED on GUARD_PRIVATE=all
        @{ Name = 'R5'; Seconds = 240;    Build = 'test'; Use = 'best', 'R5'; Try = $true }
        @{ Name = 'R5b'; Seconds = 120; Build = 'test'; Use = 'best', 'R5b'; Try = $true }   # guards re-armed outside the locks (fxceless PR #1315)
        @{ Name = 'R6'; Seconds = 120;    Build = 'test'; Use = 'best', 'R6'; Try = $true }
        @{ Name = 'R6b'; Seconds = 120;   Build = 'test'; Use = 'best', 'R6b'; Try = $true }
        @{ Name = 'R6c'; Seconds = 120; Build = 'test'; Use = 'best', 'R6c'; Try = $true }   # texture checks on helper threads (fxceless ceb9e1f)
        @{ Name = 'R7'; Seconds = 120;    Build = 'test'; Use = 'best', 'R7'; Try = $true }
        @{ Name = 'R8'; Seconds = 120;    Build = 'test'; Use = 'best', 'R8'; Try = $true }
        @{ Name = 'R9'; Seconds = 120;    Build = 'test'; Use = 'best', 'R9'; Try = $true }
        @{ Name = 'R10'; Seconds = 120;   Build = 'test'; Use = 'best', 'R10'; Try = $true }   # own warm-up (shader switch)
        @{ Name = 'R11'; Seconds = 120;   Build = 'test'; Use = 'best', 'R11'; Try = $true }   # Belobug per-draw CPU savings
        @{ Name = 'W0'; Seconds = 120;    Build = 'test'; Use = 'best', 'W0' }               # PREP_WIDE=serial, compared only ("prepared in stretches" estimate)
        @{ Name = 'W1'; Seconds = 240;    Build = 'test'; Use = 'best', 'W1'; Try = $true }  # PREP_WIDE=1 on helper threads; picture must match
        @{ Name = 'WT'; Seconds = 120; Build = 'test'; Use = 'best', 'WT'; Try = $true }   # 12 helper threads (yuriko)
        @{ Name = 'TL'; Seconds = 120; Build = 'test'; Use = 'best', 'TL' }   # RT builder thread limit (fxceless PR #1292), own warm-up; compared only, picture (RT shadows/reflections) must match or improve
        @{ Name = 'TRACER'; Seconds = 120; Build = 'test'; Use = 'best', 'TRACER' }           # own warm-up; the real test (WAVE_WIDE without b5b83854) is set up by hand
        @{ Name = 'G2'; Seconds = 120;    Build = 'test'; Use = 'best', 'G2' }               # LDS_PRUNE, own warm-up; compared only, picture must match (GPU thread)
        @{ Name = 'BUSY'; Seconds = 120;  Build = 'test'; Use = 'best', 'BUSY'; Prompt = 'Walk out of the cell into the crowded area' }
        @{ Name = 'ROOM'; Seconds = 120;  Build = 'test'; Use = 'best', 'ROOM' }   # more card room for textures: fewer missing textures? watch 'texture problems' and the card budget line
        @{ Name = 'SWEEP'; Seconds = 120; Build = 'test'; Use = 'best', 'SWEEP' }  # missing textures: free idle copies when the texture queue has no room
        @{ Name = 'KEPT'; Seconds = 120; Build = 'test'; Use = 'best', 'KEPT' }   # missing textures: kept room given to textures after 30 s unchanged
        @{ Name = 'TEX'; Seconds = 120;   Build = 'test'; Use = 'best'; Env = @{ VICTORY_CARD_ACCOUNTING = '1'; VICTORY_WAIT_MAP = '1'; VICTORY_WAIT_LOG = '1' } }   # + every CPU wait on the card with its last writer (CPU/GPU side by side data)   # missing textures: what waits for room / is not mapped yet (5-s line); not timed against others
    )
}
