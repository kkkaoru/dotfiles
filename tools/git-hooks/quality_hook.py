#!/usr/bin/env python3
"""Run change-aware quality gates for config-based Git hooks."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, TextIO

ZERO_OID = "0" * 40


@dataclass(frozen=True)
class Check:
    """One command and its repository-relative working directory."""

    directory: str
    command: tuple[str, ...]


def git(root: Path, *arguments: str, input_text: str | None = None) -> str:
    """Run Git in the repository and return normalized standard output."""
    return subprocess.run(
        ("git", *arguments),
        cwd=root,
        input=input_text,
        text=True,
        check=True,
        capture_output=True,
    ).stdout.strip()


def listed_paths(output: str) -> set[str]:
    """Convert newline-delimited Git paths into a set."""
    return {path for path in output.splitlines() if path}


def pre_commit_paths(root: Path) -> set[str]:
    """Return added, copied, modified, or renamed staged paths."""
    return listed_paths(
        git(root, "diff", "--cached", "--name-only", "--diff-filter=ACMR")
    )


def empty_tree(root: Path) -> str:
    """Return the repository's object-format-specific empty tree ID."""
    return git(root, "hash-object", "-t", "tree", "--stdin", input_text="")


def push_ranges(stream: TextIO) -> list[tuple[str, str]]:
    """Parse live pre-push updates into comparable old/new object IDs."""
    ranges = []
    for line in stream:
        fields = line.split()
        if len(fields) != 4 or is_zero_oid(fields[1]):
            continue
        ranges.append((fields[3], fields[1]))
    return ranges


def is_zero_oid(value: str) -> bool:
    """Recognize deletion/new-ref sentinels for SHA-1 and SHA-256 repositories."""
    return bool(value) and not value.strip("0")


def pre_push_base(root: Path, old: str) -> str:
    """Prefer origin/master over the empty tree for new refs when origin exists.

    Diffing a brand-new branch against the empty tree runs ``git diff --check``
    on the entire history, which fails on pre-existing vendored trailing
    whitespace (e.g. ``.codex/skills``). Only consult remotes configured in
    *this* repository so isolated fixture repos still use the empty tree.
    """
    if not is_zero_oid(old):
        return old
    try:
        remotes = git(root, "remote").splitlines()
    except subprocess.CalledProcessError:
        remotes = []
    if "origin" not in remotes:
        return empty_tree(root)
    for ref_name in (
        "refs/remotes/origin/master",
        "refs/remotes/origin/main",
    ):
        try:
            return git(root, "rev-parse", "--verify", ref_name)
        except subprocess.CalledProcessError:
            continue
    return empty_tree(root)


def pre_push_paths(root: Path, stream: TextIO) -> set[str]:
    """Return paths changed by every ref update in a push."""
    ranges = push_ranges(stream)
    if not ranges:
        try:
            upstream = git(root, "rev-parse", "--verify", "@{upstream}")
        except subprocess.CalledProcessError:
            upstream = ZERO_OID
        ranges = [(upstream, git(root, "rev-parse", "HEAD"))]
    paths: set[str] = set()
    for old, new in ranges:
        base = pre_push_base(root, old)
        git(root, "diff", "--check", base, new)
        paths.update(listed_paths(git(root, "diff", "--name-only", base, new)))
    return paths


def validate_agents(root: Path) -> None:
    """Reject malformed or untracked Claude agent definitions."""
    agents = root / ".claude/agents"
    tracked = listed_paths(git(root, "ls-files", ".claude/agents/**"))
    actual = {
        path.relative_to(root).as_posix() for path in agents.rglob("*.md") if path.is_file()
    }
    if actual != tracked:
        raise ValueError("every .claude/agents definition must be tracked by Git")
    for relative in sorted(actual):
        lines = (root / relative).read_text(encoding="utf-8").splitlines()
        if len(lines) < 4 or lines[0] != "---" or "---" not in lines[1:]:
            raise ValueError(f"invalid agent frontmatter: {relative}")


def validate_changed_files(root: Path, paths: set[str]) -> None:
    """Parse data files and statically inspect changed shell scripts."""
    for relative in sorted(paths):
        path = root / relative
        if not path.is_file():
            continue
        if path.suffix == ".json":
            json.loads(path.read_text(encoding="utf-8"))
        elif path.suffix == ".toml":
            tomllib.loads(path.read_text(encoding="utf-8"))
    if any(path == ".gitconfig" for path in paths):
        subprocess.run(
            ("git", "config", "--file", str(root / ".gitconfig"), "--list"),
            check=True,
            stdout=subprocess.DEVNULL,
        )
    shell_files = [str(root / path) for path in sorted(paths) if path.endswith(".sh")]
    if shell_files:
        subprocess.run(("shellcheck", *shell_files), check=True)
    if any(path.startswith(".claude/agents/") or path == ".gitignore" for path in paths):
        validate_agents(root)


def touches(paths: set[str], *prefixes: str) -> bool:
    """Return whether a changed path belongs to any quality domain."""
    return any(path.startswith(prefixes) for path in paths)


def checks(event: str, paths: set[str]) -> list[Check]:
    """Select fast commit checks or comprehensive push checks by changed domain."""
    selected: list[Check] = []
    hook_changed = touches(paths, "tools/git-hooks/") or bool(
        paths & {".gitconfig", ".gitignore", "create-symlinks.sh"}
    )
    routing_changed = touches(paths, ".claude/", ".config/claudex/")
    adapter_changed = touches(paths, "tools/claudex-agent-adapter/")
    if hook_changed:
        selected.extend(
            (
                Check(
                    "tools/git-hooks",
                    (
                        "uvx",
                        "--from",
                        "ruff==0.12.12",
                        "ruff",
                        "check",
                        "quality_hook.py",
                        "tests",
                    ),
                ),
                Check("tools/git-hooks", ("uv", "run", "tests/run_coverage.py")),
            )
        )
    if routing_changed:
        selected.extend(
            (
                Check(
                    ".claude/skills/claudex-routing",
                    (
                        "uvx",
                        "--from",
                        "ruff==0.12.12",
                        "ruff",
                        "check",
                        "scripts",
                        "tests",
                    ),
                ),
                Check(
                    ".claude/skills/claudex-routing",
                    ("uv", "run", "tests/run_coverage.py"),
                ),
            )
        )
    if adapter_changed:
        selected.extend(
            Check("tools/claudex-agent-adapter", ("cargo", alias))
            for alias in ("fmt-check", "lint")
        )
        if event == "pre-push":
            selected.extend(
                Check("tools/claudex-agent-adapter", ("cargo", alias))
                for alias in ("test-all", "coverage", "coverage-branch")
            )
    for project in ("sleep-control", "lid-display-watcher"):
        if touches(paths, f"tools/{project}/"):
            target = "verify" if event == "pre-push" else "lint"
            selected.append(Check(f"tools/{project}", ("make", target)))
    if paths & {"package.json", "bun.lock"}:
        selected.append(
            Check(
                ".",
                (
                    "bun",
                    "install",
                    "--dry-run",
                    "--frozen-lockfile",
                    "--ignore-scripts",
                ),
            )
        )
    return selected


def run_checks(root: Path, selected: Iterable[Check]) -> None:
    """Run checks in deterministic order and stop at the first failure."""
    for check in selected:
        print(f"quality: {' '.join(check.command)} ({check.directory})", flush=True)
        subprocess.run(check.command, cwd=root / check.directory, check=True)


def parse_arguments(arguments: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("event", choices=("pre-commit", "pre-push"))
    parser.add_argument("hook_arguments", nargs="*")
    return parser.parse_args(arguments)


def main(arguments: list[str] | None = None, stream: TextIO = sys.stdin) -> int:
    try:
        options = parse_arguments(arguments)
        root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
        paths = (
            pre_commit_paths(root)
            if options.event == "pre-commit"
            else pre_push_paths(root, stream)
        )
        validate_changed_files(root, paths)
        run_checks(root, checks(options.event, paths))
    except (
        json.JSONDecodeError,
        OSError,
        subprocess.CalledProcessError,
        tomllib.TOMLDecodeError,
        ValueError,
    ) as error:
        print(f"quality: {error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":  # pragma: no cover - exercised through the installed hook
    raise SystemExit(main())
