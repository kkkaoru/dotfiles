---
name: git-commit-by-feature
description: Analyze all current Git changes and create small, atomic commits grouped by feature or component using Conventional Commits. Use when the user asks to commit a working tree, split changes into logical commits, commit by feature, or invokes git-commit-by-feature; push only when the request includes --push.
---

# Git Commit by Feature

Create the smallest meaningful commits possible from the current repository changes. Each commit must contain one independently understandable and revertible change.

## Arguments

Treat `--push` in the invocation or user request as permission to push after all commits succeed. Without `--push`, never push.

## Conventional Commits

Use this format:

```text
<type>[optional scope][optional !]: <description>

[optional body]

[optional footer(s)]
```

Choose the type from:

- `feat`: new user-visible behavior
- `fix`: bug fix
- `build`: build system or dependency changes
- `chore`: maintenance that does not modify source or tests
- `ci`: CI configuration or scripts
- `docs`: documentation only
- `style`: formatting or other non-semantic changes
- `refactor`: code restructuring without a feature or fix
- `perf`: performance improvement
- `test`: adding or correcting tests
- `revert`: reverting an earlier commit

Rules:

- Add a lowercase noun scope when it clarifies the affected component.
- Write the description in imperative mood, lowercase its first word, and omit the trailing period.
- Add `!` after the type or scope for a breaking change, or add a `BREAKING CHANGE:` footer.
- Use a body when the reason, behavior, migration, or non-obvious tradeoff matters.

Examples:

```text
feat(lang): add Polish translations
fix(parser): prevent concurrent request races
chore!: drop Node 6 support
```

## Workflow

### 1. Establish repository state

Run read-only Git commands first:

```bash
git status --short
git log --oneline -5
git diff --stat
git diff --numstat
git diff --cached --stat
git diff --cached --numstat
```

If there are no staged, unstaged, or untracked changes, tell the user and stop.

Record the initial change set. If a merge, rebase, cherry-pick, or revert is in progress, or if another process appears to be changing the same files, stop and ask before committing unless the user explicitly requested that state.

### 2. Inspect every change

Do not plan commits from filenames or statistics alone.

- Read the full staged and unstaged diff for every tracked file with `git diff --cached -- <path>` and `git diff -- <path>`.
- Inspect the contents of every relevant untracked file.
- Identify separate concerns even when they occur in one file.
- Note dependencies between changes, generated files, tests, documentation, configuration, and lockfiles.
- Check recent commit messages for repository-specific type and scope conventions.
- Do not stage likely secrets, credentials, local runtime state, or unrelated generated artifacts. Stop and alert the user if such files are present and their intent is unclear.

### 3. Plan atomic commits

Partition the initial change set by semantic purpose, not by directory alone.

- Prefer more small commits over fewer bundled commits.
- Keep refactors separate from behavior changes.
- Keep dependency or lockfile updates with the dependency change they represent, but separate unrelated dependency updates.
- Keep tests with the behavior they verify when that makes the commit self-contained; use a separate `test` commit only when the test change is independently meaningful.
- Split unrelated hunks from the same file using selective staging.
- Order commits so each commit is coherent and the sequence respects dependencies.

Before staging, briefly state the planned commit groups and their proposed messages. If the grouping is ambiguous in a way that could change user intent, ask instead of guessing.

### 4. Commit each group iteratively

For each group:

1. Stage only its files or hunks. Use path-specific `git add` for whole files and interactive or patch-based staging for partial files.
2. Review exactly what is staged:

   ```bash
   git diff --cached --stat
   git diff --cached
   ```

3. Confirm the staged diff contains one concern and no accidental files.
4. Commit with a Conventional Commit message.
5. Run `git status --short` and continue with the next group.

Preserve any intentionally staged user changes unless they are part of the agreed grouping. Never use destructive cleanup, bypass hooks, amend an existing commit, or rewrite history unless the user explicitly asks. If a hook or commit fails, diagnose it; do not use `--no-verify` as a shortcut.

If new or changed files appear after the initial snapshot and may come from another process or agent, do not absorb them silently. Leave them untouched and report them, or ask the user when ownership is unclear.

### 5. Verify and report

After the planned commits:

```bash
git status --short
git log --oneline -n <created-commit-count>
```

Report:

- each created commit hash and subject;
- any intentionally remaining changes or blockers;
- whether the working tree is clean.

If `--push` was requested, run `git push` only after all commits and verification succeed, then report the pushed remote and branch. Otherwise, remind the user that the commits remain local and can be pushed manually.
