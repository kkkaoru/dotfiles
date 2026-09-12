#!/usr/bin/env bash
# Provision the shared local catalog, never change approval policies or credentials.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
executor="$repo/scripts/executor"
approve=false
case "${1:-}" in
  '') ;;
  --approve-registration) approve=true ;;
  *) printf 'Usage: %s [--approve-registration]\n' "$0" >&2; exit 1 ;;
esac
command -v jq >/dev/null
bun="$(command -v bun)"
bunx="$(command -v bunx)"
mkdir -p "$HOME/.local/bin" "$HOME/.pi/agent/packages"
if [[ ! -e "$HOME/.pi/agent/packages/executor-skills" && ! -L "$HOME/.pi/agent/packages/executor-skills" ]]; then
  ln -s "$repo/tools/executor-skills" "$HOME/.pi/agent/packages/executor-skills"
fi
if [[ ! -e "$HOME/.local/bin/executor" && ! -L "$HOME/.local/bin/executor" ]]; then
  ln -s "$repo/scripts/executor" "$HOME/.local/bin/executor"
fi
(cd "$repo/tools/executor-skills" && "$bun" install --frozen-lockfile)

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
  local payload="$1" auth="$2" slug
  slug="$(printf '%s' "$payload" | jq -r '.slug')"
  if ! "$executor" tools integrations | jq -e --arg slug "$slug" '.items[] | select(.id == $slug)' >/dev/null; then
    setup_call register "$payload"
  fi
  if [[ "$auth" == none ]]; then
    if ! "$executor" call executor coreTools connections list '{}' | jq -e --arg slug "$slug" '.data.connections[] | select(.integration == $slug and .owner == "user" and .name == "default")' >/dev/null; then
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

register "$(jq -nc --arg command "$bun" --arg entry "$repo/tools/executor-skills/cli.mjs" --arg global "$HOME/.agents/skills" --arg pi "$HOME/.pi/agent/skills" --arg project "$repo/.agents/skills" '{transport:"stdio",slug:"local-skills",name:"Local Agent Skills (read-only)",command:$command,args:[$entry,$global,$pi,$project]}')" none

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
