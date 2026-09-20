# Pi effort manager

Repository-owned Pi package for dynamic reasoning-effort management. It extends Pi's standard
static effort controls and has no runtime dependency on the former third-party package.

## Bounded compaction guard

The package also loads `src/context-guard.ts` as an independent safety extension. Small normal
compactions use Pi's implementation, and so do threshold compactions whose single-call
summarization request fits the model's declared window (`tokensBefore - keepRecentTokens` plus the
compaction reserve and prompt, against the context window). Deferring there keeps a long history in
one Pi call instead of dropping its middle in bounded segments. Overflow recovery and the remaining
oversized serialized histories use sequential, bounded summary segments with the selected model and
its existing credentials.
The previous summary, split-turn prefix and user focus are included; the retained-message boundary
and file-operation metadata are preserved. Original JSONL history is never rewritten.

Each request is limited conservatively using UTF-8 bytes (at most half the model's advertised token
window, capped at 96,000 bytes), leaving room for framing and output. A short running summary
carries decisions between segments. Output is capped at 8,192 tokens, uses low reasoning,
and disables prompt caching with a fresh per-compaction session ID. Each segment also sends Pi's
OpenCode session headers (`x-opencode-session`, `x-opencode-client`) when the model belongs to
`opencode`/`opencode-go` or points at `opencode.ai`: Pi's provider runner adds those to its own
requests, extension-initiated model calls bypass that runner, and OpenCode rejects a request without
the session header (`MissingSessionID`) instead of falling back. Other providers' headers are left
alone. Successful calls' usage is accumulated.
Histories larger than the 8-segment full-summary allowance automatically enter **lossy emergency
recovery** rather than cancelling: select the beginning (one quarter) and the newest tail (three
quarters) of an eight-segment character budget. With the explicit gap marker, this uses at most nine
model calls. Sequential xAI/Grok summarization of a long session otherwise stays on Auto-compacting
for tens of provider calls with no progress. The UI warns before those calls, and the saved summary permanently records that the
historical middle was omitted. That warning is not a failure: compaction still saves the head/tail
handoff so auto-compact can finish instead of cancelling and retrying the same oversized request. It must not be treated as evidence that missing work was completed;
consult the preserved original session for missing requirements and decisions. Sampling happens
before allocating code-point arrays, and selection never cuts a UTF-16 surrogate pair.

Abort and non-transient provider errors still cancel compaction
rather than falling back to the same oversized request or saving partial history. A `length` stop
(max-output truncation) is kept and then bounded: reasoning models often spend the output budget on
thinking and still return usable text. A segment that returns thinking without text keeps that
thinking, and one content-free segment leaves the running handoff untouched; only an entirely empty
summary cancels. Connection drops (`terminated`, `Connection error`) retry the
same segment a few times before cancelling. An overlong summary is bounded instead of cancelling:
agent-style providers (for example Devin, which emits tool renderings as assistant text) can ignore
the requested summary length, so each segment summary is trimmed to its byte budget on a code-point
boundary and marked as truncated. OpenCode Go compaction requests also send `User-Agent:
pi-coding-agent`; Go drops generic SDK/fetch user-agents. This bounds work;
it cannot guarantee success for incorrect model metadata, provider failures or oversized retained
recent messages. OpenCode Go is one such case: its catalog declares 1M-token windows with up to
384,000 output tokens, and Pi's character-based fallback estimate (four characters per token)
undercounts Japanese-heavy history, so a request can pass Pi's clamp and still be rejected with
`maximum context length is 1048576 tokens`. `.pi/agent/models.json` therefore overrides every
`opencode-go` model to 60% of its catalog window and at most 65,536 output tokens, which restores the
margin Pi's threshold and request clamp assume. The loop extension then pauses for explicit recovery
rather than spinning.

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
