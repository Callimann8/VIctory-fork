# Starts the main GTA 6 launcher with a NEW, EMPTY data folder, so the game begins from the very
# start (first cinematic). Carlos's real save and the test-save copies are never read, changed or deleted.
#   powershell -ExecutionPolicy Bypass -File C:\xcl\play-fresh.ps1           # play from the start
#   powershell -ExecutionPolicy Bypass -File C:\xcl\play-fresh.ps1 -Reset    # empty the fresh folder first (start over again)
# The fresh game's own saves stay in C:\xcl\victory-data-fresh. Not while a test run uses the PC.
param(
    [switch]$Reset,
    [string]$Launcher = 'C:\xcl\play-victory-gta6.ps1',
    [string]$Fresh = 'C:\xcl\victory-data-fresh',
    [Parameter(ValueFromRemainingArguments = $true)] $Rest
)
$ErrorActionPreference = 'Stop'

# Never point at a folder that holds the real save or the test saves.
$protected = @('C:\xcl\data-new', 'C:\xcl\victory-data', (Join-Path $env:LOCALAPPDATA 'victory\data')) |
    ForEach-Object { [IO.Path]::GetFullPath($_).TrimEnd('\') }
$freshFull = [IO.Path]::GetFullPath($Fresh).TrimEnd('\')
foreach ($p in $protected) {
    if ($freshFull -eq $p -or $freshFull.StartsWith("$p\") -or $p.StartsWith("$freshFull\")) {
        throw "Refusing: $freshFull overlaps a save folder ($p)."
    }
}
if (-not (Test-Path $Launcher)) { throw "Launcher not found: $Launcher" }

if ($Reset -and (Test-Path $freshFull)) {
    Remove-Item -LiteralPath $freshFull -Recurse -Force
    Write-Host "Emptied $freshFull"
}
New-Item -ItemType Directory -Force -Path $freshFull | Out-Null

# A copy of the launcher next to it (its relative paths keep working) with every
# VICTORY_DATA_DIR it sets pointed at the fresh folder, and the same set at the top.
$text = Get-Content -Path $Launcher -Raw
$set = "`$env:VICTORY_DATA_DIR = '$($freshFull.Replace("'", "''"))'"
$pattern = '(?m)^(\s*)\$env:VICTORY_DATA_DIR\s*=.*$'
$found = [regex]::Matches($text, $pattern).Count
$text = [regex]::Replace($text, $pattern, { param($m) $m.Groups[1].Value + $set })
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { throw "Launcher copy does not parse: $($errors[0].Message)" }
$at = if ($ast.ParamBlock) { $ast.ParamBlock.Extent.EndOffset } else { 0 }
$text = $text.Insert($at, "`r`n# ---- play-fresh: empty data folder ----`r`n$set`r`n")
if ($text -match '--data[-_ ]') { Write-Warning 'The launcher also passes a data path as an argument; check that the game uses the fresh folder.' }
$copy = Join-Path (Split-Path $Launcher) 'play-victory-gta6-fresh-run.ps1'
Set-Content -Path $copy -Value $text -Encoding UTF8
Write-Host "Fresh start: data folder $freshFull ($found launcher line(s) redirected). Real save untouched."
& $copy @Rest
