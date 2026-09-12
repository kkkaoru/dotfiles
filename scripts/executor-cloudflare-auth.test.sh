#!/usr/bin/env bash
# Offline integration tests: mock Executor and the browser, never touch live auth.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export EXECUTOR_CLI="$tmp/executor"
export EXECUTOR_OPENER="$tmp/open"
export TEST_TRACE="$tmp/trace"

cat > "$EXECUTOR_CLI" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$TEST_TRACE"
if [[ "$1" == call || "$1" == resume ]]; then
  [[ "$*" == *'--base-url http://localhost:4955' ]]
fi
case "$1 $2" in
  'daemon run') exit ;;
  'daemon status')
    if [[ "$MOCK_MODE" == remote ]]; then
      printf 'Daemon running at http://example.com:4955 (pid 1).\n'
    else
      printf 'Daemon running at http://localhost:4955 (pid 1).\n'
    fi
    exit ;;
  'resume --execution-id')
    if [[ "$MOCK_MODE" == nested ]]; then
      printf 'Execution paused: More permission\nexecutionId: exec_nested\n'
    elif [[ "$3" == exec_start ]]; then
      printf '{"ok":true,"data":{"status":"redirect","authorizationUrl":"https://mcp.cloudflare.com/authorize?state=private-test-state"}}\n'
    else
      printf '{"ok":true,"data":{"client":"server-chosen-client"}}\n'
    fi
    exit ;;
esac
if [[ "$*" == *'mcp getServer'* ]]; then
  slug="$(printf '%s' "$5" | jq -r '.slug')"
  endpoint="https://mcp.cloudflare.com/mcp"
  case "$slug" in
    cloudflare-bindings) endpoint=https://bindings.mcp.cloudflare.com/mcp ;;
    cloudflare-builds) endpoint=https://builds.mcp.cloudflare.com/mcp ;;
    cloudflare-observability) endpoint=https://observability.mcp.cloudflare.com/mcp ;;
    cloudflare-browser) endpoint=https://browser.mcp.cloudflare.com/mcp ;;
    cloudflare-graphql) endpoint=https://graphql.mcp.cloudflare.com/mcp ;;
  esac
  if [[ "$MOCK_MODE" == mismatch ]]; then endpoint=https://example.com/mcp; fi
  jq -nc --arg endpoint "$endpoint" '{ok:true,data:{integration:{config:{endpoint:$endpoint},authMethods:[{kind:"oauth",template:"oauth2"}]}}}'
elif [[ "$*" == *'oauth probe'* ]]; then
  if [[ "$MOCK_MODE" == failed ]]; then
    printf '{"ok":false,"error":{"message":"Discovery failed"}}\n'
    exit
  fi
  if [[ "$MOCK_MODE" == malformed ]]; then printf 'not JSON\n'; exit; fi
  endpoint="https://mcp.cloudflare.com/register"
  if [[ "$MOCK_MODE" == hostile ]]; then endpoint=https://cloudflare.com.evil.example/register; fi
  jq -nc --arg endpoint "$endpoint" '{ok:true,data:{issuer:"https://mcp.cloudflare.com",registrationEndpoint:$endpoint,authorizationUrl:"https://mcp.cloudflare.com/authorize",tokenUrl:"https://mcp.cloudflare.com/token",tokenEndpointAuthMethodsSupported:["none"]}}'
elif [[ "$*" == *'clients registerDynamic'* ]]; then
  printf '%s' "$7" | jq -e '.redirectUri == "http://localhost:4955/api/oauth/callback" and .scopes == [] and .owner == "user"' >/dev/null
  if [[ "$MOCK_MODE" == paused || "$MOCK_MODE" == nested ]]; then
    printf 'Execution paused: Register OAuth client\nexecutionId: exec_register\n'
  else
    printf '{"ok":true,"data":{"client":"server-chosen-client"}}\n'
  fi
elif [[ "$*" == *'oauth start'* ]]; then
  printf '%s' "$6" | jq -e '.client == "server-chosen-client" and .redirectUri == "http://localhost:4955/api/oauth/callback" and .template == "oauth2"' >/dev/null
  url='https://mcp.cloudflare.com/authorize?state=private-test-state'
  if [[ "$MOCK_MODE" == badurl ]]; then url='https://evil.example/authorize'; fi
  if [[ "$MOCK_MODE" == startpaused ]]; then
    printf 'Execution paused: Start OAuth\nexecutionId: exec_start\n'
  else
    jq -nc --arg url "$url" '{ok:true,data:{status:"redirect",authorizationUrl:$url}}'
  fi
else
  printf 'Unexpected mock invocation\n' >&2; exit 1
fi
MOCK
cat > "$EXECUTOR_OPENER" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$MOCK_MODE" == openfailed ]]; then exit 1; fi
printf 'browser-open\n' >> "$TEST_TRACE"
MOCK
chmod +x "$EXECUTOR_CLI" "$EXECUTOR_OPENER"

run_case() {
  local mode="$1" expected="$2" actual=0
  shift 2
  : > "$TEST_TRACE"
  MOCK_MODE="$mode" bash "$repo/scripts/executor-cloudflare-auth.sh" "$@" > "$tmp/output" 2>&1 || actual=$?
  if [[ "$actual" != "$expected" ]]; then
    printf 'FAIL %s: expected exit %s, got %s\n' "$mode" "$expected" "$actual" >&2
    cat "$tmp/output" >&2
    exit 1
  fi
  if grep -q 'private-test-state' "$tmp/output"; then
    printf 'FAIL: OAuth state leaked to console\n' >&2; exit 1
  fi
  printf 'PASS %s\n' "$mode"
}
assert_no_open() { ! grep -q '^browser-open$' "$TEST_TRACE"; }
assert_no_resume() { ! grep -q '^resume ' "$TEST_TRACE"; }

run_case success 0
grep -q '^browser-open$' "$TEST_TRACE"
run_case success 0 cloudflare-bindings --prepare-only
assert_no_open
! grep -q 'oauth start' "$TEST_TRACE"
run_case success 0 --all --prepare-only
test "$(grep -c 'clients registerDynamic' "$TEST_TRACE")" -eq 6
assert_no_open
run_case success 1 --all
run_case success 1 cloudflare-docs
run_case success 1 cloudflare-missing
run_case success 1 cloudflare-api cloudflare-bindings
run_case success 1 --unknown
run_case success 0 --help
test ! -s "$TEST_TRACE"
run_case remote 1
assert_no_open
run_case mismatch 1
! grep -q 'oauth probe' "$TEST_TRACE"
run_case hostile 1
! grep -q 'clients registerDynamic' "$TEST_TRACE"
run_case failed 1
assert_no_open
run_case malformed 1
assert_no_open
run_case badurl 1
assert_no_open
run_case paused 2
assert_no_resume
assert_no_open
run_case paused 0 --approve-setup
grep -q '^resume --execution-id exec_register --action accept --content {} --base-url http://localhost:4955$' "$TEST_TRACE"
grep -q '^browser-open$' "$TEST_TRACE"
run_case nested 1 --approve-setup
test "$(grep -c '^resume ' "$TEST_TRACE")" -eq 1
assert_no_open
run_case startpaused 2
assert_no_resume
assert_no_open
run_case startpaused 0 --approve-setup
grep -q '^resume --execution-id exec_start --action accept --content {} --base-url http://localhost:4955$' "$TEST_TRACE"
grep -q '^browser-open$' "$TEST_TRACE"
run_case openfailed 1
printf 'All OAuth setup shell tests passed.\n'
