#!/bin/bash
# Remove DeepSWE / Pier eval leftovers. Does not delete oMLX model weights.

set -euo pipefail

SHARE="${HOME}/.local/share/deepswe-omlx"
echo "removing ${SHARE} (jobs, linux pi bundle, logs)"
rm -rf "$SHARE"

if command -v docker >/dev/null 2>&1; then
  docker ps -aq | while read -r id; do
    [ -n "$id" ] || continue
    docker rm -f "$id" >/dev/null 2>&1 || true
  done
  docker images --format '{{.Repository}}:{{.Tag}} {{.ID}}' | while read -r ref image_id; do
    case "$ref" in
      public.ecr.aws/d3j8x8q7/swe-bench-202605:*|*-main:latest|*pier-egress-proxy:latest)
        echo "rmi ${ref}"
        docker image rm -f "$image_id" >/dev/null 2>&1 || true
        ;;
    esac
  done
  docker builder prune -af
  docker container prune -f >/dev/null
  docker image prune -f >/dev/null
fi

echo "done"
docker system df 2>/dev/null || true
