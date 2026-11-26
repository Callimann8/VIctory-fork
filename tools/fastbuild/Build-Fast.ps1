<#
.SYNOPSIS
  Faster Windows builds of VIctory test builds, with the same result.

.DESCRIPTION
  Builds a worktree with exactly the command, profile and features used by
  hand (default: cargo build --profile diag -p victory --features diagnostics
  --locked), with sccache in front of the compiler:

  * sccache keeps every compiled crate in one cache shared by all worktrees.
    A new worktree, or a branch that only touched a few crates, takes the
    rest from the cache (the windows crate, clap, tracing, zlib-rs and every
    own crate whose source did not change) instead of compiling it again.
    What comes out of the cache is the very file rustc made before for the
    same source and flags, so the executable is the same as without it.
  * Nothing about code generation changes: no incremental release builds, no
    other codegen units, no other linker. The game runs exactly as fast as
    with a normal build.

  Every build's time goes to C:\xcl\runs\builds\build-times.csv (with cache
  hits and misses), so the gain is measured, not guessed. The full cargo
  output goes to a log file; only the tail is shown.

.EXAMPLE
  .\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign
  .\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign -NewWorktree -Branch claude/project-thread-usi0eu
  .\Build-Fast.ps1 -Worktree C:\xcl\victory-redesign -NoCache      # the old way, for comparison
  .\Build-Fast.ps1 -ShowDefenderCommand                             # optional antivirus exclusion (your decision)
#>
[CmdletBinding()]
param(
    [string]$Worktree,
    [switch]$NewWorktree,
    [string]$Branch,
    [string]$From = 'C:\xcl\victory-new',
    [string]$BuildProfile = 'diag',
    [string]$Features = 'diagnostics',
    [string[]]$CargoArgs,
    [switch]$NoCache,
    [string]$CacheDir = 'C:\xcl\sccache',
    [string]$CacheSize = '40G',
    [string]$ToolsDir = 'C:\xcl\tools',
    [string]$LogDir = 'C:\xcl\runs\builds',
    [switch]$ShowDefenderCommand
)
# 'Continue': cargo and git write to their error output, which Windows
# PowerShell 5.1 would turn into a stopping error under 'Stop'.
$ErrorActionPreference = 'Continue'

if ($ShowDefenderCommand) {
    $paths = @("$CacheDir", "$env:USERPROFILE\.cargo", "$env:USERPROFILE\.rustup")
    $paths += @(Get-ChildItem -Path 'C:\xcl' -Directory -ErrorAction SilentlyContinue | Where-Object { Test-Path (Join-Path $_.FullName 'target') } | ForEach-Object { Join-Path $_.FullName 'target' })
    Write-Host "Windows Defender scans every file the compiler and linker write; excluding the build folders often saves a lot of build time."
    Write-Host "It also means those folders are not scanned for malware. Only do this if you are fine with that."
    Write-Host "Run in an administrator PowerShell window:" -ForegroundColor Cyan
    Write-Host ("Add-MpPreference -ExclusionPath " + (($paths | ForEach-Object { "'$_'" }) -join ','))
    Write-Host "To undo: the same line with Remove-MpPreference."
    return
}
if (-not $Worktree) { throw "-Worktree is needed (the folder of the VIctory checkout to build)" }

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log = Join-Path $LogDir ("build-{0}-{1}.log" -f (Split-Path $Worktree -Leaf), $stamp)

function Get-Sccache {
    $onPath = Get-Command sccache -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $local = Join-Path $ToolsDir 'sccache\sccache.exe'
    if (Test-Path $local) { return $local }
    # The official release (Mozilla), once.
    $ver = 'v0.10.0'
    $url = "https://github.com/mozilla/sccache/releases/download/$ver/sccache-$ver-x86_64-pc-windows-msvc.zip"
    Write-Host "getting sccache $ver from github.com/mozilla/sccache (once)..."
    $zip = Join-Path $env:TEMP "sccache-$ver.zip"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing -ErrorAction Stop
        $tmp = Join-Path $env:TEMP "sccache-$ver"
        Expand-Archive -Path $zip -DestinationPath $tmp -Force -ErrorAction Stop
        $exe = Get-ChildItem -Path $tmp -Recurse -Filter sccache.exe | Select-Object -First 1
        New-Item -ItemType Directory -Force -Path (Split-Path $local) | Out-Null
        Copy-Item $exe.FullName $local -Force
        return $local
    } catch {
        Write-Host "download failed ($($_.Exception.Message)); trying 'cargo install sccache --locked' (takes a few minutes, once)" -ForegroundColor Yellow
        & cargo install sccache --locked 2>&1 | Out-File (Join-Path $LogDir "sccache-install-$stamp.log")
        $c = Get-Command sccache -ErrorAction SilentlyContinue
        if ($c) { return $c.Source }
        return $null
    }
}

if ($NewWorktree) {
    if (-not $Branch) { throw "-NewWorktree needs -Branch" }
    if (Test-Path $Worktree) { throw "$Worktree exists already" }
    Write-Host "new worktree $Worktree for $Branch (from $From)"
    & git -C $From fetch origin $Branch 2>&1 | Out-File $log -Append
    & git -C $From worktree add $Worktree "origin/$Branch" 2>&1 | Out-File $log -Append
    & git -C $Worktree checkout -B $Branch "origin/$Branch" 2>&1 | Out-File $log -Append
    if (-not (Test-Path (Join-Path $Worktree 'Cargo.toml'))) { throw "worktree not made; see $log" }
}

$cache = $null
if (-not $NoCache) {
    $cache = Get-Sccache
    if ($cache) {
        $env:RUSTC_WRAPPER = $cache
        $env:SCCACHE_DIR = $CacheDir
        $env:SCCACHE_CACHE_SIZE = $CacheSize
        & $cache --stop-server 2>&1 | Out-Null
        & $cache --start-server 2>&1 | Out-Null
        & $cache --zero-stats 2>&1 | Out-Null
    } else {
        Write-Host "no sccache; building without the cache" -ForegroundColor Yellow
    }
} else {
    Remove-Item env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
}
if ($env:CARGO_INCREMENTAL -eq '1') { Write-Host "note: CARGO_INCREMENTAL=1 is set in this window (it changes code generation and turns the cache off for those crates)" -ForegroundColor Yellow }

$args2 = if ($CargoArgs) { $CargoArgs } else { @('build', '--profile', $BuildProfile, '-p', 'victory', '--features', $Features, '--locked') }
$commit = (& git -C $Worktree rev-parse --short HEAD 2>$null)
Write-Host ("building {0} ({1}) with: cargo {2}{3}" -f $Worktree, $commit, ($args2 -join ' '), $(if ($cache) { '  [sccache]' } else { '' })) -ForegroundColor Cyan
Write-Host "full output: $log"
$t0 = Get-Date
Push-Location $Worktree
try {
    & cargo @args2 2>&1 | ForEach-Object { "$_" -replace '^System\.Management\.Automation\.RemoteException$', '' } | Out-File $log -Append -Encoding UTF8
    $rc = $LASTEXITCODE
} finally { Pop-Location }
$secs = [int]((Get-Date) - $t0).TotalSeconds

$hits = ''; $misses = ''
if ($cache) {
    $stats = & $cache --show-stats 2>&1 | Out-String
    $stats | Out-File $log -Append
    if ($stats -match 'Cache hits\s+(\d+)') { $hits = $matches[1] }
    if ($stats -match 'Cache misses\s+(\d+)') { $misses = $matches[1] }
}
Get-Content $log -Tail 6 | Write-Host
$csv = Join-Path $LogDir 'build-times.csv'
if (-not (Test-Path $csv)) { 'date,worktree,commit,cache,seconds,cache_hits,cache_misses,result' | Set-Content $csv }
('{0},{1},{2},{3},{4},{5},{6},{7}' -f (Get-Date -Format 's'), $Worktree, $commit, $(if ($cache) { 'sccache' } else { 'none' }), $secs, $hits, $misses, $(if ($rc -eq 0) { 'ok' } else { "failed $rc" })) | Add-Content $csv
$exe = Join-Path $Worktree ("target\{0}\victory.exe" -f $(if ($BuildProfile -eq 'dev') { 'debug' } else { $BuildProfile }))
if ($rc -eq 0) {
    Write-Host ("built in {0} min {1} s{2}: {3}" -f [int][Math]::Floor($secs / 60), ($secs % 60), $(if ($cache) { " (cache: $hits hits, $misses compiled)" } else { '' }), $exe) -ForegroundColor Green
    Write-Host "Next: the shader cache warm-up for this build (the test runner does it by itself)."
} else {
    Write-Host "build FAILED after $secs s (exit $rc); see $log" -ForegroundColor Red
    exit $rc
}
