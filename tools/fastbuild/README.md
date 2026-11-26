# Faster test builds

`Build-Fast.ps1` builds a VIctory worktree with the usual command
(`cargo build --profile diag -p victory --features diagnostics --locked`) and puts
**sccache** in front of the compiler. sccache is one compiler cache shared by every worktree.

- A new worktree, or a branch that changed only a few crates, takes every unchanged crate from the
  cache instead of compiling it again: the `windows` crate, clap, tracing, zlib-rs, and the own crates
  whose source did not change. What comes out is the very file rustc made before for the same source and
  flags, so the executable is the same.
- Code generation does not change: no incremental release builds, no other codegen units, no other
  linker. The game runs exactly as fast as with a normal build.
- Every build's time, cache hits and misses go to `C:\xcl\runs\builds\build-times.csv`, so the gain is measured.

```powershell
cd C:\xcl\fastbuild    # this folder, copied to the PC
.\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign                         # build with the cache
.\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign -NewWorktree -Branch claude/project-thread-usi0eu
.\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign -NoCache                # the old way, to compare
.\Build-Fast.ps1 -ShowDefenderCommand                                      # optional, see below
```

The first build with the cache is as slow as before (it fills the cache). The gain shows from the
second worktree or branch on. sccache (official Mozilla release, v0.10.0) is downloaded once to
`C:\xcl\tools\sccache`. Cache: `C:\xcl\sccache`, at most 40 GB.

## Shader warm-up

The shader cache is keyed by the translator's own source and the switches it reads, not by the
executable. A rebuild that does not touch `crates/victory-shader` keeps every translation, and the
warm-up then only checks the cache. Builds may share one shader cache folder safely, because different
translators file their translations under different keys. When a branch changes the translator (like
the redesign branch), the first warm-up translates everything once.

## Optional: antivirus exclusion

Windows Defender scans every file the compiler and linker write, which often costs a lot of build
time. `-ShowDefenderCommand` prints the exclusion line for the build folders (run it in an
administrator window). Those folders are then not scanned for malware, so this is your decision.
