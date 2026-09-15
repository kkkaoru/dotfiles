#!/usr/bin/env bash
# Offline tests: no real Executor, launchctl, network, credentials or home writes.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
tmp="$(cd "$tmp" && pwd -P)"
trap 'rm -rf "$tmp"' EXIT
fixture="$tmp/other mac/dotfiles"
export HOME="$tmp/home" TEST_STATE="$tmp/state"
mkdir -p "$fixture/scripts" "$fixture/tools/executor-skills" "$fixture/.executor" "$HOME" "$TEST_STATE" "$tmp/bin"
cp "$repo/scripts/setup-executor.sh" "$fixture/scripts/"
cp "$repo/tools/executor-skills/integrations.json" "$fixture/tools/executor-skills/"
cp "$repo/.mcp.json" "$fixture/"
ln -s "$fixture/.executor" "$HOME/.executor"
export PATH="$tmp/bin:$PATH"
for command in bun bunx ctx; do
  printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/$command"
  chmod +x "$tmp/bin/$command"
done
printf '#!/bin/sh\nprintf "Darwin\\n"\n' > "$tmp/bin/uname"
chmod +x "$tmp/bin/uname"
cat > "$fixture/scripts/executor" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TEST_STATE/calls"
printf '%s' "$EXECUTOR_SCOPE_DIR" > "$TEST_STATE/scope"
case "$*" in
  --version) echo 'executor v1.6.8';;
  'service install --port '*) printf '%s' "$*" > "$TEST_STATE/service";;
  'tools integrations'*)
    case "${TEST_MODE:-}" in
      transport-error) exit 3;;
      bad-catalog) echo '{"ok":false}'; exit;;
      truncated) echo '{"items":[],"hasMore":true}'; exit;;
    esac
    if [[ -f "$TEST_STATE/registrations" ]]; then
      jq -s '{items:map({id:.slug}),hasMore:false}' "$TEST_STATE/registrations"
    else
      echo '{"items":[],"hasMore":false}'
    fi;;
  'call executor mcp addServer '*)
    if [[ "${TEST_MODE:-}" == paused* ]]; then
      printf 'Execution paused\nexecutionId: test-registration\n'
    else
      printf '%s\n' "$5" >> "$TEST_STATE/registrations"
      echo '{"ok":true}'
    fi;;
  'call executor coreTools connections list {}')
    if [[ "${TEST_MODE:-}" == bad-connections ]]; then echo '{"ok":false}'; exit; fi
    if [[ -f "$TEST_STATE/connections" ]]; then
      jq -s '{ok:true,data:{connections:.}}' "$TEST_STATE/connections"
    else
      echo '{"ok":true,"data":{"connections":[]}}'
    fi;;
  'call executor coreTools connections create '*)
    printf '%s\n' "$6" >> "$TEST_STATE/connections"
    echo '{"ok":true}' ;;
  'resume --execution-id test-registration --action accept --content {}')
    if [[ "${TEST_MODE:-}" == paused-nested ]]; then
      printf 'Execution paused\nexecutionId: nested\n'
    else
      echo '{"ok":false,"error":{"message":"mock refusal"}}'
    fi;;
  *) printf 'Unexpected mock command: %s\n' "$*" >&2; exit 99;;
esac
MOCK
chmod +x "$fixture/scripts/executor"
run() { bash "$fixture/scripts/setup-executor.sh" "$@" > "$tmp/output" 2>&1; }
fail() { printf 'FAIL: %s\n' "$*" >&2; tail -n 30 "$tmp/output" >&2; exit 1; }
# Another username/repo path, including spaces, must be used in the saved commands.
run --install-service --port 4790 || fail 'fresh setup'
[[ "$(< "$TEST_STATE/scope")" == "$fixture" ]] || fail 'scope'
[[ "$(< "$TEST_STATE/service")" == 'service install --port 4790' ]] || fail 'service port'
jq -se --arg root "$fixture" --arg home "$HOME" 'any(.[]; .slug == "local-skills" and .args == [($root+"/tools/executor-skills/cli.mjs"),($home+"/.agents/skills"),($home+"/.pi/agent/skills"),($root+"/.agents/skills"),($home+"/.agents/skills-stroage")])' "$TEST_STATE/registrations" >/dev/null || fail 'portable paths'
! grep -q '/Users/kkk4oru/' "$TEST_STATE/registrations" || fail 'hardcoded home'
registered="$(wc -l < "$TEST_STATE/registrations")"
connected="$(wc -l < "$TEST_STATE/connections")"
run || fail 'repeat setup'
[[ "$(wc -l < "$TEST_STATE/registrations")" == "$registered" ]] || fail 'duplicate registration'
[[ "$(wc -l < "$TEST_STATE/connections")" == "$connected" ]] || fail 'duplicate connection'
for mode in transport-error bad-catalog truncated bad-connections; do
  export TEST_MODE="$mode"
  if run; then fail "accepted $mode"; fi
done
unset TEST_MODE
for args in '--port' '--port 0 --install-service' '--port 65536 --install-service' '--port 4789' '--unknown'; do
  # Intentional word splitting: these are fixed test arguments, never user input.
  if run $args; then fail "accepted $args"; fi
done
# Real home directories must not be migrated or overwritten automatically.
rm "$HOME/.executor"
mkdir "$HOME/.executor"
if run --install-service; then fail 'accepted real home'; fi
rmdir "$HOME/.executor"
ln -s "$fixture/.executor" "$HOME/.executor"
rm "$TEST_STATE/registrations" "$TEST_STATE/connections"
export TEST_MODE=paused
: > "$TEST_STATE/calls"
if run; then fail 'accepted unapproved registration'; fi
! grep -q '^resume ' "$TEST_STATE/calls" || fail 'implicit approval'
if run --approve-registration; then fail 'accepted resume tool error'; fi
grep -q '^resume --execution-id test-registration ' "$TEST_STATE/calls" || fail 'explicit approval not used'
export TEST_MODE=paused-nested
: > "$TEST_STATE/calls"
if run --approve-registration; then fail 'accepted nested approval'; fi
[[ "$(grep -c '^resume ' "$TEST_STATE/calls")" == 1 ]] || fail 'nested auto-approval'
unset TEST_MODE
# Test the real wrapper via both direct and relative symlink invocation.
# Replace only its fixed application binary with a local mock, not live config.
awk -v binary="$tmp/bin/native-executor" '/^binary=/{print "binary=\"" binary "\""; next} {print}' "$repo/scripts/executor" > "$fixture/scripts/executor"
cat > "$tmp/bin/native-executor" <<'MOCK'
#!/bin/sh
printf '%s\n' "$EXECUTOR_SCOPE_DIR"
printf '%s\n' "$@"
MOCK
chmod +x "$tmp/bin/native-executor" "$fixture/scripts/executor"
mkdir -p "$fixture/bin"
ln -s ../scripts/executor "$fixture/bin/executor"
unset EXECUTOR_SCOPE_DIR
(cd / && "$fixture/bin/executor" tools integrations) > "$tmp/wrapper-output"
[[ "$(head -n 1 "$tmp/wrapper-output")" == "$fixture" ]] || fail 'wrapper symlink scope'
EXECUTOR_SCOPE_DIR=/explicit "$fixture/scripts/executor" --version > "$tmp/wrapper-output"
[[ "$(head -n 1 "$tmp/wrapper-output")" == /explicit ]] || fail 'explicit scope override'
printf 'PASS: portable setup, service scope, repeatability, fail-closed errors/approvals, wrapper symlinks\n'
