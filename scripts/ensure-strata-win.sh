#!/usr/bin/env bash
# Make my-win's Strata reachable from this Mac without touching my-win by hand: ensure its
# `StrataStart` scheduled task is running, then make sure the tunnel is up.
# my-win stops itself after 10 idle minutes (its own StrataIdleStop task), so this is a no-op when
# the server already answers.
#   STRATA_SSH_HOST   SSH alias (default: my-win)
#   STRATA_LOCAL_PORT loopback port that must match .pi/agent/models.json (default: 18080)
set -euo pipefail

host="${STRATA_SSH_HOST:-my-win}"
port="${STRATA_LOCAL_PORT:-18080}"

# Any HTTP answer (even 401 for a key we do not hold) means the server is up.
serving() {
  [ "$(curl -s -o /dev/null -m 5 -w '%{http_code}' "http://127.0.0.1:${port}/v1/models")" != "000" ]
}

if serving; then
  echo "strata-win already serving on 127.0.0.1:$port"
  exit 0
fi

# The task starts the server detached from this SSH session, so it outlives it.
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$host" 'schtasks /run /tn StrataStart' >/dev/null 2>&1; then
  echo "strata-win: cannot run StrataStart on $host" >&2
  exit 1
fi

"$(command -v strata-tunnel)"

for _ in $(seq 1 60); do
  if serving; then
    echo "strata-win serving on 127.0.0.1:$port"
    exit 0
  fi
  sleep 5
done

echo "strata-win: started on $host but 127.0.0.1:$port never answered" >&2
exit 1
