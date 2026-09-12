#!/usr/bin/env bash
# Offline wrapper regression: no Executor process, network, home or daemon changes.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/checkout with spaces"
mkdir -p "$repo/scripts" "$repo/.executor" "$repo/tools/executor-sync/node_modules" "$tmp/bin"
printf '{}' > "$repo/.executor/sync.json"
export TEST_CALLS="$tmp/calls" TEST_RESULT=0 TEST_HOOK_RESULT=0
cat > "$tmp/bin/native" <<'MOCK'
#!/bin/sh
printf 'native\n' >> "$TEST_CALLS"
printf '{"ok":true}\n'
exit "$TEST_RESULT"
MOCK
cat > "$tmp/bin/bun" <<'MOCK'
#!/bin/sh
printf '%s\n' "$3" >> "$TEST_CALLS"
printf 'this hook output must not contaminate JSON\n'
[ "$EXECUTOR_SYNC_ACTIVE" = 1 ] || exit 99
exit "$TEST_HOOK_RESULT"
MOCK
chmod +x "$tmp/bin/native" "$tmp/bin/bun"
export PATH="$tmp/bin:$PATH"
awk -v binary="$tmp/bin/native" '/^binary=/{print "binary=\"" binary "\""; next} {print}' "$root/scripts/executor" > "$repo/scripts/executor"
chmod +x "$repo/scripts/executor"
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
: > "$TEST_CALLS"
"$repo/scripts/executor" tools integrations > "$tmp/output"
[[ "$(< "$tmp/output")" == '{"ok":true}' ]] || fail 'stdout changed'
[[ "$(< "$TEST_CALLS")" == $'before\nnative\nafter' ]] || fail 'hook ordering'
: > "$TEST_CALLS"
export TEST_RESULT=17
if "$repo/scripts/executor" call example > "$tmp/output"; then fail 'native error swallowed'; else code=$?; fi
[[ "$code" == 17 ]] || fail 'native status changed'
[[ "$(< "$TEST_CALLS")" == $'before\nnative\nafter' ]] || fail 'after hook omitted on native failure'
export TEST_RESULT=0 TEST_HOOK_RESULT=1
"$repo/scripts/executor" tools integrations > "$tmp/output" 2>/dev/null || fail 'hook error broke local command'
export TEST_HOOK_RESULT=0
: > "$TEST_CALLS"
EXECUTOR_SYNC_ACTIVE=1 "$repo/scripts/executor" tools integrations > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == native ]] || fail 'recursive hooks'
: > "$TEST_CALLS"
EXECUTOR_SCOPE_DIR=/different "$repo/scripts/executor" tools integrations > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == native ]] || fail 'foreign scope hooked'
: > "$TEST_CALLS"
"$repo/scripts/executor" tools integrations --server remote > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == native ]] || fail 'remote server hooked'
: > "$TEST_CALLS"
EXECUTOR_DATA_DIR=/other "$repo/scripts/executor" tools integrations > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == native ]] || fail 'custom data directory hooked'
: > "$TEST_CALLS"
"$repo/scripts/executor" --version > "$tmp/output"
"$repo/scripts/executor" daemon status > "$tmp/output"
"$repo/scripts/executor" mcp > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == $'native\nnative\nnative' ]] || fail 'lifecycle intercepted'
rm "$repo/.executor/sync.json"
: > "$TEST_CALLS"
"$repo/scripts/executor" tools integrations > "$tmp/output"
[[ "$(< "$TEST_CALLS")" == native ]] || fail 'unconfigured checkout hooked'
printf 'PASS: hooks preserve output/status, prevent recursion, skip lifecycle and add no daemon\n'
