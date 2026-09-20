# Shared Agent Skills

`~/.agents` is a symlink to this entire directory. Installed skill definitions,
references, scripts, and `.skill-lock.json` are managed here.

## Global versus project-selected skills

- `skills/`: generic Python/Rust/Swift/TypeScript coding rules, Git commits, skill
  discovery, history guidance, and `project-skills` for local selection.
- `skills-stroage/`: 40 optional Cloudflare, MotherDuck, GPUI, Apple and
  web-performance skills. The spelling `stroage` is intentional. This is not an
  automatic global discovery root. Do not symlink all its entries back into `skills/`.
- A target project's `.agents/skills/`: hard links to only its selected skills. The
  project and this storage tree share one inode per file, so a storage edit reaches
  every linked project immediately and nothing needs reinstalling. `manage.sh status`
  reports `linked`, `identical, not hard-linked`, `diverged` or `local` per skill.

```sh
bash ~/.agents/skills/project-skills/scripts/manage.sh list
bash ~/.agents/skills/project-skills/scripts/manage.sh add /absolute/project cloudflare
bash ~/.agents/skills/project-skills/scripts/manage.sh status /absolute/project
bash ~/.agents/skills/project-skills/scripts/manage.sh update /absolute/project cloudflare
bash ~/.agents/skills/project-skills/scripts/manage.sh remove /absolute/project cloudflare
```

Read the `project-skills` skill before selecting dependencies; it documents the
hard-link consequences and the `--force` cases. The helper installs one skill per
call and never executes skill contents. An agent reload/new session is needed after
changing discovery. Pi's old domain-wide exclusions were removed so project-selected
copies are discoverable; history remains routed on demand. The Executor setup recipe
also includes storage as a read-only catalog root; an existing registration is not
silently changed by moving directories.

`.skill-lock.json` retains upstream provenance at the shared-home level. Upstream
installer defaults do not understand this split: review updates and route domain
skills to storage rather than recreating globally active copies.

`create-symlinks.sh` installs the top-level link. It refuses an existing real
`~/.agents` directory: back it up and merge its contents first, preserving managed
skill directories rather than replacing them with self-referential symlinks.
Resolve differing files explicitly; never overwrite an existing tree blindly.

After importing or updating skills, check `git status --short --untracked-files=all
.agents` and ignored files before committing. Skill updates now modify this
checkout directly.
