# pi-relations-extension

One graph of how a session's pieces relate: the session itself, its goal and loop, the tmux jobs it
ran, the timeline tree it planned, and the files or docs it produced. It lives in
[Pi Durable](https://www.npmjs.com/package/@earendil-works/pi-durable) storage beside the session
file (`.relations-durable`), like the loop, tmux and timeline sidecars, so a resumed session finds it.

A node id is `kind:ref`, where the kind is one of `session`, `goal`, `loop`, `tmux`, `timeline`,
`task`, `doc` or `file`. An edge is `from` + a lower-case relation + `to`, with an optional note.

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
