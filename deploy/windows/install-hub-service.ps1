#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Install (or remove) nimon-hub as the Windows service "NIMonHub".

.DESCRIPTION
    Copies nimon-hub.exe into -InstallDir and registers it with the Service
    Control Manager through the hub's own subcommands:

        nimon-hub.exe install     registers "<exe> run-service" (auto start,
                                  LocalSystem) and writes config\hub.yaml
                                  next to the exe if it does not exist
        nimon-hub.exe start | stop | uninstall

    The service reads <InstallDir>\config\hub.yaml; relative paths in it are
    resolved against that config directory (the template's
    database_path "../data/nimon.db" -> <InstallDir>\data\nimon.db).

    Secrets (auth.api_token, auth.edge_token, SMTP password) belong in
    config\hub.yaml or a smtp_password_file next to it; this script limits
    the config directory to SYSTEM and Administrators.

.EXAMPLE
    .\deploy\windows\install-hub-service.ps1 -Source .\target\release\nimon-hub.exe -Config .\config\hub.yaml

.EXAMPLE
    .\deploy\windows\install-hub-service.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    # nimon-hub.exe to install
    [string]$Source = ".\target\release\nimon-hub.exe",
    # Installation directory (the service runs from here)
    [string]$InstallDir = "$env:ProgramFiles\NIMon\hub",
    # Optional hub.yaml to install as <InstallDir>\config\hub.yaml
    [string]$Config,
    # RUST_LOG filter for the service process
    [string]$LogLevel = "info",
    # Stop and remove the service (files are left in place)
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$ServiceName = "NIMonHub"
$Exe = Join-Path $InstallDir "nimon-hub.exe"

function Invoke-Hub([string]$Command) {
    & $Exe $Command
    if ($LASTEXITCODE -ne 0) { throw "nimon-hub $Command failed (exit $LASTEXITCODE)" }
}

if ($Uninstall) {
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        try { Invoke-Hub "stop" } catch { Write-Warning $_ }
        Invoke-Hub "uninstall"
        Write-Host "Service $ServiceName removed. Files in $InstallDir were kept."
    } else {
        Write-Host "Service $ServiceName is not installed."
    }
    return
}

if (-not (Test-Path $Source)) { throw "nimon-hub.exe not found at $Source (build with: cargo build --release -p nimon-hub)" }

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    Write-Host "Updating existing service $ServiceName"
    try { Invoke-Hub "stop" } catch { Write-Warning $_ }
    Start-Sleep -Seconds 2
    $existing = $true
} else {
    $existing = $false
}

New-Item -ItemType Directory -Force -Path (Join-Path $InstallDir "config") | Out-Null
Copy-Item -Force $Source $Exe
if ($Config) {
    Copy-Item -Force $Config (Join-Path $InstallDir "config\hub.yaml")
}

if (-not $existing) {
    # Registers "<exe> run-service" and writes config\hub.yaml when missing
    Invoke-Hub "install"
}

# Environment of the service process (REG_MULTI_SZ). Do not put tokens
# here: service registry keys are readable by non-admin users.
$key = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
New-ItemProperty -Path $key -Name "Environment" -PropertyType MultiString `
    -Value @("RUST_LOG=$LogLevel") -Force | Out-Null

# Restart on failure: 5 s, 5 s, then 30 s; reset the counter after a day
& sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/30000 | Out-Null

# Config (may hold tokens / SMTP password file): SYSTEM + Administrators only
$configDir = Join-Path $InstallDir "config"
& icacls.exe $configDir /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" | Out-Null

Invoke-Hub "start"
Write-Host "Service $ServiceName running from $InstallDir"
Write-Host "Config:    $configDir\hub.yaml (restart the service after edits: nimon-hub.exe stop / start)"
Write-Host "Dashboard: http://localhost:9090 (or the port set in hub.yaml)"
