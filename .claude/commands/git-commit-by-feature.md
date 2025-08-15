---
allowed-tools: Bash(git status), Bash(git diff*), Bash(git add*), Bash(git commit*), Bash(git push*), Bash(git log*), Read
argument-hint: [--push]
description: Intelligently commits changes grouped by feature/component following Conventional Commits
---

# Git Commit by Feature

Analyze the current git changes and create organized, meaningful commits grouped by feature or component.

## Commit Message Rules
@~/.rules/conventional-commits.md

## Current repository status
!git status --short

## Show recent commits for context
!git log --oneline -5

## Analyze changes summary
!git diff --stat

## Analyze detailed changes with line counts
!git diff --numstat

## Task Instructions

Please analyze the changes and create atomic commits following these steps:

1. **Analyze ALL changes in detail**:
   - First, examine the full diff for each file using `git diff <filename>`
   - Look at specific changes within each file, not just file-level statistics
   - Identify if changes can be logically separated even within the same file
   - Consider the semantic meaning of each change

2. **Create the smallest meaningful commits possible**:
   - **IMPORTANT**: Each commit should represent ONE atomic change
   - Even if multiple files are in the same directory, commit them separately if they represent different logical changes
   - For a single file with multiple unrelated changes, consider if they can be staged and committed separately using `git add -p`
   - Never bundle changes just because they're in the same component or directory
   - Each commit should be independently revertable without breaking other functionality

3. **Process changes iteratively**:
   - Start with the most independent, standalone changes
   - Commit configuration changes separately from feature changes
   - Commit dependency updates separately from code changes
   - Keep refactoring separate from functional changes
   - Continue until `git status --short` shows no staged (M, A, D) or untracked (??) files

4. **For each atomic change**:
   - Stage ONLY the files/hunks for this specific change using `git add` (or `git add -p` for partial staging)
   - Create a commit message following Conventional Commits specification:
     - Format: type[optional scope]: description
     - Choose type based on the nature of the specific change:
       - feat: new features (mostly additions)
       - fix: bug fixes (balanced additions/deletions)
       - build: build system or external dependencies
       - chore: maintenance tasks that don't modify src or test files
       - ci: CI configuration files and scripts
       - docs: documentation only changes
       - style: formatting, missing semi colons, etc; no code change
       - refactor: code change that neither fixes a bug nor adds a feature
       - perf: code change that improves performance
       - test: adding missing tests or correcting existing tests
       - revert: reverts a previous commit
     - Add scope in parentheses if changes are focused on a specific component
     - Use imperative mood in description (e.g., "add" not "added")
     - Add ! after type/scope for breaking changes
   - Make the commit with git commit -m
   - Check `git status --short` again to see if more changes remain

5. **After all commits are complete**:
   - Verify that `git status --short` shows no remaining changes
   - Show a summary of all created commits
   - $ARGUMENTS contains "--push": Push to remote with `git push`
   - Otherwise: Remind user they can push manually

## Important Guidelines
- **CRITICAL**: Always prefer MORE commits with SMALLER changes over fewer commits with bundled changes
- Each commit should do ONE thing and do it well
- If you're unsure whether to combine changes, DON'T - make separate commits
- Follow the Conventional Commits specification from .rules/conventional-commits.md
- Each commit should be atomic and contain only related changes
- Commit messages must use the format: type[optional scope]: description
- Use lowercase for type and scope
- Use imperative mood in the description
- A good test: Could this commit be reverted independently without affecting unrelated functionality?
- Continue committing until all staged and untracked files are processed
- If there are no changes, inform the user

Arguments provided: $ARGUMENTS