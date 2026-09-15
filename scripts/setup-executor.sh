#!/usr/bin/env bash
# Provision the shared local catalog, never change approval policies or credentials.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
executor="$repo/scripts/executor"
approve=false
install_service=false
port=4789
port_set=false
usage() {
  printf 'Usage: %s [--install-service] [--port 1-65535] [--approve-registration]\n' "$0"
}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --approve-registration) approve=true; shift ;;
    --install-service) install_service=true; shift ;;
    --port)
      [[ $# -ge 2 ]] || { usage >&2; exit 1; }
      port="$2"; port_set=true; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
  esac
done
if [[ ! "$port" =~ ^[0-9]{1,5}$ ]] || (( 10#$port < 1 || 10#$port > 65535 )); then
  printf 'Invalid service port: %s\n' "$port" >&2; exit 1
fi
if [[ "$install_service" != true && "$port_set" == true ]]; then
  printf '%s\n' '--port requires --install-service' >&2; exit 1
fi
# Rebuild this checkout's catalog on each Mac; never transplant a database tenant.
export EXECUTOR_SCOPE_DIR="$repo"
if [[ "$install_service" == true ]]; then
  [[ "$(uname -s)" == Darwin ]] || { printf 'Service setup requires macOS\n' >&2; exit 1; }
  if [[ ! -L "$HOME/.executor" ]] || [[ "$(cd "$HOME/.executor" && pwd -P)" != "$repo/.executor" ]]; then
    printf 'First link ~/.executor to %s/.executor; do not overwrite an existing home. See tools/executor-skills/SETUP.ja.md\n' "$repo" >&2
    exit 1
  fi
fi
command -v jq >/dev/null
bun="$(command -v bun)"
bunx="$(command -v bunx)"
ctx="$(command -v ctx)"
"$executor" --version
mkdir -p "$HOME/.local/bin" "$HOME/.pi/agent/packages"
if [[ ! -e "$HOME/.pi/agent/packages/executor-skills" && ! -L "$HOME/.pi/agent/packages/executor-skills" ]]; then
  ln -s "$repo/tools/executor-skills" "$HOME/.pi/agent/packages/executor-skills"
fi
if [[ ! -e "$HOME/.local/bin/executor" && ! -L "$HOME/.local/bin/executor" ]]; then
  ln -s "$repo/scripts/executor" "$HOME/.local/bin/executor"
fi
(cd "$repo/tools/executor-skills" && "$bun" install --frozen-lockfile)

if [[ "$install_service" == true ]]; then
  # Executor persists EXECUTOR_SCOPE_DIR into the macOS LaunchAgent environment.
  # Explicit opt-in: installing the service can restart an existing daemon.
  "$executor" service install --port "$port"
fi

# Only the two setup operations below may be auto-approved, and only by opt-in.
# A paused execution is NOT success, even when the CLI exits with status zero.
setup_call() {
  local operation="$1" payload="$2" output id
  case "$operation" in
    register) output="$("$executor" call executor mcp addServer "$payload")" ;;
    connect) output="$("$executor" call executor coreTools connections create "$payload")" ;;
    *) printf 'Unsupported setup operation\n' >&2; return 1 ;;
  esac
  printf '%s\n' "$output"
  id="$(printf '%s\n' "$output" | awk '/^executionId: / {print $2; exit}')"
  if [[ -n "$id" ]]; then
    if [[ "$approve" != true ]]; then
      printf 'Registration approval required. Approve the action above and rerun, or explicitly use --approve-registration.\n' >&2
      return 2
    fi
    output="$("$executor" resume --execution-id "$id" --action accept --content '{}')"
    printf '%s\n' "$output"
  fi
  # Do not accept nested prompts blindly or mistake a tool-level error for success.
  printf '%s\n' "$output" | jq -e '.ok == true' >/dev/null
}

register() {
  local payload="$1" auth="$2" slug catalog connections
  slug="$(printf '%s' "$payload" | jq -r '.slug')"
  catalog="$("$executor" tools integrations --limit 1000)"
  # Transport/tool errors and truncated catalogs are not evidence of absence.
  printf '%s' "$catalog" | jq -e '(.items | type == "array") and (.hasMore == false)' >/dev/null
  if ! printf '%s' "$catalog" | jq -e --arg slug "$slug" '.items[] | select(.id == $slug)' >/dev/null; then
    setup_call register "$payload"
  fi
  if [[ "$auth" == none ]]; then
    connections="$("$executor" call executor coreTools connections list '{}')"
    printf '%s' "$connections" | jq -e '.ok == true and (.data.connections | type == "array")' >/dev/null
    if ! printf '%s' "$connections" | jq -e --arg slug "$slug" '.data.connections[] | select(.integration == $slug and .owner == "user" and .name == "default")' >/dev/null; then
      setup_call connect "$(jq -nc --arg slug "$slug" '{owner:"user",name:"default",integration:$slug,template:"none"}')"
    fi
  else
    if [[ "$slug" == cloudflare-* ]]; then
      printf 'Start OAuth: ./scripts/executor-cloudflare-auth.sh %s (registration alone is not login)\n' "$slug"
    else
      printf 'OAuth required in Executor UI: %s (no credentials stored in dotfiles)\n' "$slug"
    fi
  fi
}

register "$(jq -nc --arg command "$bun" --arg entry "$repo/tools/executor-skills/cli.mjs" --arg global "$HOME/.agents/skills" --arg pi "$HOME/.pi/agent/skills" --arg project "$repo/.agents/skills" --arg storage "$HOME/.agents/skills-stroage" '{transport:"stdio",slug:"local-skills",name:"Local Agent Skills (read-only)",command:$command,args:[$entry,$global,$pi,$project,$storage]}')" none

# These are executable integrations, separate from the read-only skill catalog.
register "$(jq -nc --arg command "$ctx" '{transport:"stdio",slug:"ctx",name:"ctx Local Agent History",command:$command,args:["mcp","serve"]}')" none
register "$(jq -nc --arg command "$bun" --arg entry "$repo/tools/executor-skills/agmsg-cli.mjs" --arg scripts "$HOME/.agents/skills/agmsg/scripts" '{transport:"stdio",slug:"agmsg",name:"agmsg Pi Messaging",command:$command,args:[$entry,$scripts]}')" none

while IFS= read -r payload; do
  register "$payload" none
done < <(jq -c --arg command "$bunx" '.mcpServers | to_entries[] | select(.value.command == "bunx") | {transport:"stdio",slug:.key,name:.key,command:$command,args:.value.args}' "$repo/.mcp.json")

while IFS= read -r item; do
  auth="$(printf '%s' "$item" | jq -r '.auth')"
  payload="$(printf '%s' "$item" | jq -c '. + {transport:"remote",remoteTransport:"streamable-http",auth:{kind:.auth}}')"
  register "$payload" "$auth"
done < <(jq -c '.remote[]' "$repo/tools/executor-skills/integrations.json")

"$executor" tools integrations
printf '\nCloudflare login: ./scripts/executor-cloudflare-auth.sh cloudflare-api\n'
printf 'Use `executor web` for connection status, policies, and other integrations.\n'
