#!/bin/bash

# Turn the executor MCP server in ~/.pi/agent/mcp.json on or off. It is on unless the
# entry carries "enabled": false, so `off` is for machines without Executor Desktop.
#   ./scripts/pi-executor.sh                     # print the state of this machine
#   ./scripts/pi-executor.sh off --host my-win   # disable it on another machine

set -euo pipefail

usage="usage: $0 [status|on|off] [--host <ssh-alias>]"

action=status
if [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; then
  action=$1
  shift
fi

host=""
while [ $# -gt 0 ]; do
  case "$1" in
    --host)
      host=${2:-}
      if [ -z "$host" ]; then
        echo "$usage" >&2
        exit 2
      fi
      shift 2
      ;;
    *)
      echo "$usage" >&2
      exit 2
      ;;
  esac
done

case "$action" in
  status | on | off) ;;
  *)
    echo "unknown action: ${action}" >&2
    exit 2
    ;;
esac

# The edit runs under bun on the target machine: bun is present on every machine this
# repository manages and reads the script from stdin, so one copy serves all of them.
if [ -n "$host" ]; then
  # `action` is one of the literals the case above accepted, and it expands here on purpose.
  # shellcheck disable=SC2029
  ssh "$host" "bun run - ${action}"
else
  bun run - "$action"
fi <<'JS'
import { readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const action = process.argv[2] ?? "status";
const file = join(homedir(), ".pi", "agent", "mcp.json");
const config = JSON.parse(readFileSync(file, "utf8"));
const server = config.mcpServers?.executor;
if (server === undefined) {
  console.error(`no executor server in ${file}`);
  process.exit(1);
}

const on = server.enabled !== false;
if (action === "status") {
  console.log(`executor: ${on ? "on" : "off"} (${file})`);
} else if (action === "off" ? on : !on) {
  if (action === "off") {
    server.enabled = false;
  } else {
    delete server.enabled;
  }
  writeFileSync(file, `${JSON.stringify(config, null, 2)}\n`);
  const written = JSON.parse(readFileSync(file, "utf8")).mcpServers?.executor;
  if ((written?.enabled !== false) !== (action === "on")) {
    console.error(`failed to turn the executor ${action} in ${file}`);
    process.exit(1);
  }
  console.log(`executor: ${action} (${file})`);
} else {
  console.log(`executor: already ${action} (${file})`);
}
JS
