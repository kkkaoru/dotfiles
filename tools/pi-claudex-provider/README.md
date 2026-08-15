# pi-claudex-provider

Bidirectional integration package for Pi and Claudex.

Direction A exposes Pi providers as a raw model gateway over an authenticated Unix socket. It calls provider `streamSimple` directly, so Pi's agent loop does not run. Direction B registers configured Claudex models as the Pi provider `claudex` and uses Pi's Anthropic Messages streaming implementation to call the adapter.

The Direction A gateway is disabled unless both environment variables are set:

- `CLAUDEX_PI_GATEWAY_SOCKET`: absolute Unix socket path inside a private runtime directory
- `CLAUDEX_PI_GATEWAY_TOKEN`: per-process authentication token

Direction B configuration:

- `CLAUDEX_ADAPTER_BASE_URL`: adapter URL; default `http://127.0.0.1:8318`
- `CLAUDEX_PROVIDER_CONFIG`: model catalog source; default `~/.config/claudex/providers.json`
- `ANTHROPIC_AUTH_TOKEN`: required when the adapter is not on loopback

Direction B sends `x-claudex-origin: pi-provider`. The adapter must reject this origin if the selected route would enter the Pi gateway again. Direction A also excludes the `claudex` provider from model listing and rejects it explicitly.

## Gateway protocol

The socket uses strict LF-delimited JSON, version `1`. Every client message includes the per-process token.

1. Client sends `hello`; server sends `ready`.
2. `list_models` returns available Pi models except provider `claudex`.
3. `request` carries `provider`, `modelId`, raw Anthropic `system` / `messages` / `tools`, safe sampling options, and `origin: "claudex"`.
4. Server emits compact `text_*`, `thinking_*`, and `toolcall_*` events. `done` or `error` includes the full authoritative Pi assistant message and is terminal.
5. `cancel` aborts the matching request. Multiple authenticated connections and multiplexed request IDs are supported.

Install after verification:

```bash
bun install --cwd tools/pi-claudex-provider
bun run --cwd tools/pi-claudex-provider check
pi install "$PWD/tools/pi-claudex-provider"
```
