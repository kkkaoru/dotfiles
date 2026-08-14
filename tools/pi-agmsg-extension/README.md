# pi agmsg extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for
[agmsg](https://github.com/fujibee/agmsg). It provides:

- `/agmsg` for interactive setup, inbox, send, history, team, and identity operations.
- Setup lets you select an existing identity or create a new identity even when other agents are already registered. Team creation and agent editors are pre-filled with a collision-free project directory name and random `pi-<id>` name.
- `/agmsg leave [team]` removes the active identity from a team after confirmation.
- `/agmsg reconnect` refreshes the active identity, restarts background delivery, and checks the inbox after resuming a session.
- The status includes both identity and team, for example `agmsg: oversea-horse-race (horse-racing-data)`.
- An LLM-callable `agmsg` tool using only agmsg's supported scripts.
- Successful sends display an `[agmsg-sent]` message with sender, recipient, team, and message body.
- Invisible background polling plus end-of-turn safety checks, equivalent to agmsg `both` delivery for pi. The LLM is instructed not to run visible `sleep`/`inbox` heartbeat tools. Incoming messages use pi's `steer` queue and start a model turn only when a real unread message arrives; empty heartbeat polls remain invisible.
- A trusted external agmsg `types/pi` manifest, installed by the dotfiles symlink script.

## Install

Install agmsg first, then run from the dotfiles root:

```bash
./create-symlinks.sh
```

This links this directory to `~/.pi/agent/extensions/agmsg` and links/trusts the
`types/pi` plugin through agmsg's `plugin.sh`. Restart pi or run `/reload`.

## Quality checks

```bash
bun install
bun run check
```

Vitest enforces 95% minimum branch, function, line, and statement coverage.
Oxlint enables every rule category and limits nesting depth to three.
