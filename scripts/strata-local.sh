#!/usr/bin/env bash
# Start this Mac's Strata where pi's `strata` provider expects it (127.0.0.1:8080).
# Nothing starts it in advance: pi's strata-lifecycle extension runs this when a `strata` model is
# selected, and strata-idle-stop stops it again after 10 idle minutes.
#   STRATA_PROFILE    saved profile name (default: launch-q2_0-32k)
#   STRATA_LOCAL_PORT port that must match .pi/agent/models.json (default: 8080)
set -euo pipefail

profile="${STRATA_PROFILE:-launch-q2_0-32k}"
port="${STRATA_LOCAL_PORT:-8080}"

if nc -z 127.0.0.1 "$port" 2>/dev/null; then
  echo "strata already serving on 127.0.0.1:$port"
  exit 0
fi

# A managed instance (not this shell): the engine takes ~30 s to warm up, hence --wait.
strata runtime start --profile "$profile" --wait 300
