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
    Calibrate = @('MAIN', 'MAIN2')  # two runs of the same build: how far apart pictures are by chance

    # The switch sets come from the redesign thread's run-switches.ps1 (read, not run),
    # so this plan follows it when it changes. Sets here would win over the file.
    SetsFile = 'C:\xcl\run-switches.ps1'
    Sets = @{}

    Runs = @(
        @{ Name = 'MAIN';  Build = 'main' }                       # picture reference (main build, launcher as is)
        @{ Name = 'MAIN2'; Build = 'main' }                       # second main run: how far apart pictures are by chance
        @{ Name = 'R0';    Build = 'test'; Use = 'R0' }           # this build with main behaviour
        @{ Name = 'R1';    Build = 'test'; Use = 'R1'; Try = $true }
        @{ Name = 'R2';    Build = 'test'; Use = 'best', 'R2'; Try = $true }
        @{ Name = 'R2b';   Build = 'test'; Use = 'best', 'R3'; Env = @{ VICTORY_INPLACE_MB = '0' }; Try = $true; OnlyIfCrashed = 'R2' }   # fallback: sections only
        @{ Name = 'R3';    Build = 'test'; Use = 'best', 'R2', 'R3'; Try = $true }           # POOL_COMMIT only means something with INPLACE
        @{ Name = 'R3b';   Build = 'test'; Use = 'best', 'R2', 'R3', 'R3b'; Try = $true }    # INPLACE_RUNS on top of in-place + pool
        @{ Name = 'R3c';   Build = 'test'; Use = 'best', 'R3c'; Try = $true }
        @{ Name = 'R3d';   Build = 'test'; Use = 'best', 'R3d' }                # compared only, not stacked (R4 must not sit on R3d)
        @{ Name = 'R4';    Build = 'test'; Use = 'best', 'R4'; Try = $true; Prompt = 'Walk out of the cell and back, then check RT shadows and reflections; Enter to measure' }
        @{ Name = 'R4b';   Build = 'test'; Use = 'best', 'R3d', 'R4b'; Prompt = 'Load the test save and walk outside; Enter to measure' }   # compared only: WATCH_FIXED on GUARD_PRIVATE=all
        @{ Name = 'R5';    Build = 'test'; Use = 'best', 'R5'; Try = $true }
        @{ Name = 'R6';    Build = 'test'; Use = 'best', 'R6'; Try = $true }
        @{ Name = 'R6b';   Build = 'test'; Use = 'best', 'R6b'; Try = $true }
        @{ Name = 'R7';    Build = 'test'; Use = 'best', 'R7'; Try = $true }
        @{ Name = 'R8';    Build = 'test'; Use = 'best', 'R8'; Try = $true }
        @{ Name = 'R9';    Build = 'test'; Use = 'best', 'R9'; Try = $true }
        @{ Name = 'R10';   Build = 'test'; Use = 'best', 'R10'; Try = $true }   # own warm-up (shader switch)
        @{ Name = 'R11';   Build = 'test'; Use = 'best', 'R11'; Try = $true }   # Belobug per-draw CPU savings
        @{ Name = 'W0';    Build = 'test'; Use = 'best', 'W0' }               # PREP_WIDE=serial, compared only ("prepared in stretches" estimate)
        @{ Name = 'W1';    Build = 'test'; Use = 'best', 'W1'; Try = $true }  # PREP_WIDE=1 on helper threads; picture must match
        @{ Name = 'TRACER'; Build = 'test'; Use = 'best', 'TRACER' }           # own warm-up; the real test (WAVE_WIDE without b5b83854) is set up by hand
        @{ Name = 'G2';    Build = 'test'; Use = 'best', 'G2' }               # LDS_PRUNE, own warm-up; compared only, picture must match (GPU thread)
        @{ Name = 'BUSY';  Build = 'test'; Use = 'best', 'BUSY'; Prompt = 'Walk out of the cell into the crowded area' }
        @{ Name = 'ROOM';  Build = 'test'; Use = 'best', 'ROOM' }   # more card room for textures: fewer missing textures? watch 'texture problems' and the card budget line
        @{ Name = 'SWEEP'; Build = 'test'; Use = 'best', 'SWEEP' }  # missing textures: free idle copies when the texture queue has no room
        @{ Name = 'TEX';   Build = 'test'; Use = 'best'; Env = @{ VICTORY_CARD_ACCOUNTING = '1' } }   # missing textures: what waits for room / is not mapped yet (5-s line); not timed against others
    )
}
