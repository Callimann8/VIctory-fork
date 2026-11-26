<#
.SYNOPSIS
  One-click A/B test runner for VIctory GTA 6 builds.

.DESCRIPTION
  Runs a whole test plan on its own: shader warm-up for each build (and for
  runs whose switches change the shader cache), a fresh copy of the test save
  for every run (the real save is never used, and a run is stopped at once if
  the game reports any other save folder), each ~4-minute run with its switch
  set, the frame-log numbers collected, the picture check against the main
  build, and a results table at the end.

  Runs come from a plan file (plans\*.psd1). Runs during which the PC was in
  use (video, browser) are marked noisy, automatically (video decoding or
  other programs using the graphics card or processor) or by hand
  (Mark-Noisy.cmd or -Noisy). Noisy runs whose result is close to what they
  are compared with are run again at the end.

  Nothing in the launcher or the save is changed: each run starts a temporary
  copy of the launcher next to the original, with the run's switches and its
  own data folder set right before the game starts; the copy is deleted after.

.EXAMPLE
  .\Run-TestPlan.ps1 -Check                       # shows what would run, starts nothing
  .\Run-TestPlan.ps1                              # the whole plan
  .\Run-TestPlan.ps1 -Only MAIN,R0,R1 -Noisy      # some runs, PC in use
  .\Run-TestPlan.ps1 -Resume C:\xcl\runs\testbench\redesign-20261126-1830   # go on after a stop
#>
[CmdletBinding()]
param(
    [string]$Plan = (Join-Path $PSScriptRoot 'plans\redesign-20261126.psd1'),
    [string]$Config = (Join-Path $PSScriptRoot 'testbench.config.psd1'),
    [string[]]$Only,
    [switch]$Check,
    [switch]$Noisy,
    [switch]$NoRerun,
    [switch]$NoWarmup,
    [switch]$NoPictures,
    [string]$ReferencePics,
    [string]$Resume
)

$ErrorActionPreference = 'Stop'


# ------------------------------------------------------------------ setup

function Import-Psd1([string]$Path) {
    if (-not (Test-Path $Path)) { throw "not found: $Path" }
    Import-PowerShellDataFile -Path $Path
}

$cfg = Import-Psd1 $Config
$planData = Import-Psd1 $Plan

function Get-Setting([string]$Name, $Default) {
    if ($planData.ContainsKey($Name)) { return $planData[$Name] }
    if ($cfg.ContainsKey($Name)) { return $cfg[$Name] }
    return $Default
}

function Expand-Path([string]$P) { if ($P) { [Environment]::ExpandEnvironmentVariables($P) } else { $P } }
$RunsRoot       = Expand-Path (Get-Setting 'RunsRoot' 'C:\xcl\runs\testbench')
$RunSeconds     = [int](Get-Setting 'RunSeconds' 240)
$SkipSeconds    = [int](Get-Setting 'SkipSeconds' 60)
$EarlyStop      = [int](Get-Setting 'EarlyStopSeconds' 0)      # 0 = off
$EarlySlowerPct = [double](Get-Setting 'EarlyStopSlowerPct' 15)
$CloseCallPct   = [double](Get-Setting 'CloseCallPct' 0)       # 0 = off
$RerunSeconds   = [int](Get-Setting 'RerunSeconds' 240)
$StartTimeout   = [int](Get-Setting 'StartTimeoutSeconds' 900)
$WarmupTimeout  = [int](Get-Setting 'WarmupTimeoutSeconds' 7200)
$ProcessName    = Get-Setting 'ProcessName' 'victory'
$PictureMode    = if ($NoPictures) { 'off' } else { Get-Setting 'Pictures' 'engine' }
$PictureEvery   = [int](Get-Setting 'PictureEvery' 64)
$WindowEvery    = [int](Get-Setting 'WindowPictureSeconds' 20)
$MarginPct      = [double](Get-Setting 'NoiseMarginPct' 3)
$Baseline       = Get-Setting 'Baseline' ''
$PicRef         = Get-Setting 'PictureReference' ''
$Calibrate      = @(Get-Setting 'Calibrate' @())
$ShaderSwitches = @(Get-Setting 'ShaderSwitches' @('VICTORY_CODE_TABLE_AFTER', 'VICTORY_TRACER_GUARDS'))
$CommonEnv      = Get-Setting 'CommonEnv' @{}
$StartKeys      = @(Get-Setting 'StartKeys' @())
$LogSearch      = @(Get-Setting 'LogSearch' @('C:\xcl\runs') | ForEach-Object { Expand-Path $_ })
$RealDataDirs   = @(Get-Setting 'RealDataDirs' @() | ForEach-Object { Expand-Path $_ })
$SaveSource     = Expand-Path (Get-Setting 'SaveSource' '')
$NoisyFlag      = Join-Path $RunsRoot 'NOISY'
$Sets           = @{}
# Switch sets from a run-switches.ps1 ($VictoryRuns = [ordered]@{ R1 = @{...} }),
# read without running it; the plan's own Sets win over it.
$SetsFile = Expand-Path (Get-Setting 'SetsFile' '')
if ($SetsFile) {
    if (-not (Test-Path $SetsFile)) { throw "SetsFile not found: $SetsFile" }
    $tk = $null; $pe = $null
    $sAst = [System.Management.Automation.Language.Parser]::ParseFile($SetsFile, [ref]$tk, [ref]$pe)
    $assign = $sAst.Find({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$VictoryRuns' }, $true)
    $hAst = if ($assign) { $assign.Right.Find({ param($n) $n -is [System.Management.Automation.Language.HashtableAst] }, $true) } else { $null }
    if (-not $hAst) { throw "no `$VictoryRuns = @{...} in $SetsFile" }
    $fromFile = $hAst.SafeGetValue()
    foreach ($k in $fromFile.Keys) { $Sets[$k] = $fromFile[$k] }
}
$planSets = Get-Setting 'Sets' @{}
foreach ($k in $planSets.Keys) { $Sets[$k] = $planSets[$k] }
$Builds         = $cfg.Builds
if ($planData.ContainsKey('Builds')) { foreach ($k in $planData.Builds.Keys) { $Builds[$k] = $planData.Builds[$k] } }
foreach ($k in @($Builds.Keys)) { $Builds[$k].Launcher = Expand-Path $Builds[$k].Launcher }

$planName = if ($planData.ContainsKey('Name')) { $planData.Name } else { [IO.Path]::GetFileNameWithoutExtension($Plan) }
if ($Resume) { $PlanDir = $Resume } else { $PlanDir = Join-Path $RunsRoot ("{0}-{1}" -f $planName, (Get-Date -Format 'yyyyMMdd-HHmm')) }
$LogFile = Join-Path $PlanDir 'testbench.log'

function Say([string]$Text, [string]$Color = 'Gray') {
    $line = "{0}  {1}" -f (Get-Date -Format 'HH:mm:ss'), $Text
    Write-Host $line -ForegroundColor $Color
    if (Test-Path $PlanDir) { Add-Content -Path $LogFile -Value $line }
}

# ------------------------------------------------------------------ runs and switches

$AllRuns = @($planData.Runs)
$Results = [ordered]@{}   # name -> result object
$BestSets = New-Object System.Collections.Generic.List[string]
$BestFrame = $null

function Expand-Env($Run) {
    # Switches of a run: the sets it names in order ('best' = the sets of the
    # earlier Try runs that helped), then its own Env.
    $envMap = [ordered]@{}
    $used = New-Object System.Collections.Generic.List[string]
    $names = @()
    if ($Run.ContainsKey('Use')) { $names = @($Run.Use) }
    foreach ($n in $names) {
        $expand = if ($n -eq 'best') { @($BestSets) } else { @($n) }
        foreach ($s in $expand) {
            if (-not $Sets.ContainsKey($s)) { throw "run $($Run.Name): unknown set '$s'" }
            foreach ($k in $Sets[$s].Keys) { $envMap[$k] = [string]$Sets[$s][$k] }
            $used.Add($s)
        }
    }
    if ($Run.ContainsKey('Env')) { foreach ($k in $Run.Env.Keys) { $envMap[$k] = [string]$Run.Env[$k] } }
    return @{ Env = $envMap; Sets = @($used) }
}

function Get-OwnSets($Run) {
    if (-not $Run.ContainsKey('Use')) { return @() }
    return @($Run.Use | Where-Object { $_ -ne 'best' })
}

# ------------------------------------------------------------------ launcher copy

function New-LauncherCopy([string]$Launcher, $Overrides, [string]$Tag) {
    # A copy of the launcher next to it (so its own relative paths still
    # work) that sets $Overrides right after its param block and again right
    # before the line that starts the game or the warm-up.
    $text = Get-Content -Path $Launcher -Raw
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
    if ($errors.Count -gt 0) { throw "launcher does not parse: $($errors[0].Message)" }
    $block = "`r`n# ---- testbench: run '$Tag' ----`r`n"
    foreach ($k in $Overrides.Keys) {
        $v = ([string]$Overrides[$k]).Replace("'", "''")
        $block += "`$env:$k = '$v'`r`n"
    }
    $block += "# ---- testbench end ----`r`n"
    $inserts = New-Object System.Collections.Generic.List[int]
    if ($ast.ParamBlock) { $inserts.Add($ast.ParamBlock.Extent.EndOffset) } else { $inserts.Add(0) }
    # Every string that holds --execute or --warm (an argument, an argument
    # list, an array of arguments): the block goes before the statement that
    # holds it.
    $starts = $ast.FindAll({
            param($n)
            ($n -is [System.Management.Automation.Language.StringConstantExpressionAst] -or
                $n -is [System.Management.Automation.Language.ExpandableStringExpressionAst]) -and
            $n.Value -match '(^|\s)--(execute|warm)(\s|$)'
        }, $true)
    $found = 0
    $paramEnd = if ($ast.ParamBlock) { $ast.ParamBlock.Extent.EndOffset } else { 0 }
    foreach ($c in $starts) {
        $s = $c
        while ($s.Parent -and -not ($s.Parent -is [System.Management.Automation.Language.NamedBlockAst]) -and
            -not ($s.Parent -is [System.Management.Automation.Language.StatementBlockAst])) { $s = $s.Parent }
        if (-not $s.Parent -or $s.Extent.StartOffset -lt $paramEnd) { continue }
        $inserts.Add($s.Extent.StartOffset)
        $found++
    }
    $sorted = @($inserts | Sort-Object -Unique -Descending)
    foreach ($at in $sorted) { $text = $text.Insert($at, $block) }
    $copy = Join-Path (Split-Path $Launcher) (".testbench-{0}-{1}.ps1" -f $Tag, $PID)
    Set-Content -Path $copy -Value $text -Encoding UTF8
    return @{ Path = $copy; StartLines = $found }
}

# ------------------------------------------------------------------ processes

function Get-GameProcesses { @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue) }

function Stop-Tree([int]$ProcessId) {
    # Windows PowerShell 5.1 turns a program's error output into a stopping error under 'Stop'.
    $ErrorActionPreference = 'Continue'
    & taskkill.exe /T /F /PID $ProcessId 2>&1 | Out-Null
}

function Stop-Game($Launch) {
    if ($Launch) { Stop-Tree $Launch.Id }
    foreach ($p in Get-GameProcesses) { try { $p.Kill() } catch { } }
    $deadline = (Get-Date).AddSeconds(60)
    while (@(Get-GameProcesses).Count -gt 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
    if (@(Get-GameProcesses).Count -gt 0) { throw "the game did not close; stop it by hand and use -Resume" }
}

# Maximum / Average of one property over objects; objects without it (or with $null) are left out.
# (Windows PowerShell 5.1's Measure-Object -Property throws when any object lacks the property.)
function Get-Stat($Items, [string]$Name, [string]$What) {
    $vals = @($Items | ForEach-Object { if ($_ -and $_.PSObject.Properties[$Name] -and $null -ne $_.$Name) { [double]$_.$Name } })
    if ($vals.Count -eq 0) { return 0 }
    $m = $vals | Measure-Object -Maximum -Average
    if ($What -eq 'Maximum') { return $m.Maximum } else { return $m.Average }
}

function Start-Launcher([string]$Copy, [string[]]$LauncherArgs, [string]$Out, [string]$Err) {
    $shell = (Get-Process -Id $PID).Path
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$Copy`"") + @($LauncherArgs | Where-Object { $_ })
    $more = @{}
    if ($PSVersionTable.PSVersion.Major -lt 6 -or $IsWindows) { $more.WindowStyle = 'Minimized' }
    Start-Process -FilePath $shell -ArgumentList $argList -RedirectStandardOutput $Out -RedirectStandardError $Err -PassThru @more
}

# Reads what was added to a log since the last call.
$script:Offsets = @{}
function Read-New([string]$Path) {
    if (-not (Test-Path $Path)) { return '' }
    $off = if ($script:Offsets.ContainsKey($Path)) { $script:Offsets[$Path] } else { [long]0 }
    try {
        $fs = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite, Delete')
        try {
            if ($fs.Length -le $off) { return '' }
            # Windows PowerShell 5.1 writes UTF-16 (Out-File, Tee-Object, >).
            if ($off -eq 0 -and $fs.Length -ge 2) {
                $bom = New-Object byte[] 2; $fs.Read($bom, 0, 2) | Out-Null; $fs.Seek(0, 'Begin') | Out-Null
                $script:Offsets["$Path|utf16"] = ($bom[0] -eq 0xFF -and $bom[1] -eq 0xFE)
            }
            if ($script:Offsets["$Path|utf16"]) {
                $len = ($fs.Length - $off) - (($fs.Length - $off) % 2)
                if ($len -le 0) { return '' }
                $fs.Seek($off, 'Begin') | Out-Null
                $buf = New-Object byte[] $len
                $n = $fs.Read($buf, 0, $len)
                $text = [Text.Encoding]::Unicode.GetString($buf, 0, $n - ($n % 2))
                $cut = $text.LastIndexOf("`n")
                if ($cut -lt 0) { return '' }
                $script:Offsets[$Path] = $off + 2 * ($cut + 1)
                return $text.Substring(0, $cut + 1)
            }
            $fs.Seek($off, 'Begin') | Out-Null
            $buf = New-Object byte[] ($fs.Length - $off)
            $n = $fs.Read($buf, 0, $buf.Length)
            # Only whole lines; the rest waits for the next call.
            $lastNl = [Array]::LastIndexOf($buf, [byte]10, $n - 1)
            if ($lastNl -lt 0) { return '' }
            $script:Offsets[$Path] = $off + $lastNl + 1
            return [Text.Encoding]::UTF8.GetString($buf, 0, $lastNl + 1)
        } finally { $fs.Dispose() }
    } catch { return '' }
}

# A log the launcher writes itself, when its output is not ours.
function Find-LauncherLog([datetime]$Since) {
    foreach ($dir in $LogSearch) {
        if (-not (Test-Path $dir)) { continue }
        $f = Get-ChildItem -Path $dir -Recurse -File -Include *.log, *.txt -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -gt $Since -and $_.FullName -notlike "$PlanDir*" -and $_.Length -gt 1000 } |
            Sort-Object LastWriteTime -Descending | Select-Object -First 5
        foreach ($c in $f) {
            if (Select-String -Path $c.FullName -Pattern 'victory_gpu|victory_loader' -Quiet) { return $c.FullName }
        }
    }
    return $null
}

# ------------------------------------------------------------------ PC in use

function Get-Usage([int[]]$GamePids) {
    # Graphics card and processor use of everything but the game, from WMI
    # (counter names are not translated there, unlike Get-Counter's).
    $r = @{ Video = 0.0; Gpu = 0.0; Cpu = 0.0; Top = '' }
    try {
        $eng = Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -Property Name, UtilizationPercentage -ErrorAction Stop
        $byPid = @{}
        foreach ($e in $eng) {
            if ($e.Name -match '^pid_(\d+)_.*engtype_(.+)$') {
                $p = [int]$matches[1]; $kind = $matches[2]
                if ($GamePids -contains $p) { continue }
                if ($kind -like 'VideoDecode*') { $r.Video += $e.UtilizationPercentage }
                if ($kind -eq '3D') {
                    $r.Gpu += $e.UtilizationPercentage
                    $byPid[$p] = $e.UtilizationPercentage + $(if ($byPid.ContainsKey($p)) { $byPid[$p] } else { 0 })
                }
            }
        }
        $dwm = @(Get-Process -Name dwm -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
        foreach ($d in $dwm) { if ($byPid.ContainsKey($d)) { $r.Gpu -= $byPid[$d]; $byPid.Remove($d) } }
        if ($byPid.Count -gt 0) {
            $top = $byPid.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 1
            if ($top.Value -gt 2) {
                $name = (Get-Process -Id $top.Key -ErrorAction SilentlyContinue).ProcessName
                $r.Top = "$name $([int]$top.Value)% GPU"
            }
        }
    } catch { }
    try {
        $procs = Get-CimInstance -ClassName Win32_PerfFormattedData_PerfProc_Process -Property Name, IDProcess, PercentProcessorTime -ErrorAction Stop
        $cores = [Environment]::ProcessorCount
        $other = 0.0
        foreach ($p in $procs) {
            if ($p.Name -in @('_Total', 'Idle', 'System')) { continue }
            if ($GamePids -contains [int]$p.IDProcess -or [int]$p.IDProcess -eq $PID) { continue }
            $other += $p.PercentProcessorTime
        }
        $r.Cpu = $other / $cores
    } catch { }
    return [pscustomobject]$r
}

# Per-core processor use, the game's memory and graphics memory, free RAM
# (WMI classes: their names are not translated on a German Windows).
function Get-Sample([int[]]$GamePids) {
    $r = @{ Cores = @(); Ws = 0.0; PeakWs = 0.0; Commit = 0.0; Dedicated = 0.0; Shared = 0.0; FreeRam = $null; CpuMs = $null }
    try {
        $cpu = Get-CimInstance -ClassName Win32_PerfFormattedData_PerfOS_Processor -Property Name, PercentProcessorTime -ErrorAction Stop |
            Where-Object { $_.Name -ne '_Total' } | Sort-Object { [int]($_.Name -replace '\D', '') }
        $r.Cores = @($cpu | ForEach-Object { [double]$_.PercentProcessorTime })
    } catch { }
    foreach ($id in $GamePids) {
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $r.Ws += $p.WorkingSet64; $r.PeakWs += $p.PeakWorkingSet64; $r.Commit += $p.PrivateMemorySize64
        try { $r.CpuMs += $p.TotalProcessorTime.TotalMilliseconds } catch { }
    }
    try {
        $gm = Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -Property Name, DedicatedUsage, SharedUsage -ErrorAction Stop
        foreach ($g in $gm) {
            if ($g.Name -match '^pid_(\d+)_' -and $GamePids -contains [int]$matches[1]) { $r.Dedicated += $g.DedicatedUsage; $r.Shared += $g.SharedUsage }
        }
    } catch { }
    try { $r.FreeRam = [double](Get-CimInstance -ClassName Win32_OperatingSystem -Property FreePhysicalMemory -ErrorAction Stop).FreePhysicalMemory * 1KB } catch { }
    return [pscustomobject]$r
}

# ------------------------------------------------------------------ the save

$script:RealSaveStamp = $null
function Get-RealSaveStamp {
    $newest = [datetime]::MinValue
    foreach ($d in $RealDataDirs) {
        if (-not (Test-Path $d)) { continue }
        $f = Get-ChildItem -Path $d -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match '\\storage\\' } |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($f -and $f.LastWriteTime -gt $newest) { $newest = $f.LastWriteTime }
    }
    return $newest
}

function New-RunData([string]$Dest) {
    # A fresh copy of the test save, as a data folder (<title id>\storage\...).
    if (-not $SaveSource -or -not (Test-Path $SaveSource)) { throw "SaveSource not found: '$SaveSource'" }
    # Windows PowerShell 5.1 turns a program's error output into a stopping error under 'Stop'.
    $ErrorActionPreference = 'Continue'
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    $isData = @(Get-ChildItem -Path $SaveSource -Directory -ErrorAction SilentlyContinue | Where-Object { Test-Path (Join-Path $_.FullName 'storage') }).Count -gt 0
    if ($isData) {
        & robocopy.exe $SaveSource $Dest /E /NFL /NDL /NJH /NJS /NP /XD temp | Out-Null
    } else {
        # A copy of a storage folder alone: the title id from the real data folder.
        $tid = $null
        foreach ($d in $RealDataDirs) {
            $t = Get-ChildItem -Path $d -Directory -ErrorAction SilentlyContinue | Where-Object { Test-Path (Join-Path $_.FullName 'storage') } | Select-Object -First 1
            if ($t) { $tid = $t.Name; break }
        }
        if (-not $tid) { throw "SaveSource holds no '<title id>\storage' folder and no RealDataDirs entry names the title id" }
        & robocopy.exe $SaveSource (Join-Path $Dest "$tid\storage") /E /NFL /NDL /NJH /NJS /NP | Out-Null
    }
    if ($LASTEXITCODE -ge 8) { throw "copying the save failed (robocopy $LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
}

# ------------------------------------------------------------------ the frame log

# Per-frame timings of the frame line ("usual frame ..."), medians per run.
$FrameFields = [ordered]@{
    'render thread ran'         = 'render thread: ran ([\d.]+) ms'
    'render: in present call'   = 'in the present call ([\d.]+)'
    'render: in submit calls'   = 'in submit calls ([\d.]+)'
    'render: paced'             = ', paced ([\d.]+)'
    'submit thread at work'     = 'submit thread: at work ([\d.]+) ms'
    'submit: preparing'         = 'preparing ([\d.]+)'
    'submit: waiting for card'  = 'waiting for the card ([\d.]+)'
    'submit: copies'            = '; copies ([\d.]+) ms'
    'copy-back'                 = 'copy-back ([\d.]+) ms'
    'card idle'                 = 'card: idle ([\d.]+) ms'
    'card: send-and-hash lists' = 'send-and-hash lists ([\d.]+) ms'
    'card: compare lists'       = 'compare lists ([\d.]+) ms'
    'card: all other lists'     = 'all other lists ([\d.]+) ms'
    'recording, all threads'    = 'recording, all threads: ([\d.]+) ms'
    'textures: refresh'         = 'textures: refresh ([\d.]+) ms'
    'remapping'                 = 'remapping: [\d.]+ calls ([\d.]+) ms'
    'pipelines: waited'         = '([\d.]+) ms waited'
    'process: user'             = 'process: ([\d.]+) user'
    'process: kernel'           = 'user ([\d.]+) kernel ms'
}
# Further lines of "label N ms | label N ms" made with the frame line (e.g. a
# split of the preparing time); every label becomes a phase of its own.
$PhaseLines = @(Get-Setting 'PhaseLines' @('prep split:'))

function ConvertFrom-FrameLog([string]$Path, [string]$WorldStamp, [double]$Skip = -1) {
    if ($Skip -lt 0) { $Skip = $SkipSeconds }
    $m = @{ Usual = @(); Prep = @(); Idle = @(); Back = @(); Free = @(); Medians = @(); Frames = 0; Seconds = 0.0; Long = 0; Lines = 0
        Crash = ''; Slots = 0; Warmup = ''; Fields = [ordered]@{}; Phases = [ordered]@{}; SwitchLines = @(); Unmapped = ''
        Tail = (New-Object System.Collections.Generic.Queue[string]); Dumps = @(); CacheDir = ''; TexLine = ''; WaitLine = ''; Accounting = @() }
    if (-not (Test-Path $Path)) { return $m }
    foreach ($k in $FrameFields.Keys) { $m.Fields[$k] = New-Object System.Collections.Generic.List[double] }
    $world = $null
    if ($WorldStamp) { $world = Get-Stamp $WorldStamp }
    foreach ($raw in [IO.File]::ReadLines($Path)) {
        $line = Clear-Line $raw
        if (-not $line.Trim()) { continue }
        $m.Tail.Enqueue($line); if ($m.Tail.Count -gt 40) { [void]$m.Tail.Dequeue() }
        if ($line -match 'unhandled exception|DEVICE_REMOVED|device removed|device was removed|Present answered|panicked at') { if (-not $m.Crash) { $m.Crash = $line.Substring(0, [Math]::Min(300, $line.Length)) } }
        if ($line -match 'no free dynamic descriptor slots') { $m.Slots++ }
        if ($line -match 'warm-up\s+\d+ hashes asked') { $m.Warmup = $line.Trim() }
        if ($line -match 'crash|dump|minidump' -and $line -match '([A-Za-z]:[\\/][^\s"'']+\.(?:dmp|bin))\b') { $dp = $matches[1]; if ($m.Dumps -notcontains $dp) { $m.Dumps += $dp } }
        if (-not $m.CacheDir -and $line -match 'path=(.+?)[\\/]images[\\/]') { $m.CacheDir = $matches[1] }
        if (-not $m.Unmapped -and $line -match 'switch map: .* \|\| set, no counter: (.*?)(?: frames=\d+)?$') { $m.Unmapped = $matches[1].Trim() }
        $isFrame = $line -match 'framelog: (usual frame|frame times)'
        $isMap = $line -match 'switch map: '
        $isTex = $line -match 'texture problems: '
        $isAcc = $line -match 'card accounting: '
        $isWait = $line -match 'card waits: '
        $phase = $null
        foreach ($p in $PhaseLines) { if ($line.Contains($p)) { $phase = $p; break } }
        if (-not ($isFrame -or $isMap -or $phase -or $isTex -or $isAcc -or $isWait)) { continue }
        if ($world) {
            $t = Get-Stamp $line
            if ($t -and ($t - $world).TotalSeconds -lt $Skip) { continue }
        }
        if ($isTex) { $m.TexLine = ($line -replace '^.*?texture problems: ', '') -replace ' frames=\d+\s*$', ''; continue }
        if ($isWait) { $m.WaitLine = ($line -replace '^.*?card waits: ', '') -replace ' frames=\d+\s*$', ''; continue }
        if ($isAcc) { $m.Accounting += (($line -replace '^.*?card accounting: ', '') -replace ' frames=\d+\s*$', ''); continue }
        if ($isMap) {
            $text = ($line -replace '^.*?switch map: ', '') -replace ' frames=\d+\s*$', ''
            $m.SwitchLines += ($text -split ' \|\| ')[0]
            continue
        }
        if ($phase) {
            $text = ($line.Substring($line.IndexOf($phase) + $phase.Length)) -replace ' frames=\d+\s*$', ''
            foreach ($seg in ($text -split '\|')) {
                if ($seg -match '^\s*(.+?)\s+([\d.]+)\s*ms\b') {
                    $label = "$($phase.TrimEnd(':')) $($matches[1].Trim())"
                    if (-not $m.Phases.Contains($label)) { $m.Phases[$label] = New-Object System.Collections.Generic.List[double] }
                    $m.Phases[$label].Add([double]$matches[2])
                }
            }
            continue
        }
        if ($line -match 'usual frame \(the mean of \d+ not long\): ([\d.]+) ms') {
            $m.Lines++
            $m.Usual += [double]$matches[1]
            if ($line -match 'preparing ([\d.]+)') { $m.Prep += [double]$matches[1] }
            if ($line -match 'card: idle ([\d.]+) ms') { $m.Idle += [double]$matches[1] }
            if ($line -match 'copy-back ([\d.]+) ms') { $m.Back += [double]$matches[1] }
            if ($line -match '\| (\d+) MB free') { $m.Free += [double]$matches[1] }
            foreach ($k in $FrameFields.Keys) { if ($line -match $FrameFields[$k]) { $m.Fields[$k].Add([double]$matches[1]) } }
        } elseif ($line -match 'frame times: (\d+) frames in ([\d.]+) s: shortest \d+ ms, median (\d+), longest \d+; (\d+) long') {
            $m.Frames += [int]$matches[1]; $m.Seconds += [double]$matches[2]
            $m.Medians += [double]$matches[3]; $m.Long += [int]$matches[4]
        }
    }
    return $m
}

# The switch map's last lines of the measured stretch, per switch: what the
# last line says, refusals over those lines, and whether it never acted.
function ConvertFrom-SwitchMap([string[]]$Lines, $RunEnv) {
    $rows = [ordered]@{}
    foreach ($text in $Lines) {
        if ($text -match '^no mapped switch set') { continue }
        foreach ($part in ($text -split ' \| ')) {
            if ($part -notmatch '^(VICTORY_[A-Z0-9_]+)=(.*?): (.*)$') { continue }
            $n = $matches[1]; $v = $matches[2]; $what = $matches[3]
            if (-not $rows.Contains($n)) { $rows[$n] = [ordered]@{ Switch = $n; Value = $v; Last = ''; Never = $false; Refused = 0; Why = '' } }
            $row = $rows[$n]
            $row.Last = $what; $row.Value = $v
            $row.Never = $what -like 'NEVER ACTED*'
            if ($what -match 'refused (\d+) \((.*)\)\s*$') { $row.Refused += [int]$matches[1]; $row.Why = $matches[2] }
        }
    }
    # Switches of the run the map does not name at all (an older build, or not wired in yet).
    $missing = @()
    if ($Lines.Count) { foreach ($k in $RunEnv.Keys) { if (-not $rows.Contains($k)) { $missing += $k } } }
    return @{ Rows = @($rows.Values | ForEach-Object { [pscustomobject]$_ }); Missing = $missing }
}

$TimeRx = '(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z)'
function Get-Stamp([string]$Line) {
    if ($Line -match $TimeRx) { return [datetime]::Parse($matches[1], [Globalization.CultureInfo]::InvariantCulture, 'RoundtripKind') }
    return $null
}
# Colour codes and the prefixes PowerShell puts on a program's error output.
$EscRx = [string][char]27 + '\[[0-9;]*[A-Za-z]'
function Clear-Line([string]$Line) { ($Line -replace $EscRx, '').TrimEnd("`r") }

function Median([double[]]$V) {
    if (-not $V -or $V.Count -eq 0) { return $null }
    $s = $V | Sort-Object
    $n = $s.Count
    if ($n % 2) { return [double]$s[[int](($n - 1) / 2)] }
    return ([double]$s[$n / 2 - 1] + [double]$s[$n / 2]) / 2
}

# ------------------------------------------------------------------ pictures

function Get-Piccheck {
    $exe = Get-Setting 'PiccheckExe' ''
    if ($exe -and (Test-Path $exe)) { return $exe }
    $here = Join-Path $PSScriptRoot 'piccheck.exe'
    if (Test-Path $here) { return $here }
    $built = Join-Path $PSScriptRoot 'piccheck\target\release\piccheck.exe'
    # Windows PowerShell 5.1 turns a program's error output into a stopping error under 'Stop'.
    $ErrorActionPreference = 'Continue'
    if (-not (Test-Path $built)) {
        Say 'building the picture check (once)...'
        Push-Location (Join-Path $PSScriptRoot 'piccheck')
        try { & cargo build --release --quiet 2>&1 | Out-File (Join-Path $PlanDir 'piccheck-build.log') } finally { Pop-Location }
    }
    if (Test-Path $built) { return $built }
    Say 'picture check not available (see piccheck-build.log); runs go on without it' 'Yellow'
    return $null
}

function Save-WindowPicture([string]$Dir, [int]$Second) {
    Add-Type -AssemblyName System.Drawing
    if (-not ('TB.Win' -as [type])) {
        Add-Type -Namespace TB -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
public struct RECT { public int Left, Top, Right, Bottom; }
public struct POINT { public int X, Y; }
'@
    }
    $p = Get-GameProcesses | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $p) { return $false }
    # Only when the game is in front: anything over it would be in the picture.
    if ([TB.Win]::GetForegroundWindow() -ne $p.MainWindowHandle) { return $false }
    $r = New-Object TB.Win+RECT
    [TB.Win]::GetClientRect($p.MainWindowHandle, [ref]$r) | Out-Null
    $pt = New-Object TB.Win+POINT
    [TB.Win]::ClientToScreen($p.MainWindowHandle, [ref]$pt) | Out-Null
    $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
    if ($w -lt 64 -or $h -lt 64) { return $false }
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($pt.X, $pt.Y, 0, 0, $bmp.Size)
    $half = New-Object System.Drawing.Bitmap $bmp, ([int]($w / 2)), ([int]($h / 2))
    $half.Save((Join-Path $Dir ("win-{0:D5}.png" -f $Second)), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose(); $half.Dispose()
    return $true
}

function Compare-Pictures($Result) {
    # Windows PowerShell 5.1 turns a program's error output into a stopping error under 'Stop'.
    $ErrorActionPreference = 'Continue'
    if ($PictureMode -eq 'off' -or -not $script:Piccheck) { return }
    $refDir = $script:RefPics
    if (-not $refDir -or -not (Test-Path $refDir) -or $Result.Pics -eq $refDir) { return }
    if (-not $Result.Pics -or -not (Test-Path $Result.Pics)) { return }
    $out = Join-Path $Result.Dir 'pictures'
    $argList = @('compare', '--ref', $refDir, '--test', $Result.Pics, '--out', $out, '--title', "$($Result.Name) vs $PicRef")
    $noise = Join-Path $PlanDir 'picture-noise.txt'
    if (Test-Path $noise) { $argList += @('--noise', $noise) }
    $line = & $script:Piccheck @argList 2>&1 | ForEach-Object { "$_" -replace '^System\.Management\.Automation\.RemoteException$', '' } | Select-Object -Last 1
    $Result.Pictures = switch ($LASTEXITCODE) { 0 { 'PASS' } 1 { 'CHECK' } 2 { 'FAIL' } 3 { 'none' } default { 'error' } }
    $Result.PictureNote = [string]$line
    $global:LASTEXITCODE = 0
}

# ------------------------------------------------------------------ one run

function Invoke-Warmup([string]$BuildName, $EnvMap, [string]$Tag) {
    $b = $Builds[$BuildName]
    $dir = Join-Path $PlanDir "warmup-$Tag"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $over = [ordered]@{}
    foreach ($k in $CommonEnv.Keys) { $over[$k] = $CommonEnv[$k] }
    foreach ($k in $EnvMap.Keys) { $over[$k] = $EnvMap[$k] }
    # An empty data folder: a launcher that goes on into the game after the
    # warm-up then starts it without any save.
    $over['VICTORY_DATA_DIR'] = (Join-Path $dir 'empty-data')
    $copy = New-LauncherCopy $b.Launcher $over "warmup-$Tag"
    $wargs = if ($b.ContainsKey('WarmupArgs')) { @($b.WarmupArgs) } else { @('-Warmup') }
    Say "warm-up for $Tag ($BuildName): $($b.Launcher) $($wargs -join ' ')" 'Cyan'
    $out = Join-Path $dir 'out.log'; $err = Join-Path $dir 'err.log'
    $began = Get-Date
    $proc = Start-Launcher $copy.Path $wargs $out $err
    $summary = ''
    try {
        while (-not $proc.HasExited) {
            Start-Sleep -Seconds 5
            foreach ($f in @($out, $err)) {
                $t = Read-New $f
                foreach ($l in ($t -split "`n")) { if ($l -match 'warm-up\s+\d+ hashes asked') { $summary = $l.Trim() } }
            }
            # A launcher that goes on into the game after its warm-up: stop it there.
            if ($summary) { Start-Sleep -Seconds 3; break }
            if (((Get-Date) - $began).TotalSeconds -gt $WarmupTimeout) { Say "warm-up took over $WarmupTimeout s; stopped" 'Yellow'; break }
        }
    } finally {
        if (-not $proc.HasExited -or @(Get-GameProcesses).Count -gt 0) { Stop-Game $proc }
        Remove-Item -Force -Path $copy.Path -ErrorAction SilentlyContinue
    }
    if (-not $summary) {
        foreach ($f in @($out, $err)) { $s = Select-String -Path $f -Pattern 'warm-up\s+\d+ hashes asked' -ErrorAction SilentlyContinue | Select-Object -Last 1; if ($s) { $summary = $s.Line.Trim() } }
    }
    Say ("warm-up done in {0:N0} s: {1}" -f ((Get-Date) - $began).TotalSeconds, $(if ($summary) { $summary } else { 'no summary line (see ' + $dir + ')' }))
}

function Invoke-Run($Run, [string]$Name, $Expanded, [int]$Seconds = 0) {
    $b = $Builds[$Run.Build]
    # Measured time: the call's, else the run's own Seconds, else RunSeconds.
    $secs = if ($Seconds -gt 0) { $Seconds } elseif ($Run.ContainsKey('Seconds')) { [int]$Run.Seconds } else { $RunSeconds }
    # Early stop (Try runs only; not the baseline): after EarlyStop s, slower than the best stack by EarlySlowerPct.
    $early = $EarlyStop -gt 0 -and $Run.ContainsKey('Try') -and $Run.Try -and $Run.Name -ne $Baseline -and $BestFrame -and $secs -gt $EarlyStop
    $earlyDone = $false
    $mode = if ($b.ContainsKey('Pictures') -and $PictureMode -ne 'off') { $b.Pictures } else { $PictureMode }
    $dir = Join-Path $PlanDir $Name
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $data = Join-Path $dir 'data'
    $pics = Join-Path $dir 'frames'
    New-Item -ItemType Directory -Force -Path $pics | Out-Null
    $r = [ordered]@{ Name = $Name; Build = $Run.Build; Sets = ($Expanded.Sets -join '+'); Switches = (($Expanded.Env.GetEnumerator() | ForEach-Object { "$($_.Key -replace '^VICTORY_','')=$($_.Value)" }) -join ' ')
        Phases = $null; SwitchMap = @(); SwitchMissing = @(); SwitchUnmapped = ''; SwitchFlags = ''; CpuCores = @(); CpuAllPct = $null; BusyCores = $null; CpuMsPerFrame = $null
        TexLine = ''; TexFlags = ''; WaitLine = ''; WaitMs = $null; Accounting = @(); PeakWsGB = $null; PeakCommitGB = $null; PeakVramGB = $null; PeakSharedGB = $null; MinFreeRamGB = $null; Crash = $null
        Status = 'not run'; Seconds = $null; FrameMs = $null; Fps = $null; MedianMs = $null; PrepMs = $null; GpuMs = $null; CopyBackMs = $null; FreeMbMin = $null; LongPct = $null
        VsBase = $null; Helped = $null; Pictures = ''; PictureNote = ''; Noisy = [bool]$Noisy; NoisyWhy = @($(if ($Noisy) { 'whole plan marked noisy (-Noisy)' })); Note = ''; Dir = $dir; Pics = $pics; Log = ''; World = ''; Started = (Get-Date).ToString('s') }
    Say "copying the test save for $Name..."
    New-RunData $data

    $over = [ordered]@{}
    foreach ($k in $CommonEnv.Keys) { $over[$k] = [string]$CommonEnv[$k] }
    foreach ($k in $Expanded.Env.Keys) { $over[$k] = $Expanded.Env[$k] }
    $over['VICTORY_DATA_DIR'] = $data
    $over['VICTORY_FRAME_LOG'] = '1'
    if ($mode -eq 'engine') {
        $over['VICTORY_CAPTURE'] = $pics
        $over['VICTORY_CAPTURE_WORLD'] = '1'
        $over['VICTORY_CAPTURE_WORLD_EVERY'] = [string]$PictureEvery
    }
    ($over.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) | Set-Content (Join-Path $dir 'env.txt')
    $copy = New-LauncherCopy $b.Launcher $over $Name
    $largs = if ($b.ContainsKey('Args')) { @($b.Args) } else { @() }
    $out = Join-Path $dir 'out.log'; $err = Join-Path $dir 'err.log'
    Say ("{0}: starting {1} ({2})  {3}" -f $Name, $Run.Build, $r.Sets, $r.Switches) 'Cyan'
    $began = Get-Date
    $proc = Start-Launcher $copy.Path $largs $out $err
    $logs = @($out, $err)
    $world = $null; $worldAt = $null; $storageOk = $false; $gamePids = @()
    $usage = New-Object System.Collections.Generic.List[object]
    $samples = New-Object System.Collections.Generic.List[object]
    $lastSample = Get-Date; $lastWin = -1; $lastPc = Get-Date
    $keysSent = 0
    if ($Run.ContainsKey('Prompt')) { $promptDone = $false } else { $promptDone = $true }
    try {
        while ($true) {
            Start-Sleep -Seconds 2
            $now = Get-Date
            $gp = Get-GameProcesses | Where-Object { try { $_.StartTime -ge $began.AddSeconds(-2) } catch { $true } }
            if ($gp) { $gamePids = @($gp | ForEach-Object { $_.Id }) }
            if (((Get-Date) - $began).TotalSeconds -gt 60 -and -not $r.Log -and $logs.Count -eq 2) {
                $own = (Get-Item $out -ErrorAction SilentlyContinue).Length + (Get-Item $err -ErrorAction SilentlyContinue).Length
                if ($own -lt 2000) { $found = Find-LauncherLog $began; if ($found) { $logs += $found; $r.Log = $found; Say "reading the launcher's own log $found" } }
            }
            foreach ($f in $logs) {
                $t = Read-New $f
                if (-not $t) { continue }
                foreach ($rawLine in ($t -split "`n")) {
                    $l = Clear-Line $rawLine
                    if ($l -match '(?:^|\s)title\s+[0-9A-F]{8}, storage (.+?)[\\/]storage \(') {
                        $st = [IO.Path]::GetFullPath($matches[1])
                        if ($st.StartsWith([IO.Path]::GetFullPath($data), [StringComparison]::OrdinalIgnoreCase)) { $storageOk = $true; if (-not $r.Log) { $r.Log = $f } }
                        else {
                            Stop-Game $proc
                            throw "SAVE SAFETY STOP: the game used '$st', not the run's copy '$data'. The launcher sets the data folder in a way the runner cannot override; fix the config before running again."
                        }
                    }
                    if (-not $world -and ($l -match 'the first frame of 500 draws or more' -or ($l -match 'framelog: usual frame' -and $l -match '\| (\d+) draws \|' -and [int]$matches[1] -ge 500))) {
                        if ($l -match $TimeRx) { $world = $matches[1] } else { $world = (Get-Date).ToUniversalTime().ToString('o') }
                        if ($l -match 'frame=(\d+)') { Set-Content -Path (Join-Path $pics 'world.txt') -Value "first=$($matches[1])" }
                        $worldAt = Get-Date
                        if (-not $storageOk) { Stop-Game $proc; throw "SAVE SAFETY STOP: the game world started but the log never said which save folder it uses; not going on blind." }
                        Say "$Name`: world started; measuring for $secs s"
                    }
                }
            }
            # Key presses to get from the title screen into the save.
            while ($keysSent -lt $StartKeys.Count -and $gp -and ($now - $began).TotalSeconds -ge [double]$StartKeys[$keysSent].AfterSeconds) {
                $sh = New-Object -ComObject WScript.Shell
                if ($sh.AppActivate($gamePids[0])) { $sh.SendKeys([string]$StartKeys[$keysSent].Keys) }
                $keysSent++
            }
            if ($world -and -not $promptDone) {
                [Console]::Beep(880, 300)
                Read-Host "$Name`: $($Run.Prompt) -- press Enter when there"
                $promptDone = $true; $worldAt = Get-Date
                $world = (Get-Date).ToUniversalTime().ToString('o')
                Say "$Name`: measuring from now for $secs s"
            }
            # The measured stretch (after the first SkipSeconds, like the frame-log numbers).
            if ($world -and $promptDone -and ($now - $worldAt).TotalSeconds -ge $SkipSeconds -and ($now - $lastPc).TotalSeconds -ge 5) {
                $samples.Add((Get-Sample $gamePids)); $lastPc = $now
            }
            if ($world -and $promptDone -and ($now - $lastSample).TotalSeconds -ge 10) {
                $usage.Add((Get-Usage $gamePids)); $lastSample = $now
                if (Test-Path $NoisyFlag) { if ($r.NoisyWhy -notcontains 'marked by hand') { $r.NoisyWhy += 'marked by hand' } }
            }
            if ($world -and $mode -eq 'window') {
                $sec = [int]($now - $worldAt).TotalSeconds
                if ($sec - $lastWin -ge $WindowEvery) { if (Save-WindowPicture $pics $sec) { $lastWin = $sec } }
            }
            if ($proc.HasExited -and -not (Get-GameProcesses | Where-Object { $gamePids -contains $_.Id })) {
                $r.Status = if ($world) { 'ENDED EARLY' } else { 'NO START' }
                break
            }
            if (-not $world -and ($now - $began).TotalSeconds -gt $StartTimeout) { $r.Status = 'NO WORLD'; break }
            if ($early -and -not $earlyDone -and $world -and $promptDone -and ($now - $worldAt).TotalSeconds -ge $EarlyStop -and $r.Log) {
                $earlyDone = $true
                # The frame-log reports of the second half of the first EarlyStop seconds.
                $mm = ConvertFrom-FrameLog $r.Log $world ($EarlyStop / 2)
                $f = Median $mm.Usual
                if ($f -and $mm.Usual.Count -ge 2) {
                    $slower = 100.0 * ($f - $BestFrame) / $BestFrame
                    if ($slower -ge $EarlySlowerPct) {
                        $r.Status = 'STOPPED EARLY'
                        $r.Note = ("stopped early: slower ({0:N0} ms after {1} s vs best stack {2:N0} ms, {3:+0}%)" -f $f, $EarlyStop, $BestFrame, $slower)
                        Say "$Name`: $($r.Note)" 'Yellow'
                        break
                    }
                }
            }
            if ($world -and $promptDone -and ($now - $worldAt).TotalSeconds -ge $secs) { $r.Status = 'OK'; break }
        }
    } finally {
        Stop-Game $proc
        Remove-Item -Force -Path $copy.Path -ErrorAction SilentlyContinue
        if (Test-Path $NoisyFlag) { if ($r.NoisyWhy -notcontains 'marked by hand') { $r.NoisyWhy += 'marked by hand' } }
    }
    $r.World = $world
    if (-not $r.Log) { $r.Log = if ((Get-Item $err -ErrorAction SilentlyContinue).Length -gt (Get-Item $out -ErrorAction SilentlyContinue).Length) { $err } else { $out } }

    # The real save must be as it was.
    $stamp = Get-RealSaveStamp
    if ($script:RealSaveStamp -and $stamp -gt $script:RealSaveStamp) {
        throw "SAVE SAFETY STOP: a file of the real save changed during run $Name ($stamp). Check C:\xcl save backups before anything else."
    }

    $stoppedEarly = $r.Status -eq 'STOPPED EARLY'
    $m = if ($stoppedEarly) { ConvertFrom-FrameLog $r.Log $world ($EarlyStop / 2) } else { ConvertFrom-FrameLog $r.Log $world }
    $crashed = ($r.Status -ne 'OK' -and $r.Status -ne 'not run' -and -not $stoppedEarly) -or $m.Crash
    if ($m.Crash -and $r.Status -ne 'OK') { $r.Status = 'CRASH' }
    if ($crashed) { try { Save-CrashInfo $r $m $data $began } catch { Say "crash details incomplete: $($_.Exception.Message)" 'Yellow' } }
    # The save copy is not needed any more (pictures and logs stay; kept after a crash).
    if (-not (Get-Setting 'KeepRunData' $false) -and -not $crashed) { Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue }

    $r.FrameMs = Median $m.Usual
    $r.MedianMs = Median $m.Medians
    $r.PrepMs = Median $m.Prep
    if ($m.Usual.Count -gt 0 -and $m.Idle.Count -eq $m.Usual.Count) {
        $busy = @(); for ($i = 0; $i -lt $m.Usual.Count; $i++) { $busy += [Math]::Max(0, $m.Usual[$i] - $m.Idle[$i]) }
        $r.GpuMs = Median $busy
    }
    $r.CopyBackMs = Median $m.Back
    if ($m.Free.Count) { $r.FreeMbMin = ($m.Free | Measure-Object -Minimum).Minimum }
    if ($m.Seconds -gt 0) { $r.Fps = [Math]::Round($m.Frames / $m.Seconds, 2) }
    if ($m.Frames -gt 0) { $r.LongPct = [Math]::Round(100.0 * $m.Long / $m.Frames, 1) }
    if ($m.Crash) { $r.Note = ($m.Crash + ' ' + $r.Note).Trim(); if ($r.Status -ne 'OK') { $r.Status = 'CRASH' } }
    $r.Seconds = $secs
    # Phases: the frame line's timings, then the extra phase lines.
    $ph = [ordered]@{}
    foreach ($k in $m.Fields.Keys) { $v = Median $m.Fields[$k].ToArray(); if ($null -ne $v) { $ph[$k] = [Math]::Round($v, 1) } }
    if ($null -ne $r.GpuMs) { $ph['GPU busy (frame - card idle)'] = [Math]::Round($r.GpuMs, 1) }
    foreach ($k in $m.Phases.Keys) { $ph[$k] = [Math]::Round((Median $m.Phases[$k].ToArray()), 1) }
    $r.Phases = [pscustomobject]$ph
    # Switch map: the last 3 lines of the measured stretch.
    $last3 = @($m.SwitchLines | Select-Object -Last 3)
    $sm = ConvertFrom-SwitchMap $last3 $Expanded.Env
    $r.SwitchMap = $sm.Rows
    $r.SwitchMissing = $sm.Missing
    $r.SwitchUnmapped = $m.Unmapped
    $never = @($sm.Rows | Where-Object { $_.Never }).Count
    $refused = @($sm.Rows | Where-Object { $_.Refused -gt 0 }).Count
    $flags = @()
    if (-not $last3.Count -and $Expanded.Env.Count) { $flags += 'no switch map' }
    if ($never) { $flags += "$never never acted" }
    if ($refused) { $flags += "$refused refused" }
    if ($sm.Missing.Count) { $flags += "$($sm.Missing.Count) not in map" }
    # Texture problems (last line of the measured stretch) and card accounting.
    $r.TexLine = $m.TexLine
    $r.Accounting = @($m.Accounting | Select-Object -Last 5)
    # Card waits (last line of the measured stretch): "none" or "N/f, X ms/f; ...".
    $r.WaitLine = $m.WaitLine
    if ($m.WaitLine -match '^\s*none\b') { $r.WaitMs = 0 }
    elseif ($m.WaitLine -match '^\s*[\d.]+/f,\s*([\d.]+)\s*ms/f') { $r.WaitMs = [double]$matches[1] }
    # Every single "card wait:" line (VICTORY_WAIT_LOG=1), whole log.
    if ($r.Log -and (Test-Path $r.Log)) {
        $one = @(Select-String -LiteralPath $r.Log -Pattern 'card wait: ' -SimpleMatch | ForEach-Object { Clear-Line $_.Line })
        if ($one.Count) { $one | Set-Content (Join-Path $dir 'card-waits.txt') -Encoding UTF8 }
    }
    if ($m.Accounting.Count) { $m.Accounting | Set-Content (Join-Path $dir 'card-accounting.txt') -Encoding UTF8 }
    $tex = @()
    if ($m.TexLine -match '([\d.]+) slots refused/f' -and [double]$matches[1] -gt 0) { $tex += "slots refused $($matches[1])/f" }
    if ($m.TexLine -match 'skipped: ([\d.]+)/f unknown layout, ([\d.]+)/f XOR mode') {
        if ([double]$matches[1] -gt 0) { $tex += "unknown-layout uploads skipped $($matches[1])/f" }
        if ([double]$matches[2] -gt 0) { $tex += "XOR uploads skipped $($matches[2])/f" }
    }
    if ($m.TexLine -match 'card refused to make ([\d.]+)/f' -and [double]$matches[1] -gt 0) { $tex += "card refused $($matches[1])/f" }
    $r.TexFlags = $tex -join ', '
    if ($tex.Count) { $flags += "TEXTURES: $($r.TexFlags)" }
    $r.SwitchFlags = $flags -join ', '
    # Processor and memory over the measured stretch.
    if ($samples.Count) {
        $nc = ($samples | ForEach-Object { $_.Cores.Count } | Measure-Object -Maximum).Maximum
        $cores = @()
        for ($c = 0; $c -lt $nc; $c++) { $cores += [Math]::Round((($samples | Where-Object { $_.Cores.Count -gt $c } | ForEach-Object { $_.Cores[$c] }) | Measure-Object -Average).Average, 0) }
        $r.CpuCores = $cores
        if ($cores.Count) { $r.CpuAllPct = [Math]::Round(($cores | Measure-Object -Average).Average, 0); $r.BusyCores = @($cores | Where-Object { $_ -ge 50 }).Count }
        $gb = { param($x) [Math]::Round($x / 1GB, 2) }
        $r.PeakWsGB = & $gb (($samples | ForEach-Object { [Math]::Max($_.Ws, $_.PeakWs) } | Measure-Object -Maximum).Maximum)
        $r.PeakCommitGB = & $gb ((Get-Stat $samples 'Commit' 'Maximum'))
        $r.PeakVramGB = & $gb ((Get-Stat $samples 'Dedicated' 'Maximum'))
        $r.PeakSharedGB = & $gb ((Get-Stat $samples 'Shared' 'Maximum'))
        $free = @($samples | Where-Object { $null -ne $_.FreeRam } | ForEach-Object { $_.FreeRam })
        if ($free.Count) { $r.MinFreeRamGB = & $gb (($free | Measure-Object -Minimum).Minimum) }
        $cpuS = @($samples | Where-Object { $null -ne $_.CpuMs })
        if ($cpuS.Count -ge 2 -and $m.Seconds -gt 0 -and $m.Frames -gt 0) {
            $span = ($cpuS.Count - 1) * 5.0
            $fps = $m.Frames / $m.Seconds
            $r.CpuMsPerFrame = [Math]::Round(($cpuS[-1].CpuMs - $cpuS[0].CpuMs) / [Math]::Max(1, $span * $fps), 0)
        }
        "core," + (0..($nc - 1) -join ',') | Set-Content (Join-Path $dir 'cpu-cores.csv')
        $samples | ForEach-Object { 'sample,' + ($_.Cores -join ',') } | Add-Content (Join-Path $dir 'cpu-cores.csv')
    }
    if ($m.Slots) { $r.Note = ($r.Note + " | $($m.Slots)x no free dynamic descriptor slots").Trim(' |') }
    if ($r.Status -eq 'OK' -and $m.Lines -lt 5) { $r.Status = 'FEW LINES'; $r.Note = ($r.Note + " | only $($m.Lines) frame-log reports after the first $SkipSeconds s").Trim(' |') }

    if ($usage.Count) {
        $video = (Get-Stat $usage 'Video' 'Average')
        $gpu = (Get-Stat $usage 'Gpu' 'Average')
        $cpu = (Get-Stat $usage 'Cpu' 'Average')
        $top = ($usage | Where-Object { $_.Top } | Group-Object Top | Sort-Object Count -Descending | Select-Object -First 1)
        $lim = Get-Setting 'NoisyLimits' @{ VideoPct = 1.0; GpuPct = 4.0; CpuPct = 5.0 }
        if ($video -gt $lim.VideoPct) { $r.NoisyWhy += ("video playing ({0:N1}% decoder)" -f $video) }
        if ($gpu -gt $lim.GpuPct) { $r.NoisyWhy += ("other programs on the graphics card ({0:N0}%{1})" -f $gpu, $(if ($top) { ', ' + $top.Name } else { '' })) }
        if ($cpu -gt $lim.CpuPct) { $r.NoisyWhy += ("other programs on the processor ({0:N0}% of all cores)" -f $cpu) }
        "video_pct,gpu_pct,cpu_pct,top" | Set-Content (Join-Path $dir 'pc-use.csv')
        $usage | ForEach-Object { "{0:N2},{1:N2},{2:N2},{3}" -f $_.Video, $_.Gpu, $_.Cpu, $_.Top } | Add-Content (Join-Path $dir 'pc-use.csv')
    }
    if ($r.NoisyWhy.Count) { $r.Noisy = $true }
    if ($mode -ne 'off' -and $world -and @(Get-ChildItem -Path $pics -Filter *.png -ErrorAction SilentlyContinue).Count -eq 0) {
        $r.Note = ($r.Note + " | no pictures saved (a build without diagnostics? set Pictures = 'window' for this build)").Trim(' |')
    }
    if (Test-Path $NoisyFlag) { Remove-Item $NoisyFlag -ErrorAction SilentlyContinue }
    $obj = [pscustomobject]$r
    Say ("{0}: {1}  frame {2} ms  fps {3}  prep {4}  GPU {5}{6}" -f $Name, $r.Status, (Fmt $r.FrameMs), $r.Fps, (Fmt $r.PrepMs), (Fmt $r.GpuMs), $(if ($r.Noisy) { '  NOISY: ' + ($r.NoisyWhy -join ', ') } else { '' })) $(if ($r.Status -eq 'OK') { 'Green' } else { 'Yellow' })
    return $obj
}

function Fmt($x) { if ($null -eq $x) { '-' } else { '{0:N0}' -f $x } }

# On a crash or early end: the exception line, the last 40 log lines, the
# game's crashes.log and any dump file written since the run began.
function Save-CrashInfo($R, $M, [string]$Data, [datetime]$Began) {
    $dumps = New-Object System.Collections.Generic.List[string]
    foreach ($d in $M.Dumps) { if (Test-Path $d) { $dumps.Add($d) } else { $dumps.Add("$d (named in the log, not found)") } }
    $dirs = @($Data) + @(Get-Setting 'CrashDirs' @('C:\xcl\runs\dumps', 'C:\xcl\victory-data', '%LOCALAPPDATA%\CrashDumps', '%LOCALAPPDATA%\victory') | ForEach-Object { Expand-Path $_ })
    foreach ($d in $dirs) {
        if (-not (Test-Path $d)) { continue }
        Get-ChildItem -Path $d -Recurse -Depth 3 -File -Include *.dmp, crash-*.bin, crashes.log -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -ge $Began } | ForEach-Object { if (-not $dumps.Contains($_.FullName)) { $dumps.Add($_.FullName) } }
    }
    if ($M.CacheDir -and (Test-Path $M.CacheDir)) {
        Get-ChildItem -Path $M.CacheDir -File -Filter 'crash-*.bin' -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -ge $Began } |
            ForEach-Object { if (-not $dumps.Contains($_.FullName)) { $dumps.Add($_.FullName) } }
    }
    $crashLog = Get-ChildItem -Path $Data -Recurse -File -Filter crashes.log -ErrorAction SilentlyContinue | Select-Object -First 1
    $copied = ''
    if ($crashLog) { $copied = Join-Path $R.Dir 'crashes.log'; Copy-Item $crashLog.FullName $copied -Force }
    $tail = @($M.Tail.ToArray())
    $R.Crash = [pscustomobject]@{ Exception = $M.Crash; Dumps = @($dumps); CrashesLog = $copied; Tail = $tail }
    $txt = @("run $($R.Name): $($R.Status)", "exception: $(if ($M.Crash) { $M.Crash } else { '(no exception line in the log)' })", "crashes.log: $(if ($copied) { $copied } else { '(none)' })", 'dump files:') +
        @($(if ($dumps.Count) { $dumps | ForEach-Object { "  $_" } } else { '  (none found)' })) + @('', 'last 40 log lines:') + $tail
    $txt | Set-Content (Join-Path $R.Dir 'crash.txt') -Encoding UTF8
}

# The full picture of one run, as Markdown (also in results.md).
function Get-RunDetails($R) {
    $o = New-Object System.Collections.Generic.List[string]
    $o.Add("### $($R.Name): $($R.Status), frame $(Fmt $R.FrameMs) ms, $($R.Fps) fps")
    $o.Add('')
    $o.Add("Switches: ``$(if ($R.Switches) { $R.Switches } else { '(launcher as is)' })``  ")
    $o.Add("Pictures: $($R.Pictures) $($R.PictureNote)  ")
    if ($R.Noisy) { $o.Add("Noisy: $($R.NoisyWhy -join '; ')  ") }
    if ($R.SwitchMap -and @($R.SwitchMap).Count) {
        $o.Add(''); $o.Add('| Switch | Value | Last switch-map report (per frame) | Flag |'); $o.Add('|---|---|---|---|')
        foreach ($s in $R.SwitchMap) {
            $flag = @(); if ($s.Never) { $flag += '**NEVER ACTED**' }; if ($s.Refused -gt 0) { $flag += "**refused $($s.Refused)** ($($s.Why))" }
            $o.Add("| $($s.Switch -replace '^VICTORY_','') | $($s.Value) | $($s.Last -replace '\|','/') | $($flag -join ', ') |")
        }
    } elseif ($R.Switches) { $o.Add(''); $o.Add('No switch-map lines in the measured stretch (a build without the switch map?).') }
    if ($R.SwitchMissing -and @($R.SwitchMissing).Count) { $o.Add(''); $o.Add("Set but not in the switch map: $(@($R.SwitchMissing) -join ', ')") }
    if ($R.SwitchUnmapped) { $o.Add(''); $o.Add("Set, no counter (from the game): $($R.SwitchUnmapped)") }
    if ($R.PSObject.Properties.Name -contains 'TexLine' -and $R.TexLine) {
        $o.Add(''); $o.Add("Texture problems (last report)$(if ($R.TexFlags) { ', **flagged: ' + $R.TexFlags + '**' }): $($R.TexLine)")
    }
    if ($R.PSObject.Properties.Name -contains 'WaitLine' -and $R.WaitLine) {
        $o.Add(''); $o.Add("Card waits (last report$(if (Test-Path (Join-Path $R.Dir 'card-waits.txt')) { '; every wait in card-waits.txt' })): $($R.WaitLine -replace '\|','/')")
    }
    if ($R.PSObject.Properties.Name -contains 'Accounting' -and @($R.Accounting).Count) {
        $o.Add(''); $o.Add("Card accounting (last $(@($R.Accounting).Count) reports; all in card-accounting.txt):"); $o.Add('')
        foreach ($a in $R.Accounting) { $o.Add("- $($a -replace '\|','/')") }
    }
    if ($R.Phases -and @($R.Phases.PSObject.Properties).Count) {
        $o.Add(''); $o.Add('| Phase (median ms per frame) | ms |'); $o.Add('|---|---|')
        foreach ($p in $R.Phases.PSObject.Properties) { $o.Add("| $($p.Name) | $($p.Value) |") }
    }
    if ($null -ne $R.CpuAllPct) {
        $o.Add('')
        $o.Add("CPU: all cores $($R.CpuAllPct)% on average, $($R.BusyCores) of $(@($R.CpuCores).Count) at 50% or more; game process $(Fmt $R.CpuMsPerFrame) ms of processor time per frame.  ")
        $o.Add("Per core (%): $((@($R.CpuCores) | ForEach-Object -Begin { $i = 0 } -Process { '{0}:{1}' -f $i, $_; $i++ }) -join ' ')  ")
        $o.Add("Memory peaks: working set $($R.PeakWsGB) GB, commit $($R.PeakCommitGB) GB, graphics memory $($R.PeakVramGB) GB dedicated + $($R.PeakSharedGB) GB shared; lowest free RAM $($R.MinFreeRamGB) GB.")
    }
    if ($R.Crash) {
        $o.Add('')
        $o.Add("Crash: $(if ($R.Crash.Exception) { $R.Crash.Exception } else { '(no exception line)' })  ")
        $o.Add("Dump files: $(if (@($R.Crash.Dumps).Count) { @($R.Crash.Dumps) -join ', ' } else { 'none found' }); crashes.log: $(if ($R.Crash.CrashesLog) { $R.Crash.CrashesLog } else { 'none' }); last 40 log lines in $($R.Dir)\crash.txt")
    }
    $o.Add('')
    return $o
}

# ------------------------------------------------------------------ the table

function Write-Table {
    $rows = @($Results.Values | Sort-Object { $i = [array]::IndexOf($names, ($_.Name -replace '-(again|long)$', '')); if ($i -lt 0) { $i = 999 }; 2 * $i + [int]($_.Name -match '-(again|long)$') })
    $csv = Join-Path $PlanDir 'results.csv'
    $rows | Select-Object Name, Build, Sets, Status, FrameMs, Fps, MedianMs, PrepMs, GpuMs, CopyBackMs, WaitMs, FreeMbMin, LongPct, VsBase, Helped, Pictures, Noisy, CpuAllPct, BusyCores, CpuMsPerFrame, PeakWsGB, PeakCommitGB, PeakVramGB, PeakSharedGB, MinFreeRamGB, SwitchFlags, @{ n = 'NoisyWhy'; e = { $_.NoisyWhy -join '; ' } }, Note, Switches, PictureNote, Dir |
        Export-Csv -Path $csv -NoTypeInformation -Encoding UTF8
    $md = New-Object System.Collections.Generic.List[string]
    $md.Add("# Test plan $planName")
    $md.Add('')
    $noiseFile = Join-Path $PlanDir 'picture-noise.txt'
    $picLimits = if (Test-Path $noiseFile) { "calibrated from $($Calibrate -join ' vs ') (picture-noise.txt)" } else { 'fixed defaults, no calibration run: detail +-3%, light 2%, colour 2.5%, tile similarity drop 0.25 or tile light 6/255, black area 0.3%, regions in 15% of pictures (CHECK above, FAIL above twice)' }
    $md.Add("Picture limits: $picLimits.")
    $md.Add('')
    if ($EarlyStop -gt 0) { $md.Add("Early stop: a Try run $EarlySlowerPct% or more slower than the best stack after $EarlyStop s (reports from $($EarlyStop / 2) s on) is stopped and not kept; its numbers are from that stretch.") ; $md.Add('') }
    if ($CloseCallPct -gt 0) { $md.Add("Close calls: Try runs within +-$CloseCallPct% of the stack before them and shorter than $RerunSeconds s ran again at $RerunSeconds s (name-long); 'best' follows the rerun.") ; $md.Add('') }
    $md.Add("Started $($script:PlanStart). Each run: its own Seconds (else $RunSeconds s) after the game world started; numbers are medians of the frame-log reports after the first $SkipSeconds s. Frame = usual frame (long frames left out), fps = all frames / time. GPU = frame minus card idle. vs base = frame against $Baseline (negative = faster). Close = within $MarginPct%.")
    $md.Add('')
    $md.Add('| Run | Build | Sets | s | Status | Frame ms | fps | vs base | Prep ms | GPU ms | Copy-back ms | Waits ms/f | Min free MB | Long % | CPU % (cores >=50%) | Peak RAM / VRAM GB | Flags | Pictures | Noisy | Note |')
    $md.Add('|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    foreach ($r in $rows) {
        $vs = if ($null -ne $r.VsBase) { '{0:+0.0;-0.0}%' -f $r.VsBase } else { '-' }
        $helped = if ($r.Helped -eq $true) { ' (kept)' } else { '' }
        $nz = if ($r.Noisy) { 'yes: ' + ($r.NoisyWhy -join '; ') } else { '' }
        if ($r.Name -eq $PicRef) { $r.Pictures = 'reference' }
        $md.Add("| $($r.Name)$helped | $($r.Build) | $($r.Sets) | $(if ($r.PSObject.Properties.Name -contains 'Seconds') { $r.Seconds }) | $($r.Status) | $(Fmt $r.FrameMs) | $($r.Fps) | $vs | $(Fmt $r.PrepMs) | $(Fmt $r.GpuMs) | $(Fmt $r.CopyBackMs) | $(if ($r.PSObject.Properties.Name -contains 'WaitMs') { if ($null -ne $r.WaitMs) { '{0:N1}' -f $r.WaitMs } else { '-' } }) | $(Fmt $r.FreeMbMin) | $($r.LongPct) | $(if ($null -ne $r.CpuAllPct) { "$($r.CpuAllPct) ($($r.BusyCores))" }) | $(if ($null -ne $r.PeakWsGB) { "$($r.PeakWsGB) / $($r.PeakVramGB)" }) | $($r.SwitchFlags) | $($r.Pictures) | $nz | $($r.Note -replace '\|','/') |")
    }
    $md.Add('')
    $md.Add("Best stack at the end: $(if ($BestSets.Count) { $BestSets -join ' + ' } else { '(none helped)' })")
    $md.Add('')
    $md.Add('Picture check: PASS = no visual difference found against the main build; CHECK = look at the report; FAIL = a visual change (see <run>\pictures\report.html).')
    $md.Add('')
    $md.Add('## Run details')
    $md.Add('')
    foreach ($r in $rows) { foreach ($l in (Get-RunDetails $r)) { $md.Add($l) } }
    $md | Set-Content (Join-Path $PlanDir 'results.md') -Encoding UTF8
    $rows | Format-Table Name, Build, Status, @{ n = 'Frame'; e = { Fmt $_.FrameMs } }, Fps, @{ n = 'vsBase'; e = { if ($null -ne $_.VsBase) { '{0:+0.0;-0.0}%' -f $_.VsBase } } }, @{ n = 'Prep'; e = { Fmt $_.PrepMs } }, @{ n = 'GPU'; e = { Fmt $_.GpuMs } }, Pictures, Noisy, Sets -AutoSize | Out-String | Write-Host
}

# Picture noise from two runs of the same build, as soon as both are done;
# every run so far is then compared again with the new limits.
function Update-Calibration {
    $noise = Join-Path $PlanDir 'picture-noise.txt'
    if ($script:Calibrated -or $Calibrate.Count -ne 2 -or -not $script:Piccheck) { return }
    if (-not ($Results.Contains($Calibrate[0]) -and $Results.Contains($Calibrate[1]))) { return }
    $a = $Results[$Calibrate[0]].Pics; $b2 = $Results[$Calibrate[1]].Pics
    & { $ErrorActionPreference = 'Continue'; & $script:Piccheck calibrate --ref $a --test $b2 --out $noise 2>&1 | ForEach-Object { "$_" -replace '^System\.Management\.Automation\.RemoteException$', '' } | Out-File (Join-Path $PlanDir 'picture-noise.log') }
    $script:Calibrated = $true
    if (Test-Path $noise) {
        Say "picture limits set from $($Calibrate -join ' vs ') (see picture-noise.txt); comparing the runs so far again"
        foreach ($r in $Results.Values) { Compare-Pictures $r; Save-Result $r }
    } else { Say "picture calibration failed (see picture-noise.log); default limits stay" 'Yellow' }
}

function Save-Result($r) {
    $r | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $r.Dir 'result.json') -Encoding UTF8
    Get-RunDetails $r | Set-Content (Join-Path $r.Dir 'details.md') -Encoding UTF8
}

# ------------------------------------------------------------------ check

$names = @($AllRuns | ForEach-Object { $_.Name })
foreach ($run in $AllRuns) {
    if (-not $Builds.ContainsKey($run.Build)) { throw "run $($run.Name): unknown build '$($run.Build)'" }
    foreach ($s in (Get-OwnSets $run)) { if (-not $Sets.ContainsKey($s)) { throw "run $($run.Name): unknown set '$s'" } }
}
if ($Only) { foreach ($o in $Only) { if ($names -notcontains $o) { throw "no run named $o in the plan" } } }

if ($Check) {
    Write-Host "Plan $planName ($Plan), results would go to $PlanDir" -ForegroundColor Cyan
    foreach ($k in $Builds.Keys) {
        $l = $Builds[$k].Launcher
        $ok = Test-Path $l
        Write-Host ("build {0}: {1} {2}" -f $k, $l, $(if ($ok) { 'found' } else { 'MISSING' })) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
        if ($ok) {
            $c = New-LauncherCopy $l ([ordered]@{ VICTORY_DATA_DIR = 'X' }) 'check'
            Write-Host ("  the game/warm-up start line found {0} time(s); switches are set there and after the param block" -f $c.StartLines) -ForegroundColor $(if ($c.StartLines -gt 0) { 'Green' } else { 'Yellow' })
            $launcherSets = Select-String -Path $l -Pattern '\$env:(VICTORY_DATA_DIR|VICTORY_CACHE_DIR|VICTORY_CAPTURE\w*)\s*=' | ForEach-Object { $_.Line.Trim() }
            foreach ($s in $launcherSets) { Write-Host "  launcher sets: $s (the runner sets its own value right before the start)" }
            Remove-Item -Force $c.Path
        }
    }
    $saveOk = $SaveSource -and (Test-Path $SaveSource)
    Write-Host ("test save: {0} {1}" -f $SaveSource, $(if ($saveOk) { 'found' } else { 'MISSING' })) -ForegroundColor $(if ($saveOk) { 'Green' } else { 'Red' })
    if ($saveOk) {
        $isData = @(Get-ChildItem -Path $SaveSource -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'storage') }).Count -gt 0
        $size = (Get-ChildItem -Path $SaveSource -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB
        Write-Host ("  layout: {0}, {1:N0} MB copied per run" -f $(if ($isData) { 'data folder (<title id>\storage)' } else { 'storage folder (title id taken from RealDataDirs)' }), $size)
    }
    foreach ($d in $RealDataDirs) { Write-Host ("real data folder (never used, checked after each run): {0} {1}" -f $d, $(if (Test-Path $d) { 'found' } else { 'missing' })) }
    $pc = if ($PictureMode -ne 'off') { Get-Setting 'PiccheckExe' '' } else { '' }
    Write-Host "pictures: $PictureMode (every $PictureEvery frames in the world)"
    $t = 0
    foreach ($run in $AllRuns) {
        if ($Only -and $Only -notcontains $run.Name) { continue }
        $e = Expand-Env $run
        $t += $(if ($run.ContainsKey('Seconds')) { [int]$run.Seconds } else { $RunSeconds }) + 180
        Write-Host ("  {0,-8} {1,-5} {2}" -f $run.Name, $run.Build, (($e.Env.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
    }
    Write-Host ("about {0:N0} min plus warm-ups ('best' stacks show only what is known now)" -f ($t / 60))
    return
}

# ------------------------------------------------------------------ main

New-Item -ItemType Directory -Force -Path $PlanDir | Out-Null
Copy-Item $Plan (Join-Path $PlanDir 'plan.psd1') -Force
$script:PlanStart = Get-Date -Format 'yyyy-MM-dd HH:mm'
Say "plan $planName -> $PlanDir" 'Cyan'

# Switches left in this window from earlier tries would leak into every run.
foreach ($v in Get-ChildItem env: | Where-Object { $_.Name -like 'VICTORY_*' }) {
    Say "removing $($v.Name)=$($v.Value) from this window's environment (runs set their own)" 'Yellow'
    Remove-Item "env:$($v.Name)"
}
if (@(Get-GameProcesses).Count -gt 0) { throw "the game is running; close it first" }
if (Test-Path $NoisyFlag) { Remove-Item $NoisyFlag }
$script:RealSaveStamp = Get-RealSaveStamp
$script:Piccheck = if ($PictureMode -ne 'off') { Get-Piccheck } else { $null }
$script:RefPics = $ReferencePics
if ($ReferencePics) { Say "pictures are compared with $ReferencePics" }

# Earlier results of a resumed plan.
if ($Resume) {
    $done = @(Get-ChildItem -Path $PlanDir -Filter result.json -Recurse -ErrorAction SilentlyContinue | ForEach-Object { Get-Content $_.FullName -Raw | ConvertFrom-Json } |
        Sort-Object { $i = [array]::IndexOf($names, ($_.Name -replace '-(again|long)$', '')); if ($i -lt 0) { 999 } else { $i } })
    foreach ($o in $done) {
        $Results[$o.Name] = $o
        if ($o.Helped -eq $true) { $own = $AllRuns | Where-Object { $_.Name -eq $o.Name } | Select-Object -First 1; foreach ($s in (Get-OwnSets $own)) { if (-not $BestSets.Contains($s)) { $BestSets.Add($s) } }; $BestFrame = $o.FrameMs }
        if ($o.Name -eq $Baseline -and -not $BestFrame) { $BestFrame = $o.FrameMs }
        if ($o.Name -eq $PicRef -and -not $script:RefPics) { $script:RefPics = $o.Pics }
    }
    Say "resuming: $($Results.Count) run(s) already done"
}

$warmed = @{}
try {
    foreach ($run in $AllRuns) {
        if ($Only -and $Only -notcontains $run.Name) { continue }
        if ($Results.Contains($run.Name) -and $Results[$run.Name].Status -eq 'OK' -and -not ($Only -and $Only -contains $run.Name)) { continue }
        if ($run.ContainsKey('OnlyIfCrashed')) {
            $dep = $Results[$run.OnlyIfCrashed]
            if (-not $dep -or $dep.Status -eq 'OK') { Say "$($run.Name): skipped ($($run.OnlyIfCrashed) did not crash)"; continue }
        }
        $e = Expand-Env $run
        # Warm-up: once per build and per value of the switches that change the shader cache.
        if (-not $NoWarmup) {
            $shaderEnv = [ordered]@{}
            foreach ($k in $ShaderSwitches) { if ($e.Env.Contains($k)) { $shaderEnv[$k] = $e.Env[$k] } }
            $key = $run.Build + '|' + (($shaderEnv.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ',')
            if (-not $warmed.ContainsKey($key)) {
                $tag = $run.Build + $(if ($shaderEnv.Count) { '-' + $run.Name } else { '' })
                Invoke-Warmup $run.Build $e.Env $tag
                $warmed[$key] = $true
            }
        }
        $r = Invoke-Run $run $run.Name $e
        if ($run.Name -eq $PicRef -and $r.Status -eq 'OK' -and -not $ReferencePics) { $script:RefPics = $r.Pics }
        Compare-Pictures $r
        # Against the baseline, and whether a Try run helped the stack so far.
        if ($run.Name -eq $Baseline -and $r.FrameMs) { $BestFrame = $r.FrameMs }
        $base = if ($Results.Contains($Baseline)) { $Results[$Baseline].FrameMs } elseif ($run.Name -eq $Baseline) { $r.FrameMs } else { $null }
        if ($base -and $r.FrameMs) { $r.VsBase = [Math]::Round(100.0 * ($r.FrameMs - $base) / $base, 1) }
        if ($run.ContainsKey('Try') -and $run.Try -and $r.Status -eq 'OK' -and $r.FrameMs -and $BestFrame) {
            $gain = 100.0 * ($BestFrame - $r.FrameMs) / $BestFrame
            $r | Add-Member -NotePropertyName ComparedTo -NotePropertyValue $BestFrame -Force
            if ($gain -gt $MarginPct -and $r.Pictures -ne 'FAIL') {
                $r.Helped = $true
                foreach ($s in (Get-OwnSets $run)) { if (-not $BestSets.Contains($s)) { $BestSets.Add($s) } }
                $BestFrame = $r.FrameMs
                Say ("{0} helped ({1:N1}% faster than the stack so far): kept. Best stack: {2}" -f $run.Name, $gain, ($BestSets -join ' + ')) 'Green'
            } else {
                $r.Helped = $false
                Say ("{0}: {1:N1}% against the stack so far{2}; not kept" -f $run.Name, $gain, $(if ($r.Pictures -eq 'FAIL') { ', picture FAIL' } else { '' }))
            }
        } elseif ($r.Status -eq 'STOPPED EARLY') { $r.Helped = $false }
        $Results[$run.Name] = $r
        Save-Result $r
        Update-Calibration
        Write-Table
    }

    Update-Calibration

    # Close calls: Try runs within CloseCallPct of the stack before them that
    # ran shorter than RerunSeconds run again at RerunSeconds; 'best' follows the rerun.
    $rerunLong = @()
    if ($CloseCallPct -gt 0 -and -not $NoRerun) {
        $close = @($Results.Values | Where-Object {
                $_.Status -eq 'OK' -and $_.Name -notmatch '-(again|long)$' -and ($_.PSObject.Properties.Name -contains 'ComparedTo') -and $_.ComparedTo -and
                $_.Seconds -and $_.Seconds -lt $RerunSeconds -and [Math]::Abs(100.0 * ($_.ComparedTo - $_.FrameMs) / $_.ComparedTo) -le $CloseCallPct
            } | Sort-Object { [array]::IndexOf($names, $_.Name) })
        foreach ($old in $close) {
            $run = $AllRuns | Where-Object { $_.Name -eq $old.Name } | Select-Object -First 1
            Say ("{0} was a close call ({1:+0.0;-0.0}% against the stack before it, {2} s): running it again at {3} s" -f $old.Name, (100.0 * ($old.ComparedTo - $old.FrameMs) / $old.ComparedTo), $old.Seconds, $RerunSeconds) 'Cyan'
            $e = @{ Env = [ordered]@{}; Sets = @($old.Sets -split '\+') }
            foreach ($kv in ($old.Switches -split ' ')) { if ($kv -match '^(.+?)=(.*)$') { $e.Env["VICTORY_$($matches[1])"] = $matches[2] } }
            $r = Invoke-Run $run "$($old.Name)-long" $e $RerunSeconds
            Compare-Pictures $r
            $base = if ($Results.Contains($Baseline)) { $Results[$Baseline].FrameMs } else { $null }
            if ($base -and $r.FrameMs) { $r.VsBase = [Math]::Round(100.0 * ($r.FrameMs - $base) / $base, 1) }
            $r | Add-Member -NotePropertyName ComparedTo -NotePropertyValue $old.ComparedTo -Force
            $gain = if ($r.FrameMs) { 100.0 * ($old.ComparedTo - $r.FrameMs) / $old.ComparedTo } else { $null }
            $helpedNow = $r.Status -eq 'OK' -and $null -ne $gain -and $gain -gt $MarginPct -and $r.Pictures -ne 'FAIL'
            $r.Helped = $helpedNow
            $own = @(Get-OwnSets $run)
            if ($helpedNow -and -not $old.Helped) {
                foreach ($s in $own) { if (-not $BestSets.Contains($s)) { $BestSets.Add($s) } }
                Say ("{0}-long: {1:N1}% faster than the stack before it: now kept. Best stack: {2}" -f $old.Name, $gain, ($BestSets -join ' + ')) 'Green'
            } elseif (-not $helpedNow -and $old.Helped) {
                # Out of best, except sets another kept run also brought in.
                $others = @($Results.Values | Where-Object { $_.Helped -eq $true -and $_.Name -ne $old.Name -and $_.Name -notmatch '-(again|long)$' } |
                    ForEach-Object { $n = $_.Name; Get-OwnSets ($AllRuns | Where-Object { $_.Name -eq $n } | Select-Object -First 1) })
                foreach ($s in $own) { if ($others -notcontains $s) { [void]$BestSets.Remove($s) } }
                Say ("{0}-long: {1} against the stack before it: taken out of best. Best stack: {2}" -f $old.Name, $(if ($null -ne $gain) { '{0:N1}%' -f $gain } else { $r.Status }), ($BestSets -join ' + ')) 'Yellow'
            } else {
                Say ("{0}-long: {1}; decision unchanged ({2})" -f $old.Name, $(if ($null -ne $gain) { '{0:N1}%' -f $gain } else { $r.Status }), $(if ($helpedNow) { 'kept' } else { 'not kept' }))
            }
            $old.Helped = $false
            $old.Note = ($old.Note + " | close call: decided by $($old.Name)-long").Trim(' |')
            Save-Result $old
            $rerunLong += $old.Name
            $Results[$r.Name] = $r
            Save-Result $r
            Write-Table
        }
    }

    # Noisy runs whose result is close: once more, at the end.
    if (-not $NoRerun) {
        $again = @($Results.Values | Where-Object {
                $_.Noisy -and $_.Status -eq 'OK' -and $_.Name -notmatch '-(again|long)$' -and $rerunLong -notcontains $_.Name -and (
                    ($null -ne $_.VsBase -and [Math]::Abs($_.VsBase) -lt 2 * $MarginPct) -or
                    (($_.PSObject.Properties.Name -contains 'ComparedTo') -and $_.ComparedTo -and [Math]::Abs(100.0 * ($_.ComparedTo - $_.FrameMs) / $_.ComparedTo) -lt 2 * $MarginPct))
            })
        if ($Noisy -and $again.Count) {
            # The whole plan was marked noisy: a rerun now would be noisy too.
            Say ("noisy and close, run again on a quiet PC: .\Run-TestPlan.ps1 -Resume '{0}' -Only {1}" -f $PlanDir, (($again | ForEach-Object { $_.Name }) -join ',')) 'Yellow'
            $again = @()
        }
        foreach ($old in $again) {
            $run = $AllRuns | Where-Object { $_.Name -eq $old.Name } | Select-Object -First 1
            Say "$($old.Name) was noisy and close: running it again" 'Cyan'
            # The same switches as the first time (the best stack may have grown since).
            $e = @{ Env = [ordered]@{}; Sets = @($old.Sets -split '\+') }
            foreach ($kv in ($old.Switches -split ' ')) { if ($kv -match '^(.+?)=(.*)$') { $e.Env["VICTORY_$($matches[1])"] = $matches[2] } }
            $r = Invoke-Run $run "$($old.Name)-again" $e
            Compare-Pictures $r
            $base = if ($Results.Contains($Baseline)) { $Results[$Baseline].FrameMs } else { $null }
            if ($base -and $r.FrameMs) { $r.VsBase = [Math]::Round(100.0 * ($r.FrameMs - $base) / $base, 1) }
            $Results[$r.Name] = $r
            Save-Result $r
            Write-Table
        }
    }
} finally {
    foreach ($p in Get-ChildItem -Path (Split-Path $Builds[@($Builds.Keys)[0]].Launcher) -Filter ".testbench-*-$PID.ps1" -ErrorAction SilentlyContinue) { Remove-Item -Force $p.FullName -ErrorAction SilentlyContinue }
    if ($Results.Count) { Write-Table }
    Say "results: $(Join-Path $PlanDir 'results.md')" 'Cyan'
}
