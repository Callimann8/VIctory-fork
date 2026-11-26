# Test bench: one-click A/B runs with a picture check

Runs a whole test plan on Carlos's PC by itself and writes a results table.

| File | What it does |
|---|---|
| `Run-TestPlan.ps1` | The runner: warm-up, a fresh save copy per run, each run with its switches, frame-log numbers, picture check, results table |
| `testbench.config.psd1` | PC paths: launchers, test save, real save folders, run length |
| `plans/redesign-20261126.psd1` | Tonight's run list (runs in order; switch sets read from `C:\xcl\run-switches.ps1` via `SetsFile`) |
| `piccheck/` | The picture check (small Rust tool, built automatically on first use) |
| `Mark-Noisy.cmd` | Double-click during a run to mark it noisy (you used the PC) |

## Set up once

1. Copy this folder to `C:\xcl\testbench`.
2. Check the paths in `testbench.config.psd1`. The `test` build needs its own launcher: a copy of
   `play-victory-gta6.ps1` named `play-victory-test.ps1` that points at the test worktree and its shader cache folder.
3. Run `.\Run-TestPlan.ps1 -Check`. It starts nothing. It shows whether each launcher was found, where the
   runner will put the run's switches, what the launcher sets itself, the save copy, and every run's switches.

## Run

```powershell
cd C:\xcl\testbench
powershell -ExecutionPolicy Bypass -File .\Run-TestPlan.ps1                  # the whole plan
powershell -ExecutionPolicy Bypass -File .\Run-TestPlan.ps1 -Only MAIN,R0,R1   # some runs
powershell -ExecutionPolicy Bypass -File .\Run-TestPlan.ps1 -Noisy             # you will use the PC the whole time
powershell -ExecutionPolicy Bypass -File .\Run-TestPlan.ps1 -Resume C:\xcl\runs\testbench\redesign-20261126-1830   # continue after a stop
```

Results go to `C:\xcl\runs\testbench\<plan>-<date>-<time>\`:
- `results.md` / `results.csv`: one row per run, with frame ms, fps, change against the baseline,
  CPU preparing, GPU (frame minus card idle), copy-back, lowest free memory, long frames, picture verdict and noisy mark.
- `<run>\`: `err.log`/`out.log` (the game's log), `env.txt` (exact switches), `frames\` (pictures),
  `pictures\report.html` (side by side with main and a red difference map), `pc-use.csv`.
- `results.md` also has a **Run details** section per run (also in `<run>\details.md`):
  - texture problems (last report; flagged when slots are refused or uploads skipped) and the last 5 `card accounting:` lines (all in `card-accounting.txt`);
  - the switch map: what each switch did per frame in the last 3 `switch map:` lines of the measured stretch,
    with NEVER ACTED and refusals flagged, and switches the map does not name;
  - phase timings (medians per frame) from the frame line, plus every `prep split: label N ms | ...` line
    (more such lines: `PhaseLines` in the config);
  - processor use per core and for all cores, the game's processor time per frame;
  - peaks of working set, commit, and dedicated and shared graphics memory, and the lowest free RAM;
  - on a crash: the exception line, dump files (`*.dmp`, `crash-*.bin`, named in the log or written since the
    run began), the game's `crashes.log`, and `crash.txt` with the last 40 log lines. The run's save copy is kept.
- `<run>\cpu-cores.csv`: per-core samples every 5 s.
- `testbench.log`: everything the runner did.

Ctrl+C stops the plan; the game is closed and `-Resume` goes on from the next run.

## What each run does

1. Warm-up: once per build, and again for a run that sets a switch the shader translator reads
   (`ShaderSwitches` in the config). The warm-up only translates what is not in the cache yet.
2. Copies the test save (`SaveSource`) into the run's own folder and points `VICTORY_DATA_DIR` there.
3. Starts a temporary copy of the build's launcher (next to the original, deleted after). The copy sets
   the run's switches right after the launcher's `param` block and again right before the line that
   starts the game. The launcher itself is not changed.
4. Waits for the game world (the first frame of 500 draws or more), measures `RunSeconds` (240),
   then closes the game. Numbers are medians of the frame-log reports after the first `SkipSeconds` (60).
5. Compares its pictures with the main build's (`PictureReference`).

**Save safety:** the game prints which save folder it uses as it starts. If that is not the run's copy,
the runner closes the game at once and stops the plan, before the game world loads. After every run it
also checks that no file of the real save (`RealDataDirs`) changed.

## The plan file

```powershell
@{ Name = 'redesign'; Baseline = 'R0'; PictureReference = 'MAIN'; Calibrate = @('MAIN','MAIN2')
   Sets = @{ R1 = @{ VICTORY_MEMORY_REFRESH_ONCE = '1' }; ... }
   Runs = @(
     @{ Name = 'MAIN'; Build = 'main' }
     @{ Name = 'R1'; Build = 'test'; Use = 'R1'; Try = $true }
     @{ Name = 'R2'; Build = 'test'; Use = 'best','R2'; Try = $true }
     @{ Name = 'R2b'; Build = 'test'; Use = 'best','R2b'; Try = $true; OnlyIfCrashed = 'R2' }
     @{ Name = 'BUSY'; Build = 'test'; Use = 'best','BUSY'; Prompt = 'Walk to the crowded area' } ) }
```

- `Use`: sets stacked in order. `best` = the sets of earlier `Try` runs that helped.
- `Try = $true`: the run is kept in `best` if it is more than `NoiseMarginPct` (3%) faster than the
  stack it was built on, finished, and its pictures did not FAIL.
- `OnlyIfCrashed`: a fallback that runs only if the named run did not finish.
- `Prompt`: beeps when the world has started and waits for Enter before measuring.
- `Env`: extra switches for one run.
- `SetsFile`: a `run-switches.ps1` whose `$VictoryRuns` sets are read (not run); `Sets` in the plan win over it.

## Noisy runs (using the PC during a run)

A run is marked noisy when, averaged over the run:
- the video decoder is in use (YouTube, Prime Video), or
- other programs use more than 4% of the graphics card or 5% of the processor, or
- `Mark-Noisy.cmd` was double-clicked during it, or the plan was started with `-Noisy`.

Noisy runs within twice the margin (6%) of the run they are compared with are run again at the end,
once. With `-Noisy` the plan instead lists the command to rerun them later on a quiet PC.

## Picture check

Each run saves the game's own frames (`VICTORY_CAPTURE`, diagnostics builds) every 64th frame after
the world starts. Each costs about a quarter of a second, the same in every run.
`piccheck` pairs every test picture with the most similar main picture near the same point. Two runs
never show quite the same frame, so it looks for changes across the whole run:

- **less fine detail** (lower resolution, blurrier textures, lost mip levels),
- **light or colour shift** (missing bounce light, shadows, a colour change),
- **more almost-black area**, and **regions that differ** (a missing lamp, a black or broken patch).

Verdicts: PASS, CHECK (look at the report), FAIL (a visual change). With two main runs in the plan
(`Calibrate`), the limits are set from how far apart two runs of the same build are, and every run is
compared again at the end.

On its own:

```powershell
piccheck compare --ref <main frames folder> --test <test frames folder> --out <report folder> [--noise picture-noise.txt]
piccheck calibrate --ref <main run 1 frames> --test <main run 2 frames> --out picture-noise.txt
```

For a build without diagnostics (no engine pictures), set `Pictures = 'window'` for that build in the config.
The runner then screenshots the game window every 20 s, but only while the game window is in front.

## Limits

- The game must go from start into the save by itself, or with `StartKeys` (key presses sent to the game window).
- Watch the first run's first few minutes once, to see the world start and the save line detected with the real launcher.
