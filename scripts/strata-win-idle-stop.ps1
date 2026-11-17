param(
  [int]$IdleMinutes = 10
)
# Stop my-win's Strata after $IdleMinutes with no request and no client connection: the model
# holds ~26 GB of RAM on a 32 GB machine, so it must not idle forever.
# Registered as the StrataIdleStop scheduled task (every 5 minutes).
$ErrorActionPreference = 'Stop'
$strataHome = Join-Path $env:LOCALAPPDATA 'Strata'
$engineLog = Join-Path $strataHome 'launch\q2_0-32k.engine.log'
$log = Join-Path $strataHome 'strata-idle-stop.log'
$task = 'StrataStart'

function Write-Note([string]$text) {
  "$(Get-Date -Format o) $text" | Out-File -FilePath $log -Append -Encoding utf8
}

# Not serving: nothing to stop.
$listening = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
if (-not $listening) { exit 0 }

# A connected client means it is in use.
$established = Get-NetTCPConnection -LocalPort 8080 -State Established -ErrorAction SilentlyContinue
if ($established) { exit 0 }

# No request has been served since the engine log last changed.
if (Test-Path $engineLog) {
  $idle = (Get-Date) - (Get-Item $engineLog).LastWriteTime
  if ($idle.TotalMinutes -lt $IdleMinutes) { exit 0 }
}

Stop-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue
Write-Note "stopped after $IdleMinutes min idle"
