# Shared Agent Skills

`~/.agents` is a symlink to this entire directory. Installed skill definitions,
references, scripts, and `.skill-lock.json` are managed here.

## Global versus project-selected skills

- `skills/`: generic Python/Rust/Swift/TypeScript coding rules, Git commits, skill
  discovery, agmsg/history guidance, and `project-skills` for local selection.
- `skills-stroage/`: 40 optional Apple, Cloudflare, MotherDuck, GPUI and web-performance
  skills. The spelling `stroage` is intentional. This is not an automatic global
  discovery root. Do not symlink all its entries back into `skills/`.
- A target project's `.agents/skills/`: complete copies of only its selected skills.
  Copies are portable and can be reviewed/committed with that project. They do not
  auto-update from storage. Existing project skills are never overwritten.

```sh
bash ~/.agents/skills/project-skills/scripts/manage.sh list
bash ~/.agents/skills/project-skills/scripts/manage.sh add /absolute/project cloudflare
```

Read the `project-skills` skill before selecting dependencies. Its helper installs
one skill per call and never executes copied code. Agent reload/new session is
needed after changing discovery. Pi's old domain-wide exclusions were removed so
project-selected copies are discoverable; agmsg/history remain routed on demand.
The Executor setup recipe also includes storage as a read-only catalog root; an
existing registration is not silently changed by moving directories.

`.skill-lock.json` retains upstream provenance at the shared-home level. Upstream
installer defaults do not understand this split: review updates and route domain
skills to storage rather than recreating globally active copies.

`create-symlinks.sh` installs the top-level link. It refuses an existing real
`~/.agents` directory: back it up and merge its contents first, preserving managed
skill directories rather than replacing them with self-referential symlinks.
Resolve differing files explicitly; never overwrite an existing tree blindly.

The agmsg `db/`, `run/`, and `teams/` directories remain in this tree locally but
are ignored by Git: they contain conversations, live process state, local team
configuration, and machine-specific plugin trust. Moving live SQLite state must
preserve file inodes (same-filesystem rename), not replace it with a stale copy.
Credentials must never be committed. The pi plugin link points at the existing
repository-owned implementation under `tools/pi-agmsg-extension`.

After importing or updating skills, check `git status --short --untracked-files=all
.agents` and ignored files before committing. Skill updates now modify this
checkout directly.
