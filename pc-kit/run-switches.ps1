# Switch sets for the memory-redesign test runs (branch claude/project-thread-usi0eu).
# Usage, in the PowerShell window that then starts the usual launcher:
#   . C:\xcl\run-switches.ps1
#   Use-VictoryRun R1,R2,R3        # stacks the sets in order ("best so far" = the runs that helped)
#   Clear-VictoryRun               # removes every switch this file sets
# Base = the main launcher's RT block, unchanged. The test plan doc is the reference.

$VictoryRuns = [ordered]@{
    R0  = @{ VICTORY_SELF_COPY_SHRINK = '0' }
    R1  = @{ VICTORY_MEMORY_REFRESH_ONCE = '1' }
    R2  = @{ VICTORY_INPLACE = '1'; VICTORY_INPLACE_MB = '1024' }   # RAM cap (Carlos 17:28: no 10 GB free while playing): at most 1 GB of title memory locked in place (default 4096)
    R3  = @{ VICTORY_POOL_COMMIT = '1'; VICTORY_POOL_CHUNK_MB = '64' }   # RAM cap: pool sections of 64 MB (default 256), a section a window lies on is held in RAM whole
    R3b = @{ VICTORY_INPLACE_RUNS = '1' }
    R3c = @{ VICTORY_GUARD_PRIVATE = '1' }
    R3d = @{ VICTORY_GUARD_PRIVATE = 'all' }
    R4  = @{ VICTORY_MEMORY_WRITTEN_FRAMES = '16'; VICTORY_COPY_THREADS = '4'; VICTORY_FETCH_JOIN = '1'; VICTORY_MAP_RUNS = '1' }   # stack on R3c (=1) or R3b, not on R3d
    R4b = @{ VICTORY_WATCH_FIXED = '1' }               # later, own run on R3d only: load the save, walk outside
    R5  = @{ VICTORY_LOOKUP_CACHE = '1'; VICTORY_HEAP_WORKER = '1'; VICTORY_LIGHT_REFRESH = '1'; VICTORY_ABSENT_ONCE = '1'; VICTORY_ABSENT_MS = 'told'; VICTORY_STEP_INDEX = '1'; VICTORY_FILLS_PLAIN = '1' }
    R5b = @{ VICTORY_GUARD_OUTSIDE = '1' }             # fxceless PR #1315: guards made read-only outside the locks (writers' lock waits 5-7 -> 0.1 ms in their bench)
    R6  = @{ VICTORY_FILL_WAIT_PIECES = '1'; VICTORY_MERGE_RUNS = '4096' }
    R6b = @{ VICTORY_SETTLE_ROOM = '1' }               # fxceless 06702fe: lists in the air x2, merge cap x8 (measured -11.5 ms prep there)
    R6c = @{ VICTORY_PARALLEL_REFRESH = '1' }          # fxceless ceb9e1f: texture checks on helper threads (-8.7/-9.4 ms prep there; with SETTLE_ROOM ~-20)
    R7  = @{ VICTORY_QUEUE2 = '1' }
    R8  = @{ VICTORY_CARD_BUFFERS_MIN_KB = '64'; VICTORY_CARD_BUFFERS_MB = '1280'; VICTORY_CARD_SYNC_BATCH = '1' }
    R9  = @{ VICTORY_CARD_HASH_WIDE = '1'; VICTORY_CARD_HASH_LEN = '1' }
    R10 = @{ VICTORY_CODE_TABLE_AFTER = '1' }          # changes the shader cache: own warm-up
    R11 = @{ VICTORY_PIPELINE_ONCE = '1'; VICTORY_TABLE_QUICK_HASH = '1'; VICTORY_GPU_WRITE_ONCE = '1'; VICTORY_UPLOAD_POOLED = '1'; VICTORY_FILL_ONCE = '1' }  # Belobug per-draw CPU savings
    W0  = @{ VICTORY_PREP_WIDE = 'serial' }           # multi-core prep thread: new order, one thread, timed (compared only)
    W1  = @{ VICTORY_PREP_WIDE = '1' }                # multi-core prep: stretches on helper threads; watch committed RAM
    WT  = @{ VICTORY_WIDE_THREADS = '12' }             # yuriko 97876cb9: 12 helper threads instead of 6 (7945HX has 32)
    TL  = @{ VICTORY_THREAD_LIMIT = '1' }              # fxceless PR #1292: builder lanes past the thread count outside EXEC; changes the shader cache: own warm-up
    TB  = @{ VICTORY_TEXTURE_BACK = '1' }              # fxceless PR #999: water height maps copied back so the CPU's boat physics sees them (boats sink without it); correctness
    BUSY = @{ VICTORY_LANDED_TABLES = '1' }            # crowded-area run
    TRACER = @{ VICTORY_TRACER_GUARDS = '1' }          # changes the shader cache: own warm-up
    SWEEP = @{ VICTORY_SWEEP_AT_ROOM = '1' }           # idle texture copies (5 s) let go while new textures find no room (90..100 % gap of SWEEP_PCT=100)
    KEPT = @{ VICTORY_ROOM_KEPT_SECS = '30' }          # room kept from textures for buffer copies/targets dropped once unchanged for 30 s
    ROOM = @{ VICTORY_TEXTURE_ROOM_PERCENT = '94' }    # textures may fill the card to 94 % of its budget instead of 90 (missing textures)
    G2  = @{ VICTORY_LDS_PRUNE = '1' }                 # changes the shader cache: own warm-up (GPU thread)
}

function Use-VictoryRun([string[]]$Names) {
    foreach ($n in $Names) {
        if (-not $VictoryRuns.Contains($n)) { throw "unknown run $n" }
        foreach ($kv in $VictoryRuns[$n].GetEnumerator()) {
            Set-Item -Path "env:$($kv.Key)" -Value $kv.Value
            Write-Host "$n  $($kv.Key)=$($kv.Value)"
        }
    }
}

function Clear-VictoryRun {
    foreach ($set in $VictoryRuns.Values) {
        foreach ($k in $set.Keys) { Remove-Item -Path "env:$k" -ErrorAction SilentlyContinue }
    }
}
