# nvidia-testing branch

Test branch on this fork: upstream `main` plus open upstream PRs and own fixes
that help on an NVIDIA RTX 4070 (desktop card). `main` on this fork stays a
clean mirror of upstream (Marvisak/VIctory).

## Base
- upstream main `ffe310e` (2026-11-24)

## Included upstream PRs
| PR | What | Effect measured here |
|----|------|----------------------|
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
  3. Empty constant buffers (size 0) and failed scratch copies get a null
     constant buffer view instead of an invalid one (fixed vertex
     `8157a974717876a9`, "Shader Instruction Error").

## Tested, not included (no effect on the NVIDIA world hang)
#542, #334, #587, #650 (alone, in pairs, all three), #693, #581, #483.
#550 is already in main. #595 no longer merges cleanly.

## Warm up the shader cache
```powershell
cargo build --release -p victory
target\release\victory.exe translate --write-cache   # uses game_dir from victory.toml
```

## Known state on RTX 4070
Boots through all menus into the world. Upstream main freezes the GPU about
25-45 s into the world. With the fixes above, the world runs for about
80-100 s and shows some geometry and lighting. Remaining blockers:
- Pixel shader `1af4a50c7fb40c97` can run long enough to time out the GPU;
  `VICTORY_SHADER_LOOP_LIMIT=4096` (diagnostics) gets past it.
- Then an MMU page fault (read of an unmapped address) in mesh shader
  `c962201cddbdd7dd`, likely a freed resource.
