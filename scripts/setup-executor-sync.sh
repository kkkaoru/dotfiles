#!/usr/bin/env bash
# Install dependencies / enter secrets locally; never register another daemon.
set -euo pipefail
set +x
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
case "${1:-}" in
  install)
    command -v bun >/dev/null
    command -v age >/dev/null || { printf 'Install age with: brew install age\n' >&2; exit 1; }
    (cd "$repo/tools/executor-sync" && bun install --frozen-lockfile)
    "$repo/scripts/executor-sync" doctor
    printf 'No daemon installed. Next: ./scripts/setup-executor-sync.sh configure\n'
    ;;
  configure)
    [[ -t 0 ]] || { printf 'Interactive terminal required; automation can pipe the account ID to executor-sync configure.\n' >&2; exit 1; }
    read -r -s -p 'Cloudflare account ID (saved only in Keychain): ' account; printf '\n' >&2
    printf '%s\n' "$account" | "$repo/scripts/executor-sync" configure
    unset account
    ;;
  enable|disable|doctor)
    "$repo/scripts/executor-sync" "$1"
    ;;
  *)
    printf 'Usage: %s install|configure|enable|disable|doctor\n' "$0" >&2
    exit 1
    ;;
esac
