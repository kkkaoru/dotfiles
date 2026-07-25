from __future__ import annotations

import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import quality_hook as quality  # noqa: E402


class GitFixture(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        subprocess.run(("git", "init", "-q", str(self.root)), check=True)
        subprocess.run(("git", "config", "user.name", "Test"), cwd=self.root, check=True)
        subprocess.run(
            ("git", "config", "user.email", "test@example.com"), cwd=self.root, check=True
        )
        # Fixture commits must not recurse into this repository's global quality domains.
        subprocess.run(
            ("git", "config", "hook.dotfiles-pre-commit.enabled", "false"),
            cwd=self.root,
            check=True,
        )
        subprocess.run(
            ("git", "config", "hook.dotfiles-pre-push.enabled", "false"),
            cwd=self.root,
            check=True,
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_commit(self, path: str, content: str) -> str:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        subprocess.run(("git", "add", path), cwd=self.root, check=True)
        subprocess.run(("git", "commit", "-qm", path), cwd=self.root, check=True)
        return quality.git(self.root, "rev-parse", "HEAD")


class PathTests(GitFixture):
    def test_finds_staged_and_pushed_paths(self) -> None:
        first = self.write_commit("first.txt", "first\n")
        (self.root / "second.json").write_text("{}\n", encoding="utf-8")
        subprocess.run(("git", "add", "second.json"), cwd=self.root, check=True)
        self.assertEqual(quality.pre_commit_paths(self.root), {"second.json"})
        subprocess.run(("git", "commit", "-qm", "second"), cwd=self.root, check=True)
        second = quality.git(self.root, "rev-parse", "HEAD")
        stream = io.StringIO(f"refs/heads/main {second} refs/heads/main {first}\n")
        self.assertEqual(quality.pre_push_paths(self.root, stream), {"second.json"})

    def test_new_push_and_manual_fallback_use_valid_bases(self) -> None:
        head = self.write_commit("all.txt", "all\n")
        new_branch = io.StringIO(
            f"refs/heads/new {head} refs/heads/new {quality.ZERO_OID}\n"
        )
        self.assertEqual(quality.pre_push_paths(self.root, new_branch), {"all.txt"})
        self.assertEqual(quality.pre_push_paths(self.root, io.StringIO()), {"all.txt"})

    def test_ignores_deletions_and_malformed_push_lines(self) -> None:
        stream = io.StringIO(
            f"refs/heads/x {quality.ZERO_OID} refs/heads/x {'1' * 40}\nmalformed\n"
        )
        self.assertEqual(quality.push_ranges(stream), [])
        self.assertTrue(quality.is_zero_oid("0" * 64))
        self.assertFalse(quality.is_zero_oid(""))
        self.assertFalse(quality.is_zero_oid("1" * 40))
        self.assertEqual(quality.listed_paths("a\n\nb\n"), {"a", "b"})


class ValidationTests(GitFixture):
    def test_parses_json_toml_git_config_and_shell(self) -> None:
        files = {
            "data.json": json.dumps({"ok": True}),
            "data.toml": "ok = true\n",
            ".gitconfig": "[user]\nname = Test\n",
            "valid.sh": "#!/bin/bash\nset -eu\n",
        }
        for name, content in files.items():
            (self.root / name).write_text(content, encoding="utf-8")
        with mock.patch.object(quality.subprocess, "run") as run:
            quality.validate_changed_files(self.root, set(files))
        self.assertEqual(run.call_count, 2)

    def test_rejects_invalid_data_and_agent_tracking(self) -> None:
        (self.root / "bad.json").write_text("{", encoding="utf-8")
        with self.assertRaises(json.JSONDecodeError):
            quality.validate_changed_files(self.root, {"bad.json"})
        agents = self.root / ".claude/agents"
        agents.mkdir(parents=True)
        (agents / "new.md").write_text("missing frontmatter", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "tracked"):
            quality.validate_agents(self.root)

    def test_accepts_tracked_agents_and_rejects_bad_frontmatter(self) -> None:
        valid = "---\nname: valid\n---\nPrompt\n"
        self.write_commit(".claude/agents/valid.md", valid)
        quality.validate_agents(self.root)
        quality.validate_changed_files(self.root, {".gitignore"})
        (self.root / ".claude/agents/valid.md").write_text("---\nname: invalid\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "frontmatter"):
            quality.validate_agents(self.root)


class SelectionTests(unittest.TestCase):
    def test_commit_selects_fast_checks_for_every_domain(self) -> None:
        paths = {
            ".gitconfig",
            ".claude/agents/worker.md",
            "tools/claudex-agent-adapter/src/lib.rs",
            "tools/sleep-control/Sources/App.swift",
            "tools/lid-display-watcher/Sources/main.swift",
            "package.json",
        }
        selected = quality.checks("pre-commit", paths)
        commands = [check.command for check in selected]
        self.assertIn(("cargo", "fmt-check"), commands)
        self.assertIn(("cargo", "lint"), commands)
        self.assertNotIn(("cargo", "test-all"), commands)
        self.assertEqual(sum(command == ("make", "lint") for command in commands), 2)
        self.assertTrue(any(command[0] == "bun" for command in commands))
        self.assertEqual(sum(command[0] == "uv" for command in commands), 2)
        self.assertEqual(sum(command[0] == "uvx" for command in commands), 2)

    def test_push_adds_tests_and_coverage(self) -> None:
        selected = quality.checks(
            "pre-push", {"tools/claudex-agent-adapter/src/lib.rs"}
        )
        aliases = [check.command[1] for check in selected]
        self.assertEqual(
            aliases,
            ["fmt-check", "lint", "test-all", "coverage", "coverage-branch"],
        )
        swift = quality.checks("pre-push", {"tools/sleep-control/Package.swift"})
        self.assertEqual(swift[0].command, ("make", "verify"))
        self.assertFalse(quality.touches({"README.md"}, "tools/"))

    @mock.patch.object(quality.subprocess, "run")
    def test_runs_commands_in_their_directories(self, run: mock.Mock) -> None:
        quality.run_checks(Path("/repo"), [quality.Check("sub", ("tool", "check"))])
        run.assert_called_once_with(("tool", "check"), cwd=Path("/repo/sub"), check=True)


class MainTests(unittest.TestCase):
    def test_repository_config_declares_config_based_hooks(self) -> None:
        repository = ROOT.parents[1]
        output = subprocess.run(
            (
                "git",
                "config",
                "--file",
                str(repository / ".gitconfig"),
                "--get-regexp",
                r"^hook\.",
            ),
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        self.assertIn("hook.staged-whitespace.event pre-commit", output)
        self.assertIn("hook.dotfiles-pre-commit.event pre-commit", output)
        self.assertIn("hook.dotfiles-pre-push.event pre-push", output)
        self.assertIn("dotfiles-git-quality", output)

    def test_launcher_resolves_its_python_module_through_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            launcher = Path(directory) / "dotfiles-git-quality"
            launcher.symlink_to(ROOT / "dotfiles-git-quality")
            result = subprocess.run(
                (str(launcher), "--help"), check=True, capture_output=True, text=True
            )
        self.assertIn("pre-commit", result.stdout)

    def test_parses_hook_arguments_and_runs_commit_flow(self) -> None:
        options = quality.parse_arguments(["pre-push", "origin", "url"])
        self.assertEqual(options.hook_arguments, ["origin", "url"])
        with (
            mock.patch.object(quality, "git", return_value="/repo"),
            mock.patch.object(quality, "pre_commit_paths", return_value={"README.md"}),
            mock.patch.object(quality, "validate_changed_files") as validate,
            mock.patch.object(quality, "run_checks") as run,
        ):
            self.assertEqual(quality.main(["pre-commit"]), 0)
        validate.assert_called_once_with(Path("/repo"), {"README.md"})
        run.assert_called_once()

    def test_main_uses_pre_push_input(self) -> None:
        stream = io.StringIO("updates")
        with (
            mock.patch.object(quality, "git", return_value="/repo"),
            mock.patch.object(quality, "pre_push_paths", return_value=set()) as pushed,
            mock.patch.object(quality, "validate_changed_files"),
            mock.patch.object(quality, "run_checks"),
        ):
            self.assertEqual(quality.main(["pre-push", "origin"], stream), 0)
        pushed.assert_called_once_with(Path("/repo"), stream)


if __name__ == "__main__":
    unittest.main()
