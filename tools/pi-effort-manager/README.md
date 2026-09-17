# Pi effort manager

Repository-owned Pi package for dynamic reasoning-effort management. It extends Pi's standard
static effort controls and has no runtime dependency on the former third-party package.

## Bounded compaction guard

The package also loads `src/context-guard.ts` as an independent safety extension. Small normal
compactions use Pi's implementation. Overflow recovery and oversized serialized histories use
sequential, bounded summary segments with the selected model and its existing credentials.
The previous summary, split-turn prefix and user focus are included; the retained-message boundary
and file-operation metadata are preserved. Original JSONL history is never rewritten.

Each request is limited conservatively using UTF-8 bytes (at most half the model's advertised token
window, capped at 96,000 bytes), leaving room for framing and output. A short running summary
carries decisions between segments. Output is capped at 4,096 tokens, uses low OpenAI reasoning,
and disables prompt caching with a fresh request session ID. Successful calls' usage is accumulated.
Histories larger than the 128-segment full-summary allowance automatically enter **lossy emergency
recovery** rather than cancelling: select the beginning (one quarter) and the newest tail (three
quarters) of an eight-segment character budget. With the explicit gap marker, this uses at most nine
model calls. The UI warns before those calls, and the saved summary permanently records that the
historical middle was omitted. It must not be treated as evidence that missing work was completed;
consult the preserved original session for missing requirements and decisions. Sampling happens
before allocating code-point arrays, and selection never cuts a UTF-16 surrogate pair.

Abort, provider errors, tool calls and empty/truncated/oversized summaries still cancel compaction
rather than falling back to the same oversized request or saving partial history. This bounds work;
it cannot guarantee success for incorrect model metadata, provider failures or oversized retained
recent messages. The loop extension then pauses for explicit recovery rather than spinning.

The repository global settings now reserve 65,536 tokens for proactive compaction and retain
12,000 recent tokens. Project overrides still take precedence. This global reserve is intended
for the configured 272k model; use a smaller project reserve for models with small windows.
Restart Pi to load settings and the new package entry point. For a failed existing session,
first `/loop pause` and `/goal pause`, then `/compact`; only resume scheduling after successful
compaction. If bounded recovery fails, use `/tree` to an earlier safe point or `/new` with a concise
handoff, retaining the original session for reference. Do not repeatedly send `continue`.

## Controls

Pi's standard controls remain authoritative for static effort selection:

- `Shift+Tab` cycles effort levels.
- `--thinking <level>` selects the initial level.
- `defaultThinkingLevel` configures Pi's default level.

This package does not redefine those controls. Its additional controls are:

- `/fast [on|off]` controls OpenAI/Codex/Azure GPT-5 priority service tier.
- `/dynamic-effort on|off|status`
- `/dynamic-effort start <level|default>`
- `/dynamic-effort end <level|default>`
- `/dynamic-effort compact <level|default>`
- `/dynamic-effort reset-effort <level|default>`
- `/dynamic-effort reset <positive-integer|default>`
- `Ctrl+Shift+E` is the sole package-defined shortcut and toggles dynamic mode for the session.
  `Ctrl+Shift+D` is reserved by Pi's TUI for writing `pi-debug.log`.
- `--dynamic-effort on|off` overrides the restored/default mode.

Dynamic mode discovers supported levels from the active Pi model. Provider-equivalent mapped levels
are deduplicated. Normal work starts at the configured `startEffort` (`medium` by default), ramps from
60% of Pi's effective compaction limit through the configured `endEffort` (the penultimate supported
effort by default), and uses `compactionEffort` (the deepest supported effort by default) for Pi
compaction. Unsupported configured boundaries resolve to the nearest usable model capability. After
compaction it recalculates from current context usage. A compaction counts toward the reset interval
only when its pre-compaction effort is equal to or deeper than `compactionResetEffort` (`xhigh` by
default). The configurable reset interval defaults to one qualifying compaction and forces one
start-effort turn before normal ramping resumes.

Models without reasoning stay unchanged. Models with fewer than three distinct levels degrade
safely: the deepest level remains reserved when possible, and a single-level model uses that level for
both work and compaction.

`/dynamic-effort status` reports dynamic state, resolved start/end/compaction efforts, reset interval, context
usage, supported levels, successful compaction count, and observed reasoning-token average/maximum
per effort when the provider supplies `usage.reasoning`. The status and working-message effort labels
include `dynamic` while automatic control is active.

Natural progress text is model-authored rather than emitted as fixed extension notifications. When
enabled, an effort change or successful compaction gives the next model call one private, one-shot
opportunity to write a brief update. The system prompt tells the model to remain silent when there is
no meaningful development, avoid narrating routine tools, and never expose internal effort or
compaction mechanics. The two triggers are independently configurable.
Both are opt-in: only an explicit `true` enables a trigger; an omitted, malformed, or `false` value
keeps it disabled.

Optional defaults live under `pi-effort-manager` in Pi's global settings:

```json
{
  "pi-effort-manager": {
    "dynamicDefault": true,
    "startEffort": "medium",
    "endEffort": "xhigh",
    "compactionEffort": "max",
    "compactionResetEffort": "xhigh",
    "compactionResetInterval": 1,
    "fastMode": false,
    "progressTextOnCompaction": true,
    "progressTextOnEffortChange": true,
    "rampStartRatio": 0.6
  }
}
```

The manager settings may be placed in global `~/.pi/agent/settings.json` or trusted project
`.pi/settings.json`; project values override global values. The extension also uses global/project
`compaction.reserveTokens` when calculating the limit.
`/dynamic-effort start|end|compact|reset-effort|reset` creates session-local overrides; pass
`default` to clear one. State changes are recorded as session custom
entries, so resume and branching do not share a mutable process-global controller.
