# Pi compaction verification

Verified against installed Pi 0.85.1 with the local startup patches.

## Automated checks

```sh
node --test scripts/pi-compaction-singleflight.test.mjs scripts/pi-compaction-core.test.mjs scripts/pi-compaction-lifecycle.test.mjs scripts/pi-compaction-rpc.test.mjs
```

45 tests passed. Coverage includes manual/automatic success, cancellation during auth and generation, unresponsive providers, late replies/rejections, empty and partial summaries, extension notifications, idle release, split-turn inputs, fullscreen queued input, and disk-session restart/resume. Terminal and RPC tests run real Pi processes against a loopback mock API with isolated temporary sessions. The terminal test requires macOS `/usr/bin/expect`.

## Opt-in live-service verification

```sh
PI_LIVE_COMPACTION_TEST=1 node scripts/pi-compaction-live.mjs
```

Uses ordinary Pi authentication, openai-codex/gpt-5.6-sol, max effort, no tools, and an in-memory session. Makes real billable API requests. The synthetic verification preserved an exact marker through compaction and returned it on the following prompt: compaction 8.0 seconds, total 10.5 seconds.

To verify an existing session without modifying its file, also set `PI_LIVE_SOURCE_SESSION` to its JSONL path. This sends its compactable context to the configured live provider. The script copies parsed entries into memory and checks the source file's hash afterwards.

The reported problem session `01a04a5e-558d-70fa-a827-3ef5c4f83a42` was verified with 24,725 entries: compaction 139.7 seconds; compaction plus subsequent input/reply 144.7 seconds. Source-file contents remained unchanged. The generated summary was nonempty (67,435 characters). This checks successful generation and subsequent input, not exhaustive semantic equivalence of all historical content.

## Operational notes

- Startup patches apply to newly started `pi` processes; restart an existing process to load changes.
- User settings remain `compactionEffort=max` and agmsg polling every 60 seconds.
- Large-session max-effort summaries can take minutes. That latency alone is not a deadlock.
- Conciseness is a model instruction, not a guaranteed word limit. The original generation budget is preserved to avoid truncating reasoning and summaries.
- Validation establishes the tested paths and version; it does not guarantee that future provider or Pi releases cannot introduce other failures.
