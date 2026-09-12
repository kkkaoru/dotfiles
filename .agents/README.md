# Shared Agent Skills

`~/.agents` is a symlink to this entire directory. All installed skill definitions,
references, scripts, and `.skill-lock.json` are managed here, including the four
coding/commit skills that were previously linked individually.

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
