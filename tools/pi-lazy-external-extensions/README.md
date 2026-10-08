# Session-deferred external providers

CommandCode and Antigravity register eagerly when explicitly selected, otherwise on Pi's awaited
`session_start` boundary. Resource discovery alone must not start a timer or fetch their catalogs.
Initialization is cached once per extension runtime, including across session switches. Loading a
provider at session start can delay that boundary until its own initialization finishes; this is
intentional so a model request cannot race registration. CommandCode currently fetches its catalog
before falling back to cache, so it remains deferred during discovery-only invocations.

Web Access is **not** deferred: `.pi/agent/settings.json` loads the installed version through
`npm:pi-web-access@0.37.0`. Its `session_start`/`session_shutdown` hooks must be registered by the
native loader before session dispatch. The old 250ms factory timer missed the initial event and
could outlive reload; `PI_LAZY_EXTENSION_DELAY_MS` is no longer used.

No external packages are upgraded by this change. The two providers retain their existing host
compatibility aliases. Full removal of the custom importer is deferred until their catalog startup
can be made cache-only without changing the upstream providers. Reload or restart Pi to apply.

## Offline verification

Run `bun run smoke`. It creates a disposable provider fixture, checks discovery remains inert,
checks awaited/idempotent session loading, and loads the installed Web Access through Pi's native
resource loader with networking disabled. It starts no agent, OAuth flow or model call.
