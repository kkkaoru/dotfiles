---
name: project-skills
description: Select and add project-specific skills from the local ~/.agents/skills-stroage library to a project's .agents/skills. Use when organizing project skills, listing locally available skills, or enabling Apple, Cloudflare, MotherDuck, GPUI or other domain guidance for a project.
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
4. Propose the smallest useful selection. Read prerequisites: Apple app skills
   also require `apple-pro-apps`; Cloudflare application skills may require
   `cloudflare` and `workers-best-practices`; GPUI components require `gpui`.
   Dependencies are reviewed explicitly, not automatically installed.
5. After the user asks to add the selected skills, run one installation per ID:

   ```sh
   bash /absolute/path/to/project-skills/scripts/manage.sh add /absolute/project cloudflare
   bash /absolute/path/to/project-skills/scripts/manage.sh add /absolute/project workers-best-practices
   ```

The helper copies the complete skill (including scripts/assets/references), never
runs its contents, refuses an existing destination and rejects symlinked source
contents or destination directories. It must not be used concurrently with another
process modifying the same destination. Copies are portable project files, not
absolute links to one user's home. Review `git status` in the destination; do not
stage, commit or push without permission. Refresh/restart the coding agent after
installation (pi: `/reload` or a new session).

## Updating or removing

Project copies do not auto-update. Compare the project copy with storage, preserve
local modifications, and obtain permission before replacing or removing it. The
helper intentionally has no force/update/remove operation. Installing several
skills is not an atomic batch: report each result and any partial completion.

For missing skills, use `find-skills` to research candidates. New domain skills
belong in storage, not global startup discovery. Review downloads and use Bun for
package management. `.agents/.skill-lock.json` records upstream provenance; moving
installed directories does not make an upstream installer aware of this custom
layout. Do not run a blanket global update that recreates the old flat layout.

Executor's read-only skill catalog may also expose storage after its registration
is updated. Discovery there does not install a project skill or authorize service
actions. When a selected project copy differs from storage, read the project copy
for that project's instructions rather than silently substituting the catalog copy.
