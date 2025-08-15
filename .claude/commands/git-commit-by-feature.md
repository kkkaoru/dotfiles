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

1. **Analyze the line changes** from the diff output:
   - Look at added/removed line counts per file
   - Consider the magnitude of changes
   - Identify if changes are additions, deletions, or modifications

2. **Group related changes** by:
   - Feature/module (based on directory structure)
   - File type (config, scripts, docs, etc.)
   - Logical components

3. **For each group**:
   - Stage the relevant files using `git add`
   - Create a commit message following Conventional Commits specification:
     - Format: type[optional scope]: description
     - Choose type based on the nature of changes and line counts:
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

4. **After all commits**:
   - Show a summary of created commits
   - $ARGUMENTS contains "--push": Push to remote with `git push`
   - Otherwise: Remind user they can push manually

## Important Guidelines
- Follow the Conventional Commits specification from .rules/conventional-commits.md
- Each commit should be atomic and contain only related changes
- Commit messages must use the format: type[optional scope]: description
- Use lowercase for type and scope
- Use imperative mood in the description
- Don't commit unrelated changes together
- If there are no changes, inform the user

Arguments provided: $ARGUMENTS