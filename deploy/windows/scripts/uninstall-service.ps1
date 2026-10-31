# Removes the Linkrs Windows service installed by install-service.ps1.
# By default the config, data and log directories are kept; purge switches
# below remove them explicitly.
[CmdletBinding()]
param(
    [string]$NssmPath,
    [string]$ServiceName = "linkrs",
    [switch]$PurgeConfig,
    [switch]$PurgeData
)

$ErrorActionPreference = "Stop"

function Assert-Admin {
    $identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "This script must run elevated (Run as Administrator)."
    }
}

function Find-Nssm {
    param([string]$Explicit)
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    foreach ($dir in @("$scriptDir", "$scriptDir\bin", "$env:ProgramFiles\linkrs", ".")) {
        if (-not $dir) { continue }
        $candidate = Join-Path $dir "nssm.exe"
        if ($Explicit -and $candidate -eq $Explicit) { break }
        if (Test-Path $candidate) { return (Resolve-Path $candidate).Path }
    }
    if ($Explicit) {
        if (-not (Test-Path $Explicit)) { throw "Not found: $Explicit" }
        return (Resolve-Path $Explicit).Path
    }
    $onPath = Get-Command "nssm.exe" -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    throw "nssm.exe not found; pass -NssmPath explicitly."
}

Assert-Admin

if (-not (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)) {
    Write-Host "Service '$ServiceName' is not installed; nothing to do."
    exit 0
}

$nssm = Find-Nssm $NssmPath

& $nssm stop $ServiceName 2>$null
& $nssm remove $ServiceName confirm | Out-Null
Write-Host "Service '$ServiceName' removed"

if ($PurgeConfig) {
    Remove-Item -Recurse -Force "$env:ProgramData\linkrs" -ErrorAction SilentlyContinue
    Write-Host "Removed $env:ProgramData\linkrs"
} elseif ($PurgeData) {
    # Config kept, data and logs removed
    Remove-Item -Recurse -Force "$env:ProgramData\linkrs\data", "$env:ProgramData\linkrs\logs" -ErrorAction SilentlyContinue
    Write-Host "Removed data and log directories"
}

Write-Host "Done. The server binary is left in place (remove it manually if desired)."
