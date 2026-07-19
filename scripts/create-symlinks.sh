#!/bin/bash

set -euo pipefail

# Prefer repo-root create-symlinks.sh (this file lives in scripts/)
SCRIPT_DIR=$(cd "$(dirname "$0")" || exit 1; pwd)
ROOT_DIR=$(cd "${SCRIPT_DIR}/.." || exit 1; pwd)

exec "${ROOT_DIR}/create-symlinks.sh"
