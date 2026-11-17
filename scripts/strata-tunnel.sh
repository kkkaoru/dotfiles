#!/usr/bin/env bash
# Forward Strata's OpenAI-compatible API from the GPU host to a fixed loopback port.
# pi's provider entry (.pi/agent/models.json) points at 127.0.0.1:18080 so the file
# is machine-independent: the target comes from the SSH alias and these variables.
#   STRATA_SSH_HOST    SSH host alias of the Strata machine (default: my-win)
#   STRATA_REMOTE_ADDR address Strata listens on there (default: 127.0.0.1)
#   STRATA_REMOTE_PORT Strata's port there (default: 8080)
#   STRATA_LOCAL_PORT  loopback port here, must match models.json (default: 18080)
set -euo pipefail

host="${STRATA_SSH_HOST:-my-win}"
remote_addr="${STRATA_REMOTE_ADDR:-127.0.0.1}"
remote_port="${STRATA_REMOTE_PORT:-8080}"
local_port="${STRATA_LOCAL_PORT:-18080}"

if nc -z 127.0.0.1 "$local_port" 2>/dev/null; then
  # A listener that forwards nothing (remote restarted or gone) must be replaced, not trusted:
  # nc only proves the local ssh holds the port.
  if curl -fsS -m 5 "http://127.0.0.1:$local_port/v1/models" >/dev/null 2>&1; then
    echo "strata tunnel already up on 127.0.0.1:$local_port"
    exit 0
  fi
  stale=$(lsof -nP -iTCP:"$local_port" -sTCP:LISTEN -t 2>/dev/null || true)
  [ -n "$stale" ] && printf "%s
" "$stale" | xargs kill
  sleep 1
fi

ssh -N -f -o ExitOnForwardFailure=yes -o ConnectTimeout=10 \
  -L "$local_port:$remote_addr:$remote_port" "$host"
echo "strata tunnel up: 127.0.0.1:$local_port -> $host:$remote_addr:$remote_port"
