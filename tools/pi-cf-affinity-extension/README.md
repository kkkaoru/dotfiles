# pi-cf-affinity-extension

Pins a Pi session to one Workers AI instance so its prompt prefix stays cached.

Workers AI enables prefix caching by default for select models: a request that shares the start of
an earlier prompt skips the prefill for that part, which cuts time to first token and bills those
tokens at a lower rate. The cached tensors live on the instance that computed them, so Cloudflare
documents the `x-session-affinity` header as the way to keep a session on that instance.

Measured through `gateway.ai.cloudflare.com` with `workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017`,
the same 3043-token prompt sent twice:

|                           | cached tokens | neurons |
| ------------------------- | ------------- | ------- |
| without the header        | 0             | 83.9    |
| with `x-session-affinity` | 2944          | 5.2     |

This extension sets the header, once per request, to `pi-<session id>` for
`cloudflare-ai-gateway` and `cloudflare-workers-ai`. Every other provider is untouched, and a
header an earlier handler already set is left as it is.

Nothing else changes: the model, the context window, the output limit and the reasoning effort stay
as configured. It only makes the same request cheaper and faster.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/cf-affinity`, and
`~/.pi/agent/settings.json` loads `./extensions/cf-affinity`. Start a new session after installing.

## Check

```sh
bun install
bun run check
```
