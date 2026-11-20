# pi-timeline-extension

A task tree for the session, kept in
[Pi Durable](https://www.npmjs.com/package/@earendil-works/pi-durable) storage, modelled on the
Harness timeline: tasks and subtasks, each with a status, a note and a weight, and one weighted
percentage that says how much of the work is finished.

The agent records the tree with the `timeline` tool, and you can read it at any time with
`/timeline`. The percentage also sits in the footer status area.

## The tool

| action   | fields                                           |
| -------- | ------------------------------------------------ |
| `add`    | `title`, optional `parent`, `weight` (default 1) |
| `update` | `id`, any of `status`, `note`, `title`, `weight` |
| `list`   | —                                                |

`status` is `pending`, `active`, `done` or `blocked`. Every call answers with the rendered tree:

```
63% · 3 tasks
· t1 Fix bug (100%)
  ✔ t2 Read code
▸ t3 Ship — waiting on review
```

A task that has subtasks takes their weighted score instead of its own status, so a parent is
finished only when its children are. A leaf counts `done` as 1, `active` as 0.5, `blocked` and
`pending` as 0.

## Storage

The tree is committed to Pi Durable before it is shown. With a session file it lives in
`<session file>.timeline-durable/` — a sibling directory, like the loop and tmux sidecars, so a
resumed session finds its own tree and nothing is shared between sessions. Without a session file
(`--no-session`) the tree lives in memory for that run.

Saves are serialized and fire-and-forget: the tool answers immediately, and a failed commit is
reported once in the chat and then dropped rather than retried. A session that cannot open its
sidecar still runs; the tree just stays empty.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/timeline`. Start a new pi
session afterwards, or run `/reload`.

## Check

```sh
cd tools/pi-timeline-extension
bun run check
```
