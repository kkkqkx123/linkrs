# Prints the Linkrs Windows service status for quick diagnostics.
[CmdletBinding()]
param(
    [string]$ServiceName = "linkrs"
)

$ErrorActionPreference = "Continue"

$service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if (-not $service) {
    Write-Host "Service '$ServiceName' is not installed."
    exit 1
}

Write-Host "== service =="
$service | Format-List Name, DisplayName, Status, StartType

Write-Host "== nssm =="
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$nssm = $null
foreach ($dir in @("$scriptDir", "$scriptDir\bin", "$env:ProgramFiles\linkrs", ".")) {
    $candidate = Join-Path $dir "nssm.exe"
    if (Test-Path $candidate) { $nssm = (Resolve-Path $candidate).Path; break }
}
if (-not $nssm) {
    $onPath = Get-Command "nssm.exe" -ErrorAction SilentlyContinue
    if ($onPath) { $nssm = $onPath.Source }
}

if ($nssm) {
    & $nssm status $ServiceName
    & $nssm dump $ServiceName
} else {
    Write-Host "nssm.exe not found; skipping NSSM details."
}

Write-Host "== recent log files =="
$logDir = "$env:ProgramData\linkrs\logs"
if (Test-Path $logDir) {
    Get-ChildItem $logDir -File | Sort-Object LastWriteTime -Descending |
        Select-Object -First 5 Name, Length, LastWriteTime | Format-Table -AutoSize
} else {
    Write-Host "No log directory at $logDir"
}
