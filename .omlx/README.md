# oMLX (local Qwen 3.8 + DFlash2)

This directory is repository-managed the same way as `.pi`: `~/.omlx` is a
symlink into the checkout. Only the DFlash/server config is tracked.

Weights, caches, logs, and the oMLX binary wrappers stay on the machine.

## Tracked

- `model_settings.json` — DFlash2 draft, block size, verify mode, context
- `settings.json.example` — server template (no `secret_key`)

## Ignored

- `models/` — safetensors (about 19GB for 4-bit target + DFlash2)
- `cache/`, `logs/`, `bin/`, `stats.json`, live `settings.json`

## Setup

```sh
./create-symlinks.sh
./scripts/setup-omlx.sh
```

`setup-omlx.sh` creates `settings.json` from the example if missing, generates
a local `secret_key`, downloads:

- `mlx-community/Qwen3.8-27B-4bit`
- `incoai/Qwen3.8-27B-DFlash2`

and restarts oMLX.

Requires `/Applications/oMLX.app` (z-lab `0.6.2-dflash2` or newer with DFlash).

## Idle unload / process stop

The 27B weights stay loaded only while in use:

- `is_pinned` is false, so TTL applies
- global idle timeout is 15 minutes (`idle_timeout_seconds = 900`)
- `auto_start_on_launch` is false
- `~/.local/bin/pi` starts oMLX on demand, then runs the real `~/.bun/bin/pi`
- launch agent `com.kkkaoru.omlx-idle-stop` stops `omlx-server` once the model is unloaded and nothing is connected (skipped while DeepSWE/Pier is running)
