param()
# Serve my-win's Strata in the foreground of the StrataStart scheduled task, so the task owns the
# server's lifetime instead of an SSH session (a launched instance dies with its parent).
# The run config is the one `strata launch` wrote (model, --max-context, --kv, port).
$ErrorActionPreference = 'Stop'
$strataHome = Join-Path $env:LOCALAPPDATA 'Strata'
$strata = Join-Path $strataHome 'terminal\current\bin\strata.ps1'
$profilePath = Join-Path $strataHome 'launch\q2_0-32k.json'
$log = Join-Path $strataHome 'strata-start.log'

if (-not (Test-Path $profilePath)) {
  "$(Get-Date -Format o) missing run config $profilePath" | Out-File -FilePath $log -Append -Encoding utf8
  exit 1
}

"$(Get-Date -Format o) serving $profilePath" | Out-File -FilePath $log -Append -Encoding utf8
& $strata serve --profile $profilePath --port 8080 *>&1 |
  Out-File -FilePath $log -Append -Encoding utf8
