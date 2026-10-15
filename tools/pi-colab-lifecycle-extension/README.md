# pi colab lifecycle extension

Starts and stops the Colab-hosted Qwen3.8-27B (`google-colab` provider, see `tools/colab-qwen`)
in step with pi.

- Before every agent run on `google-colab` (and on switching into it) the extension checks that the
  Colab session `qwen` is alive. If not, it runs `scripts/colab-qwen-up.sh` (A100, vLLM, tunnel;
  a cold start takes ~10 min) and points the provider at the new tunnel URL with
  `pi.registerProvider("colab-qwen", { baseUrl })`.
- Requests, tool executions and agent ends on `google-colab` refresh `lastRequestAt` in
  `~/.cache/colab-qwen/state.json`.
- A detached watcher (`scripts/colab-qwen-idle-stop.sh`, single instance via a lock) stops the
  Colab session after 10 minutes without a request, so it also works after pi has exited.
- While a `google-colab` model is active the footer shows the remaining Colab compute units
  (`Colab 194.1 CU · 5.38/hr · ~36.1h left`, from `colab usage`, refreshed after each agent run and
  cached for 60 s); it is cleared when you switch to another provider.
- Failures degrade to a warning toast; they never break a prompt or a model switch.

The API key is the `google-colab` entry of `~/.pi/agent/auth.json` (never in Git).

```bash
bun install && bun run check
```

`create-symlinks.sh` links the extension to `~/.pi/agent/extensions/colab-lifecycle`.
Restart pi or run `/reload`.
