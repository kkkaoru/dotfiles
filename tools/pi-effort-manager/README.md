# Pi effort manager

Repository-owned Pi package for dynamic reasoning-effort management. It extends Pi's standard
static effort controls and has no runtime dependency on the former third-party package.

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
