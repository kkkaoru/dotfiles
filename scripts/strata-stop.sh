#!/usr/bin/env bash
# Stop this Mac's Strata instance that scripts/strata-local.sh started (127.0.0.1:8080).
# It holds ~20 GB of RAM and streams a 37.6 GB shard while it runs.
#   STRATA_PROFILE    saved profile name (default: launch-q2_0-32k)
#   STRATA_LOCAL_PORT port that must match .pi/agent/models.json (default: 8080)
set -euo pipefail

profile="${STRATA_PROFILE:-launch-q2_0-32k}"
port="${STRATA_LOCAL_PORT:-8080}"

if ! nc -z 127.0.0.1 "$port" 2>/dev/null; then
  echo "strata not serving on 127.0.0.1:$port"
  exit 0
fi

strata runtime stop --profile "$profile"
