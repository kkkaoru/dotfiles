# pi-timeline-extension

A task tree for the session, kept in
[Pi Durable](https://www.npmjs.com/package/@earendil-works/pi-durable) storage, modelled on the
Harness timeline: tasks and subtasks, each with a status, a note and a weight, and one weighted
percentage that says how much of the work is finished.

The agent records the tree with the `timeline` tool, and you can read it at any time with
`/timeline`. While the tree has tasks it is drawn below the editor, so it stays in sight without
competing for the footer row; the percentage also sits in the footer status area.

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

## Translation

The tree is drawn in the language `tools/pi-plamo-translate` gives you (`replyLanguage`, default
`ja`), so a task reads the same way as the reply that created it. Only titles and notes are sent;
the rest of the line stays as it is.

- The English tree is painted first and the Japanese replaces it when the translation lands, so a
  server that is still starting never leaves the tree blank.
- Each text is translated once and then kept, so a repaint of an unchanged task costs nothing.
- A text the server could not translate stays English and is asked again on the next paint — a
  restart that left the model stopped therefore heals itself.
- A translation that lands after a newer tree was drawn is dropped, so the widget never goes back to
  an older tree.
- The local server is started on demand through `pi-plamo-translate`'s own code. With
  `replyLanguage: "off"` nothing is translated and no request is made.

## Storage

The tree is committed to Pi Durable before it is shown. With a session file it lives in
`<session file>.timeline-durable/` — a sibling directory, like the loop and tmux sidecars, so a
resumed session finds its own tree and nothing is shared between sessions. Without a session file
(`--no-session`) the tree lives in memory for that run.

Saves are serialized and fire-and-forget: the tool answers immediately, and a failed commit is
reported once in the chat and then dropped rather than retried. A sidecar another pi holds, or one
that cannot be opened, leaves the tree in memory with one warning instead of taking the tree away.
The open sidecar is shared with a copy that `/reload` leaves behind, so a reload keeps both the tree
and its lock. A session that cannot open its
sidecar still runs; the tree just stays empty.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/timeline`. Start a new pi
session afterwards, or run `/reload`.

## Check

```sh
cd tools/pi-timeline-extension
bun run check
```
