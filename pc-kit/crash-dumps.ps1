# Windows Error Reporting crash dumps for victory.exe only (Carlos OK'd it 2026-11-26).
# Run in an ADMIN PowerShell:
#   powershell -ExecutionPolicy Bypass -File C:\xcl\crash-dumps.ps1          # turn on
#   powershell -ExecutionPolicy Bypass -File C:\xcl\crash-dumps.ps1 -Undo    # turn off again (deletes only the victory.exe key)
#   powershell -ExecutionPolicy Bypass -File C:\xcl\crash-dumps.ps1 -Mini    # small dumps instead of full ones
# Dumps land in C:\xcl\runs\dumps (at most 2 kept; Windows replaces the oldest).
# A full dump holds the whole process (can be 20-30 GB with RT on): needs free disk space.
# Our own crash handler (victory-loader crash.rs) still writes crash.txt / crash-<rva>.bin first;
# its unhandled-exception filter passes the crash on, so Windows then writes the dump.
param([switch]$Undo, [switch]$Mini)

$key = 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\victory.exe'
$folder = 'C:\xcl\runs\dumps'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this in an administrator PowerShell (HKLM is changed).'
}

if ($Undo) {
    if (Test-Path $key) { Remove-Item -Path $key -Recurse -Force; Write-Host "Removed $key (crash dumps for victory.exe off). Dumps already written stay in $folder." }
    else { Write-Host 'Nothing to undo: the key is not there.' }
    return
}

New-Item -ItemType Directory -Force -Path $folder | Out-Null
$free = (Get-PSDrive -Name ($folder.Substring(0, 1))).Free
$type = 2  # full
if ($Mini) { $type = 1 }
elseif ($free -lt 80GB) {
    Write-Warning ("Only {0:N0} GB free on {1}: using small dumps (-Mini). Free space, then run again for full dumps." -f ($free / 1GB), $folder.Substring(0, 2))
    $type = 1
}
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DumpFolder -PropertyType ExpandString -Value $folder -Force | Out-Null
New-ItemProperty -Path $key -Name DumpCount -PropertyType DWord -Value 2 -Force | Out-Null
New-ItemProperty -Path $key -Name DumpType -PropertyType DWord -Value $type -Force | Out-Null
Write-Host ("Crash dumps for victory.exe on: {0} dumps, at most 2, in {1}. Undo: -Undo." -f $(if ($type -eq 2) { 'full' } else { 'small' }), $folder)
Get-ItemProperty -Path $key | Select-Object DumpFolder, DumpCount, DumpType | Format-List
