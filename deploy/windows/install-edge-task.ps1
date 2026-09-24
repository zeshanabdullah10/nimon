#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Run nimon-edge at boot as a Scheduled Task (or remove it).

.DESCRIPTION
    nimon-edge has no Windows service mode. This script registers a
    Scheduled Task "NIMonEdge" that starts
        <InstallDir>\nimon-edge.exe <InstallDir>\config\edge.yaml
    at system startup as SYSTEM, from <InstallDir> (so the default
    logging.file ./logs/nimon-edge.log and scripts_dir ./scripts are under
    it), restarting it if it exits with an error.

    Stopping the task terminates the process (no graceful shutdown). For a
    real service with Ctrl-C shutdown use NSSM instead (see README).

    The hub token (node.hub_token) lives in config\edge.yaml; this script
    limits the config directory to SYSTEM and Administrators.

.EXAMPLE
    .\deploy\windows\install-edge-task.ps1 -Source .\target\release\nimon-edge.exe -Config .\config\edge.yaml

.EXAMPLE
    .\deploy\windows\install-edge-task.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [string]$Source = ".\target\release\nimon-edge.exe",
    [string]$InstallDir = "$env:ProgramFiles\NIMon\edge",
    # edge.yaml to install as <InstallDir>\config\edge.yaml (required on first install)
    [string]$Config,
    [string]$TaskName = "NIMonEdge",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$Exe = Join-Path $InstallDir "nimon-edge.exe"
$ConfigPath = Join-Path $InstallDir "config\edge.yaml"

$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($Uninstall) {
    if ($task) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "Task $TaskName removed. Files in $InstallDir were kept."
    } else {
        Write-Host "Task $TaskName is not registered."
    }
    return
}

if (-not (Test-Path $Source)) { throw "nimon-edge.exe not found at $Source (build with: cargo build --release -p nimon-edge)" }
if ($task) { Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2 }

New-Item -ItemType Directory -Force -Path (Join-Path $InstallDir "config") | Out-Null
Copy-Item -Force $Source $Exe
if ($Config) { Copy-Item -Force $Config $ConfigPath }
if (-not (Test-Path $ConfigPath)) { throw "No config at ${ConfigPath}: pass -Config path\to\edge.yaml" }

& icacls.exe (Join-Path $InstallDir "config") /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" | Out-Null

$action = New-ScheduledTaskAction -Execute $Exe -Argument "`"$ConfigPath`"" -WorkingDirectory $InstallDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "Task $TaskName registered and started ($Exe `"$ConfigPath`")"
Write-Host "Logs: $InstallDir\logs (per logging.file in edge.yaml)"
