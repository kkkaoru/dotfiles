# pi-timeline-extension

A task tree for the session, kept in
[Pi Durable](https://www.npmjs.com/package/@earendil-works/pi-durable) storage, modelled on the
Harness timeline: tasks and subtasks, each with a status, a note, a weight, done units, a priority
and an estimate, and one weighted percentage that says how much of the work is finished. A session
can hold several plans, one per goal, and the widget draws the active one.

The agent records the tree with the `timeline` tool, and you can read it at any time with
`/timeline`. While the tree has unfinished tasks it is drawn below the editor, so it stays in sight
without competing for the footer row; the percentage also sits in the footer status area. Once every
task is done the widget goes away, since there is nothing left to track, and `/timeline` still prints
the finished tree.

## A tree that goes stale

The widget is only useful while it says something true, so it carries its own age. Five minutes
after the last change, the head adds `· updated 42m`, repainted once a minute (`1m`, `59m`, `1h`,
`1d`), so a frozen tree looks frozen instead of authoritative.

A turn can run for an hour, so the reminder arrives mid-turn: after 24 tool calls without a
`timeline` call, and while tasks are unfinished, the agent is steered once with a hidden
`timeline-nudge` message — a task progress checkpoint asking it to mark what finished as done, keep
the current task active with a note, to record done units or an estimate, and to add tasks if the
scope changed. Any `timeline` call restarts the count, and a plan with every task done or retired is
never reported.

The agent is also told, in the tool's own guidelines, how the reference package plans work: break
multi-step work into checkable tasks before starting, update at meaningful checkpoints, verify a
task before marking it done, recheck a blocked dependency once, and save progress before a final
answer. It is told what the fields are for as well: a weight when the work is uneven, done units for
partly finished work, an estimate in minutes when one is known, `P0`–`P3` when the order matters,
`paused`/`blocked` for work that waits on someone else, `retired` for work that leaves the goal, and
a new plan with `start` for a new goal.

## The tool

| action   | fields                                                                                            |
| -------- | ------------------------------------------------------------------------------------------------- |
| `start`  | `title` — a plan for a goal; the same goal reopens its own plan                                   |
| `select` | `tree` — show another plan                                                                        |
| `add`    | `title`, optional `parent`, `weight` (default 1), `tree`                                          |
| `update` | `id`, any of `status`, `note`, `title`, `weight`, `done`, `priority`, `remaining`, `mode`, `tree` |
| `list`   | —                                                                                                 |

`status` is `pending`, `active`, `paused`, `blocked`, `done` or `retired`: a paused task waits on
someone else and still counts as unfinished, while a retired one has left the goal and is dropped
with everything under it. `done` takes units finished out of units in total (`[2, 5]`), so a task
can be partly finished, and `remaining` is the minutes still expected. `priority` is `P0`–`P3` or
null, `mode` is `serial` (default) or `parallel` for a task's own subtasks, and `tree` names the
plan a call works on. Every call answers with the rendered tree:

```
██████░░░░ 62.50% · 3 tasks
██████ 100.00% · t1 Fix bug
██████ 100.00%   ✔ t2 Read code
███░░░  50.00% ▸ t3 Ship — waiting on review
```

Every line opens with its own bar and the percentage right after it, as the Harness timeline draws
them: the tree's bar is longer than a task's, the number keeps two decimals, and work that is not
finished never reads `100.00%`. The percentage is padded to the width of `100.00%`, so its digits
line up down the tree, and a subtask reads as the deeper indent after it. A row then carries a glyph
for its state (`·` pending, `▸` active, `⏸` paused, `✖` blocked, `✔` done, `–` retired), a `[P0]`-style
tag when it has a priority, and `· ~25m` when it has an estimate left. The head adds the plan's title
when it has one, `· ~30m left` when every task is estimated, and the age once the tree is stale.

Colours come from the active pi theme, so the Dracula palette is used here: green for finished work,
purple for what is running, red for a blocked task, the comment colour for the rest, and the dim
colour for the part of a bar that is still empty. Only the widget is coloured; the tool's answer and
`/timeline` stay plain text.

The tree's own bar is drawn in segments, as the reference package draws its dock bar: the finished
part fills it in green, and the cells of what remains take the colour of the state they are in —
purple while a task runs, red for blocked work, dim for work not started. Plain text keeps the
`█`/`░` split, so a tool answer still says how much is left without a colour to read.

A task that has subtasks takes their weighted score instead of its own status, so a parent is
finished only when its children are; a group's state is the loudest of its children's (all done is
done, otherwise active, then blocked, then paused, then pending). A leaf is as far along as the done
units it recorded, or all the way when its status is `done`, and a retired task is out of the sum
together with its subtree. Siblings are weighted by their share of the largest, as the reference
package weighs them, and work that is not finished is capped just below 1.

An estimate adds up across subtasks that run one after another and takes the longest when the task's
`mode` is `parallel`; one task without an estimate makes its group unknown rather than wrong.

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

Every plan of the session is committed to Pi Durable before it is shown; a sidecar written before
plans existed (a single `{ tasks }` tree) is read as one plan, with the new fields filled in. With a
session file it lives in
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
