#!/usr/bin/env bash
# Use Executor's OAuth storage and callback; never handle Cloudflare tokens here.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
executor="${EXECUTOR_CLI:-$repo/scripts/executor}"
opener="${EXECUTOR_OPENER:-open}"
integration=cloudflare-api
prepare=false
approve=false
all=false
chosen=false
usage() {
  printf 'Usage: %s [cloudflare-integration] [--prepare-only] [--all] [--approve-setup]\n' "$0"
}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prepare-only) prepare=true ;;
    --approve-setup) approve=true ;;
    --all) all=true ;;
    --help|-h) usage; exit 0 ;;
    cloudflare-*)
      if [[ "$chosen" == true ]]; then usage >&2; exit 1; fi
      integration="$1"; chosen=true ;;
    *) usage >&2; exit 1 ;;
  esac
  shift
done
if [[ "$all" == true && ( "$prepare" != true || "$chosen" == true ) ]]; then
  printf '%s\n' '--all requires --prepare-only and no individual integration' >&2
  exit 1
fi
command -v jq >/dev/null
catalog="$repo/tools/executor-skills/integrations.json"
if [[ "$all" == true ]]; then
  integrations="$(jq -er '.remote[] | select(.auth == "oauth2" and (.slug | startswith("cloudflare-"))) | .slug' "$catalog")"
else
  integrations="$(jq -er --arg slug "$integration" '.remote[] | select(.slug == $slug and .auth == "oauth2") | .slug' "$catalog")" || {
    printf 'Unknown OAuth integration or public server: %s\n' "$integration" >&2
    exit 1
  }
fi

# Validate every tool-level response: the CLI can exit zero for errors or pauses.
call_checked() {
  local operation="$1" payload="$2" output id
  case "$operation" in
    server) output="$("$executor" call executor mcp getServer "$payload" --base-url "$base")" ;;
    probe) output="$("$executor" call executor coreTools oauth probe "$payload" --base-url "$base")" ;;
    register) output="$("$executor" call executor coreTools oauth clients registerDynamic "$payload" --base-url "$base")" ;;
    start) output="$("$executor" call executor coreTools oauth start "$payload" --base-url "$base")" ;;
    *) printf 'Unsupported OAuth operation\n' >&2; return 1 ;;
  esac
  id="$(printf '%s\n' "$output" | awk '/^executionId: / { print $2; exit }')"
  if [[ -n "$id" ]]; then
    if [[ "$approve" != true || ( "$operation" != register && "$operation" != start ) ]]; then
      printf '%s\n' "$output" >&2
      printf 'Approve the displayed action or rerun with explicit --approve-setup consent.\n' >&2
      return 2
    fi
    output="$("$executor" resume --execution-id "$id" --action accept --content '{}' --base-url "$base")"
  fi
  # Never auto-accept a second prompt or print OAuth URLs/credentials on failure.
  if ! printf '%s\n' "$output" | jq -e '.ok == true' >/dev/null 2>&1; then
    printf 'Executor OAuth %s did not complete. Inspect the pending action or error in Executor UI.\n' "$operation" >&2
    printf '%s\n' "$output" | jq -r '.error.message // empty' >&2 2>/dev/null || true
    return 1
  fi
  printf '%s\n' "$output"
}

# Stay local even when the CLI's default server profile points at a hosted server.
# Discover the actual daemon port rather than baking 4788 into client registration.
"$executor" daemon run >/dev/null
status="$("$executor" daemon status)"
base="$(printf '%s\n' "$status" | awk '/^Daemon running at / { print $4; exit }')"
if ! printf '%s\n' "$base" | grep -Eq '^http://(localhost|127\.0\.0\.1):[0-9]+$'; then
  printf 'Cannot determine a loopback Executor daemon URL; refusing remote callback.\n' >&2
  exit 1
fi
callback="$base/api/oauth/callback"

prepare_integration() {
  local slug="$1" endpoint server template probe payload client result url
  endpoint="$(jq -er --arg slug "$slug" '.remote[] | select(.slug == $slug) | .endpoint' "$catalog")"
  server="$(call_checked server "$(jq -nc --arg slug "$slug" '{slug:$slug}')")"
  if ! printf '%s\n' "$server" | jq -e --arg endpoint "$endpoint" '.data.integration.config.endpoint == $endpoint' >/dev/null; then
    printf 'Registered endpoint differs from the official catalog for %s; refusing to overwrite it.\n' "$slug" >&2
    return 1
  fi
  template="$(printf '%s\n' "$server" | jq -er '[.data.integration.authMethods[] | select(.kind == "oauth") | .template][0] | select(type == "string" and length > 0)')"
  probe="$(call_checked probe "$(jq -nc --arg url "$endpoint" '{url:$url}')")"
  # Only use HTTPS Cloudflare metadata, never a caller-supplied issuer or scope list.
  if ! printf '%s\n' "$probe" | jq -e '[.data.registrationEndpoint, .data.authorizationUrl, .data.tokenUrl] | all(.[]; type == "string" and test("^https://([a-z0-9-]+\\.)*cloudflare\\.com/"))' >/dev/null; then
    printf 'Missing or untrusted Cloudflare OAuth discovery metadata for %s\n' "$slug" >&2
    return 1
  fi
  payload="$(printf '%s\n' "$probe" | jq -c --arg slug "$slug" --arg resource "$endpoint" --arg callback "$callback" '{owner:"user",slug:($slug + "-pi"),issuer:.data.issuer,registrationEndpoint:.data.registrationEndpoint,authorizationUrl:.data.authorizationUrl,tokenUrl:.data.tokenUrl,resource:$resource,scopes:[],tokenEndpointAuthMethodsSupported:.data.tokenEndpointAuthMethodsSupported,clientName:"Executor for pi",redirectUri:$callback,originIntegration:$slug}')"
  result="$(call_checked register "$payload")"
  # Executor chooses/reuses its own DCR slug; it need not match the requested slug.
  client="$(printf '%s\n' "$result" | jq -er '.data.client | select(type == "string" and length > 0)')"
  printf 'OAuth client ready: %s (%s)\n' "$slug" "$client"
  if [[ "$prepare" == true ]]; then return 0; fi
  payload="$(jq -nc --arg client "$client" --arg slug "$slug" --arg template "$template" --arg callback "$callback" '{client:$client,clientOwner:"user",owner:"user",name:"default",integration:$slug,template:$template,redirectUri:$callback}')"
  result="$(call_checked start "$payload")"
  if ! url="$(printf '%s\n' "$result" | jq -er 'select(.data.status == "redirect") | .data.authorizationUrl | select(type == "string" and test("^https://([a-z0-9-]+\\.)*cloudflare\\.com/"))')"; then
    printf 'Executor did not return a trusted Cloudflare authorization URL.\n' >&2
    return 1
  fi
  "$opener" "$url"
  printf 'Opened Cloudflare authorization for %s. Complete login and choose permissions in the browser.\n' "$slug"
  printf 'Opening the page is not authentication success; verify the connection in Executor afterwards.\n'
}

while IFS= read -r slug; do
  prepare_integration "$slug"
done <<< "$integrations"
