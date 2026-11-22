# pi-relations-extension

One graph of how a session's pieces relate: the session itself, its goal and loop, the tmux jobs it
ran, the timeline tree it planned, and the files or docs it produced. It lives in
[Pi Durable](https://www.npmjs.com/package/@earendil-works/pi-durable) storage beside the session
file (`.relations-durable`), like the loop, tmux and timeline sidecars, so a resumed session finds it.

## Automatic recording

Nobody links the goal, the loop, a job or a plan by hand: the extension subscribes to
`tool_result` and records what each of those tools did, so `relations list` shows one picture.

| Tool | Edge added | Note |
| --- | --- | --- |
| `start_goal` `get_goal` `goal_wait` | `session —pursued→ goal:<id>` | the objective, clipped to 160 characters |
| `update_goal` | the pursuit, plus `goal —completed\|blocked→ session` | the reason |
| `loop_wakeup` | `session —looped→ loop:<session id>` | the reason |
| `loop_complete` | the loop, plus `loop —completed→ session` | the reason |
| `tmux_exec` | `session —ran→ tmux:<tmux session>` | the command |
| `tmux_check_in` | `session —ran→ tmux:<tmux session>` | none |
| `timeline` | `session —planned→ timeline:<tree>`, plus `timeline —contains→ task:<tree>/<id>` for `add` | the title |

A loop is one per session, so its id is the session id; a timeline call that names no tree is
`tree-1`. Nothing is inferred beyond the call: a goal tool that found no goal, or a tmux launch
that never started, records nothing. Recording is idempotent and silent — a lock held elsewhere or
a full disk is dropped, because the tool call it observed has already answered. Only these eight
tools are recorded; a file or document edge is still a `relations link`.

The graph is one hop: recorded edges are added, never interpreted.

```
relations list                                     # the whole graph
relations show session:01a1fcbf                    # one node, both directions
relations link session:01a1fcbf ran tmux:pi-tmux-1 # creates either endpoint if missing
relations add doc:docs/report.md --label "the report"
```

The same graph is reachable from a shell, which is what a script uses:

```
bun tools/pi-relations-extension/cli.ts link session:$PI_SESSION_ID produced doc:docs/report.md
bun tools/pi-relations-extension/cli.ts list
```

`cli.ts` writes `$PI_SESSION_FILE` by default, so a shell started inside a pi session records into
that session's graph; `--session <file>` picks another.

Writing is idempotent: adding a known node changes nothing, and linking the same edge twice only
updates its note. `bun run check` runs format, lint, typecheck and the coverage gate.
