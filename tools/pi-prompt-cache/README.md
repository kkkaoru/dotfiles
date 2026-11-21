# pi-prompt-cache

Shows how much of every request the provider served from its prompt cache, and names what broke
the prefix when a large prompt was recomputed.

`/cache` prints the session's hit rate and the last few prompts the provider recomputed. The same
percentage sits in the status line as `cache 93%`.

A line appears in the chat when a prompt of 8000 tokens or more came back mostly uncached:

- `Cache break: 79403 tokens recomputed after the tools changed.` — the request itself changed the
  prefix: the model, a tool definition, the system prompt, or an earlier message. Informational: the
  cost was the change you asked for.
- `Cache miss: … although the prefix was unchanged; the provider dropped it.` — nothing in the
  request changed, so the entry was evicted or the request landed on another instance. This one is a
  warning, because it is the only case that costs tokens for no reason. Reported once per session,
  because repeating it would only repeat the same fact.

The prefix is fingerprinted as four parts — model, system prompt, tool definitions, and the messages
— so a normal turn (which only appends) never counts as a change, and switching model is named as
the cause instead of being blamed on the provider.

The ledger is a Pi Durable document in `<session file>.cache-durable/`, so `/resume` and
`/reload` keep the same numbers. It holds the newest 200 requests.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/prompt-cache`. Start a new
session after installing.

`tools/pi-cf-affinity-extension` is what makes the hits happen: it pins a session to one Workers AI
instance. This extension only reports.

## Check

```sh
bun install
bun run check
```
