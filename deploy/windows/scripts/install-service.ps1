# Installs the Linkrs server as a Windows service via NSSM.
#
# Usage (elevated PowerShell):
#   .\install-service.ps1                          # install, no autostart
#   .\install-service.ps1 -EnableService -StartService
#
# Requirements:
#   - nssm.exe on PATH, in .\bin, next to the script, or via -NssmPath
#   - linkrs-server.exe built (release): look-up order mirrors the binary
#     search below (bin dir, current dir, program dir), or use -ServerPath
#
# Default directories (override with the matching parameters):
#   config:  C:\ProgramData\linkrs\config.toml
#   data:    C:\ProgramData\linkrs\data
#   logs:    C:\ProgramData\linkrs\logs
#   program: C:\Program Files\linkrs
[CmdletBinding()]
param(
    [string]$NssmPath,
    [string]$ServerPath,
    [string]$ConfigDir = "$env:ProgramData\linkrs",
    [string]$DataDir = "$env:ProgramData\linkrs\data",
    [string]$LogDir = "$env:ProgramData\linkrs\logs",
    [string]$ProgramDir = "$env:ProgramFiles\linkrs",
    [string]$ServiceName = "linkrs",
    [switch]$EnableService,
    [switch]$StartService
)

$ErrorActionPreference = "Stop"

function Assert-Admin {
    $identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "This script must run elevated (Run as Administrator)."
    }
}

function Find-Tool {
    param([string]$Name, [string]$Explicit, [string[]]$SearchDirs)
    if ($Explicit) {
        if (-not (Test-Path $Explicit)) { throw "Not found: $Explicit" }
        return (Resolve-Path $Explicit).Path
    }
    foreach ($dir in $SearchDirs) {
        if (-not $dir) { continue }
        $candidate = Join-Path $dir $Name
        if (Test-Path $candidate) { return (Resolve-Path $candidate).Path }
    }
    $onPath = Get-Command $Name -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    throw "$Name not found. Place it in one of the search directories or pass it explicitly."
}

function New-DirIfMissing {
    param([string]$Path)
    if (-not (Test-Path $Path)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
    }
}

Assert-Admin

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$deployRoot = Split-Path -Parent $scriptDir
$defaultConfig = Join-Path $deployRoot "windows\config\linkrs.toml"

$nssm = Find-Tool "nssm.exe" $NssmPath @("$scriptDir", "$scriptDir\bin", $ProgramDir, ".")
Write-Host "Using nssm: $nssm"

$server = Find-Tool "linkrs-server.exe" $ServerPath @("$scriptDir", "$scriptDir\bin", $ProgramDir, ".")
Write-Host "Using server: $server"

New-DirIfMissing $ConfigDir | Out-Null
New-DirIfMissing $DataDir | Out-Null
New-DirIfMissing $LogDir | Out-Null

# Install the default config when none exists yet
$configPath = Join-Path $ConfigDir "config.toml"
if (-not (Test-Path $configPath)) {
    Copy-Item $defaultConfig $configPath
    Write-Host "Installed config template from $defaultConfig"
}

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    & $nssm stop $ServiceName 2>$null
    & $nssm remove $ServiceName confirm | Out-Null
    Write-Host "Removed previous service installation"
}

# NSSM wraps linkrs-server.exe so the console process interacts with the
# Windows Service Control Manager (start/stop) without in-process SCM code.
& $nssm install $ServiceName $server serve --config $configPath
& $nssm set $ServiceName AppDirectory $DataDir
& $nssm set $ServiceName AppStdout (Join-Path $LogDir "service-out.log")
& $nssm set $ServiceName AppStderr (Join-Path $LogDir "service-err.log")
# Rotate the redirected streams: one file, 10 MB each, keep 5, timestamped
& $nssm set $ServiceName AppRotateFiles 1
& $nssm set $ServiceName AppRotateOnline 1
& $nssm set $ServiceName AppRotateBytes 10485760
& $nssm set $ServiceName AppRotateFilesToKeep 5
# Restart on unexpected exit, throttled like the systemd unit
& $nssm set $ServiceName AppExit Default Restart
& $nssm set $ServiceName AppRestartDelay 5000
& $nssm set $ServiceName DisplayName "Linkrs graph database server"
& $nssm set $ServiceName Description "Lightweight single-node graph database service"

if ($EnableService) {
    Set-Service -Name $ServiceName -StartupType Automatic
    Write-Host "Service set to start automatically on boot"
} else {
    Set-Service -Name $ServiceName -StartupType Manual
    Write-Host "Service set to manual start"
}

if ($StartService) {
    Start-Service -Name $ServiceName
    Get-Service -Name $ServiceName | Format-List Name, Status, StartType
}

Write-Host "Done."
Write-Host "Manage with: Get-Service $ServiceName / Start-Service, Stop-Service, Restart-Service"
