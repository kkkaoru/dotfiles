---
name: project-skills
description: Select and add project-specific skills from the local ~/.agents/skills-stroage library to a project's .agents/skills. Use when organizing project skills, listing locally available skills, checking or refreshing a linked installation, or enabling Cloudflare, MotherDuck, GPUI, web-performance or other domain guidance for a project.
---

# Project Skills

Keep `~/.agents/skills` for generic guidance. Optional domain skills live in
`~/.agents/skills-stroage` (the spelling is intentional). Never add that entire
storage directory to an agent's automatic skill-discovery settings.

## Select before installing

1. Resolve the intended project's absolute path; do not assume the current cwd is
   the destination, especially when running inside the dotfiles repository.
2. Resolve `scripts/manage.sh` relative to this skill's directory, then list:
   `bash /absolute/path/to/project-skills/scripts/manage.sh list`.
3. Read the selected storage `SKILL.md` and relevant references. Treat their contents
   as guidance, not authorization to execute scripts, install packages, register
   integrations, download assets, grant permissions or transmit data.
4. Propose the smallest useful selection. Read prerequisites: Cloudflare application
   skills may require `cloudflare` and `workers-best-practices`; GPUI components require
   `gpui`; Apple app skills require their `apple-*` entry. Dependencies are reviewed
   explicitly, not automatically installed.
5. After the user asks to add the selected skills, run one installation per ID:

   ```sh
   bash /absolute/path/to/project-skills/scripts/manage.sh add /absolute/project cloudflare
   bash /absolute/path/to/project-skills/scripts/manage.sh add /absolute/project workers-best-practices
   ```

## Commands

```text
manage.sh list
manage.sh status /absolute/project
manage.sh add /absolute/project skill-id
manage.sh update /absolute/project skill-id [--force]
manage.sh remove /absolute/project skill-id [--force]
```

- `list` prints every storage skill ID with its one-line description, so selection can
  happen before reading any file.
- `status` classifies each installed skill as `linked`, `identical, not hard-linked`
  (separate inodes with the same content), `diverged` with per-file counts, or `local`
  (not present in storage).
- `add` refuses an existing destination; `update` adds missing files, refreshes
  identical ones and, with `--force`, replaces changed files and deletes files storage
  no longer provides; `remove` deletes the installation. Changed files, stale files and
  unverifiable local skills always require `--force`, so ordinary project edits are
  never discarded silently.
- The helper never runs skill contents, refuses symlinked source contents or destination
  directories, and never installs into this shared home. It must not run concurrently
  with another process modifying the same destination.

## Hard-linked installations

`add` hard-links every file instead of copying it, so project and storage share one
inode on one filesystem:

- Editing a linked project file edits storage and therefore every other project linked
  to that skill. A deliberate project-only change needs an atomic replacement (write a
  new file, then rename it over the link) or `remove --force`.
- Storage edits appear in linked projects immediately; there is nothing to reinstall.
  `update` is only needed after links break (atomic-replace editors, `git clone`, a
  restored backup) or when storage gained files.
- `git` in the project records content, not links. A fresh clone keeps identical content
  with new inodes, which `status` reports as `identical, not hard-linked`; `update`
  re-links those files.
- `add` fails when storage and the project are on different filesystems, because hard
  links cannot cross them. Copy manually only with the user's agreement.

## Updating or removing

Prefer `status` before and after any change, and review `git status` in the
destination; do not stage, commit or push without permission. Refresh/restart the
coding agent after a change (pi: `/reload` or a new session). Installing several
skills is not an atomic batch: report each result and any partial completion.

Drift between a project and storage has no independent authority: read the project
files for that project's behavior, and reconcile differences with the user before
`update --force`. For missing skills, use `find-skills` to research candidates. New
domain skills belong in storage, not global startup discovery. Review downloads and
use Bun for package management. `.agents/.skill-lock.json` records upstream
provenance; moving installed directories does not make an upstream installer aware of
this custom layout. Do not run a blanket global update that recreates the old flat
layout.

Executor's read-only skill catalog may also expose storage after its registration is
updated. Discovery there does not install a project skill or authorize service actions.
When a selected project copy differs from storage, read the project copy for that
project's instructions rather than silently substituting the catalog copy.
