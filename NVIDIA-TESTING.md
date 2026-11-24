# nvidia-testing branch

Test branch on this fork: upstream `main` plus open upstream PRs that help on
an NVIDIA RTX 4070 (desktop card). `main` on this fork stays a clean mirror of
upstream (Marvisak/VIctory).

## Base
- upstream main `ffe310e` (2026-11-24)

## Included upstream PRs
| PR | What | Effect measured here |
|----|------|----------------------|
| [#254](https://github.com/Marvisak/VIctory/pull/254) | Stable shader cache key (survives rebuilds), in-flight dedup, `victory translate --write-cache` warm-up | Warm-up 85 s, 21,898 of 26,148 shaders cached (606 MB). Next run: 1,553 cache hits, 0 misses; world loading starts at 36 s instead of 47 s. Merge conflicts in `shader_cache.rs` and `pipeline/src/cache.rs` resolved in favour of the PR. |

## Tested, not included (no effect on the NVIDIA world hang)
#542, #334, #587, #650 (alone, in pairs, all three), #693. #595 no longer
merges cleanly.

## Warm up the shader cache
```powershell
cargo build --release -p victory
target\release\victory.exe translate --write-cache   # uses game_dir from victory.toml
```

## Known state on RTX 4070
Boots through all menus into the world (title card), then the GPU freezes
about 25-45 s in (see upstream #71, #97).
