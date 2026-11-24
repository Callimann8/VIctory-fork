# nvidia-testing branch

Test branch on this fork: upstream `main` plus open upstream PRs and own fixes
that help on an NVIDIA RTX 4070 (desktop card). `main` on this fork stays a
clean mirror of upstream (Marvisak/VIctory).

## Base
- upstream main `ffe310e` (2026-11-24)

## Included upstream PRs
| PR | What | Effect measured here |
|----|------|----------------------|
| [#609](https://github.com/Marvisak/VIctory/pull/609) | Retain released textures, staging, shadow buffers and descriptor resources across submissions | With the retention raised from 3 to 2000 submissions (own commit): no more freed-texture page faults (Aftermath named a destroyed 1024x1024 BC7 texture, upstream #557). |
| [#650](https://github.com/Marvisak/VIctory/pull/650) | Bound root view accesses by the size the host binds | Together with #609: 15 minutes in the world without a GPU fault. Its new test adapted to the current `resource::track`. |
| [#490](https://github.com/Marvisak/VIctory/pull/490) | Ask whether guarded memory is a view once per block (includes #464) | World frame time 2,656 -> 2,285 ms (-14%); upload copies 520 -> 142 ms. Hand-merged (watch.rs). |
| [#489](https://github.com/Marvisak/VIctory/pull/489) (+ #302) | Read root arguments the console spills to memory | Together with #953 and #539: dropped draws 1,698 -> 1,627 per frame, untranslatable shaders 446 -> 418; stable. |
| [#953](https://github.com/Marvisak/VIctory/pull/953) | s_buffer_load SMEM uses in descriptor_data | (see #489) |
| [#539](https://github.com/Marvisak/VIctory/pull/539) | 64-bit image atomics | (see #489) |
| [#233](https://github.com/Marvisak/VIctory/pull/233) | Wave64 compute on 32-lane hosts (in halves and in the console flow) | No compute pipeline refused any more on the RTX 4070; much more of the night skyline lights up (some post effects still smear). About 2.8 s per frame (more shader writes to copy back). Merged with #254: the wave size is part of the cache key. |
| [#453](https://github.com/Marvisak/VIctory/pull/453) | Pixel barycentrics through SV_Barycentrics | 35 fewer untranslatable shaders (582 -> 547); stable. |
| [#704](https://github.com/Marvisak/VIctory/pull/704) (+ #193) | Unknown-stride buffers through a raw view | Removes the top unknown-stride compute drop (e30970f1727063fd, about 200 draws/frame). Hand-merged with our raw structured buffers. |
| [#439](https://github.com/Marvisak/VIctory/pull/439) | One console wave sharing LDS runs in the console flow on 32-lane hosts | About 330 more shaders translate (compute 1,249 -> 1,468 emitted in the warm-up); exposed two NVIDIA faults, fixed below (own fixes 5 and 6). Dropped draws 1,607 -> 1,212 per frame. |
| [#254](https://github.com/Marvisak/VIctory/pull/254) | Stable shader cache key (survives rebuilds), in-flight dedup, `victory translate --write-cache` warm-up | Warm-up 85 s, 21,898 of 26,148 shaders cached (606 MB). Next run: 1,553 cache hits, 0 misses; world loading starts at 36 s instead of 47 s. Merge conflicts in `shader_cache.rs` and `pipeline/src/cache.rs` resolved in favour of the PR. |

## Own additions
- **Nsight Aftermath GPU crash dumps** (diagnostics builds only):
  `VICTORY_AFTERMATH=<path to GFSDK_Aftermath_Lib.x64.dll>` loads NVIDIA's
  library at run time (nothing linked or shipped), and
  `VICTORY_AFTERMATH_DIR=<dir>` sets where the `.nv-gpudmp`, its decoded
  `.json`, shader debug info, and each shader's console container and DXIL
  (`shaders/<hash>.*`) go. The log line `aftermath shader hash` maps the
  dump's "Shader hash" to VIctory's shader hash. Open a dump in Nsight
  Graphics with the shader search path set to `<dir>\shaders` to see the
  faulting DXIL line.
- **NVIDIA descriptor-type fixes.** Console buffer descriptors carry no view
  type; D3D12's do, and NVIDIA faults (AMD does not) when a shader reads a
  descriptor as another type:
  1. Constant buffers read through a descriptor loaded at a data-computed
     heap index are read as raw bytes (fixed the first world freeze: compute
     `0e45e59e93b736e4`, "Shader Instruction Error").
  2. Structured buffers are raw on both sides: shaders address them in bytes,
     the host makes structured views raw where they start on 16 bytes
     (fixed "Misaligned Address Error" in compute `8080af82aaac33df` and
     siblings).
  3. Empty constant buffers (size 0 or address 0) read 256 zero bytes
     through a real constant buffer view; a size-0 view or a null view still
     faulted (fixed vertex `8157a974717876a9`, "Shader Instruction Error").

  4. A capped view heap (NVIDIA: 1,000,000) keeps 262,144 dynamic slots
     instead of 65,536: the game's per-draw tables past the cap ran them out
     and about 450 draws a frame were dropped. Draws per frame 1,547 -> 3,568.

  5. Out-of-bounds LDS (group shared) accesses go to a sink dword after the
     allocation, as the console tolerates them (NVIDIA: Out-Of-Range Address
     Error).
  6. Raw buffer accesses ignore the byte offset's low two bits, as the
     console's dword accesses do (NVIDIA: Misaligned Address Error).

## Tested, not included (no effect on the NVIDIA world hang)
#542, #334, #587 (alone, in pairs, with #650), #693, #581, #483, #431
(conflicts with #609). Speed: #436 and #393 (no measurable gain).
#550 is already in main. #595 no longer merges cleanly.

## Warm up the shader cache
The cache key includes the translator, the loop limit and the GPU's wave
sizes, so warm it with the same build and settings the game runs with:
```powershell
cargo build --profile diag -p victory --features diagnostics
$env:VICTORY_SHADER_LOOP_LIMIT = "4096"
target\diag\victory.exe translate --write-cache   # uses game_dir from victory.toml
```

## Known state on RTX 4070
With everything above and `VICTORY_SHADER_LOOP_LIMIT=4096` (diagnostics
build): boots into the world and reaches **player control** (the first
tutorial hint, "L Move / R Move camera", reacts to input). 15 minutes in the
world without a GPU fault at about 0.4-1.5 fps (about 2.3 s per frame in
the world after #490). Much of the scene is still
dark (untranslated shaders drop draws).

Upstream main alone freezes the GPU 25-45 s into the world.

Remaining: pixel shader `1af4a50c7fb40c97` needs the loop limit (it runs long
enough to time out the GPU otherwise); speed; missing draws.

## Tried and not kept (local branches only)
- **#294 (root UAVs)**, branch `draw/294`: merges with 28 conflicts; one
  needed fix is that shader root-view bounds use #294's per-kind caps (CBV
  64 KiB, SRV 4 MiB, UAV 2 MiB). Stable for 6 minutes, but barely more draws
  (about 790 vs 850 dropped per frame) and slower frames (copy-back of root
  UAV ranges on every submission roughly doubles).
- **Indirect rectangle lists**, branch `draw/rect-indirect`: the 256 draws
  per frame that failed with "indirect kind 0: prepare failed" are
  rectangle-list `ExecuteIndirect` draws with one record and no count buffer
  (190 per frame with 240 rectangles, 62 with 1). The fix expands the
  record's vertices like a direct draw and patches the record's vertex count
  and first vertex with `WriteBufferImmediate` around the execution.
  Stable in two 6-minute runs, those drops are gone, and about 650-950 more
  draws per frame run, but frames got slower in both runs (5.8 and 6.8 s
  vs 4.0 s for this branch on the same day, mostly unexplained "other"
  time, GPU time unchanged). Not merged until the slowdown is understood.

## Run it
```powershell
cargo build --profile diag -p victory --features diagnostics
$env:VICTORY_SHADER_LOOP_LIMIT = "4096"
target\diag\victory.exe run "<game dir>\game_scarlett_master.exe" --execute
```
