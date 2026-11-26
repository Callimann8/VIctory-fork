# Memory-redesign A/B plan for 2026-11-26 evening (branch claude/project-thread-usi0eu).
# Sets = switch sets (from run-switches.ps1); Runs = what runs, in order.
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

    Sets = @{
        R0  = @{ VICTORY_SELF_COPY_SHRINK = '0' }
        R1  = @{ VICTORY_MEMORY_REFRESH_ONCE = '1' }
        R2  = @{ VICTORY_INPLACE = '1' }
        R2b = @{ VICTORY_INPLACE_MB = '0'; VICTORY_POOL_COMMIT = '1' }
        R3  = @{ VICTORY_POOL_COMMIT = '1' }
        R3b = @{ VICTORY_INPLACE_RUNS = '1' }
        R3c = @{ VICTORY_GUARD_PRIVATE = '1' }
        R3d = @{ VICTORY_GUARD_PRIVATE = 'all' }
        R4  = @{ VICTORY_MEMORY_WRITTEN_FRAMES = '16'; VICTORY_COPY_THREADS = '4'; VICTORY_WATCH_FIXED = '1'; VICTORY_FETCH_JOIN = '1'; VICTORY_MAP_RUNS = '1' }
        R5  = @{ VICTORY_LOOKUP_CACHE = '1'; VICTORY_HEAP_WORKER = '1'; VICTORY_LIGHT_REFRESH = '1'; VICTORY_ABSENT_ONCE = '1'; VICTORY_ABSENT_MS = 'told'; VICTORY_STEP_INDEX = '1'; VICTORY_FILLS_PLAIN = '1' }
        R6  = @{ VICTORY_FILL_WAIT_PIECES = '1'; VICTORY_MERGE_RUNS = '4096' }
        R7  = @{ VICTORY_QUEUE2 = '1' }
        R8  = @{ VICTORY_CARD_BUFFERS_MIN_KB = '64'; VICTORY_CARD_BUFFERS_MB = '1280'; VICTORY_CARD_SYNC_BATCH = '1' }
        R9  = @{ VICTORY_CARD_HASH_WIDE = '1'; VICTORY_CARD_HASH_LEN = '1' }
        R10 = @{ VICTORY_CODE_TABLE_AFTER = '1' }
        BUSY = @{ VICTORY_LANDED_TABLES = '1' }
        TRACER = @{ VICTORY_TRACER_GUARDS = '1' }
    }

    Runs = @(
        @{ Name = 'MAIN';  Build = 'main' }                       # picture reference (main build, launcher as is)
        @{ Name = 'MAIN2'; Build = 'main' }                       # second main run: how far apart pictures are by chance
        @{ Name = 'R0';    Build = 'test'; Use = 'R0' }           # this build with main behaviour
        @{ Name = 'R1';    Build = 'test'; Use = 'R1'; Try = $true }
        @{ Name = 'R2';    Build = 'test'; Use = 'best', 'R2'; Try = $true }
        @{ Name = 'R2b';   Build = 'test'; Use = 'best', 'R2b'; Try = $true; OnlyIfCrashed = 'R2' }
        @{ Name = 'R3';    Build = 'test'; Use = 'best', 'R3'; Try = $true }
        @{ Name = 'R3b';   Build = 'test'; Use = 'best', 'R3b'; Try = $true }
        @{ Name = 'R3c';   Build = 'test'; Use = 'best', 'R3c'; Try = $true }
        @{ Name = 'R4';    Build = 'test'; Use = 'best', 'R4'; Try = $true }
        @{ Name = 'R5';    Build = 'test'; Use = 'best', 'R5'; Try = $true }
        @{ Name = 'R6';    Build = 'test'; Use = 'best', 'R6'; Try = $true }
        @{ Name = 'R7';    Build = 'test'; Use = 'best', 'R7'; Try = $true }
        @{ Name = 'R8';    Build = 'test'; Use = 'best', 'R8'; Try = $true }
        @{ Name = 'R9';    Build = 'test'; Use = 'best', 'R9'; Try = $true }
        @{ Name = 'R10';   Build = 'test'; Use = 'best', 'R10'; Try = $true }   # own warm-up (shader switch)
        @{ Name = 'BUSY';  Build = 'test'; Use = 'best', 'BUSY'; Prompt = 'Walk out of the cell into the crowded area' }
    )
}
