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
        subprocess.run(
            ("git", "config", "hook.staged-whitespace.enabled", "false"),
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
        # Fixture repos have no origin remote, so new refs still diff vs empty tree.
        self.assertEqual(quality.pre_push_paths(self.root, new_branch), {"all.txt"})
        self.assertEqual(quality.pre_push_paths(self.root, io.StringIO()), {"all.txt"})
        self.assertEqual(
            quality.pre_push_base(self.root, quality.ZERO_OID),
            quality.empty_tree(self.root),
        )

    def test_new_push_uses_origin_master_when_remote_exists(self) -> None:
        base = self.write_commit("base.txt", "base\n")
        subprocess.run(
            ("git", "remote", "add", "origin", str(self.root)),
            cwd=self.root,
            check=True,
        )
        subprocess.run(("git", "fetch", "-q", "origin"), cwd=self.root, check=True)
        head = self.write_commit("feature.txt", "feature\n")
        new_branch = io.StringIO(
            f"refs/heads/feature {head} refs/heads/feature {quality.ZERO_OID}\n"
        )
        self.assertEqual(quality.pre_push_base(self.root, quality.ZERO_OID), base)
        self.assertEqual(quality.pre_push_paths(self.root, new_branch), {"feature.txt"})

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
        selected = quality.checks(Path("/repo"), "pre-commit", paths)
        commands = [check.command for check in selected]
        self.assertIn(
            ("rustfmt", "--edition", "2024", "--check", "--files-with-diff", "src/lib.rs"),
            commands,
        )
        self.assertIn(("cargo", "lint"), commands)
        self.assertNotIn(("cargo", "test-all"), commands)
        self.assertEqual(sum(command == ("make", "lint") for command in commands), 2)
        self.assertTrue(any(command[0] == "bun" for command in commands))
        self.assertEqual(sum(command[0] == "uv" for command in commands), 2)
        self.assertEqual(sum(command[0] == "uvx" for command in commands), 2)

    def test_push_adds_tests_and_coverage(self) -> None:
        selected = quality.checks(
            Path("/repo"), "pre-push", {"tools/claudex-agent-adapter/src/lib.rs"}
        )
        commands = [check.command for check in selected]
        self.assertEqual(
            commands,
            [
                ("rustfmt", "--edition", "2024", "--check", "--files-with-diff", "src/lib.rs"),
                ("cargo", "lint"),
                ("cargo", "test-all"),
                ("cargo", "coverage"),
                ("cargo", "coverage-branch"),
            ],
        )
        swift = quality.checks(Path("/repo"), "pre-push", {"tools/sleep-control/Package.swift"})
        self.assertEqual(swift[0].command, ("make", "verify"))
        self.assertFalse(quality.touches({"README.md"}, "tools/"))

    def test_skips_the_fmt_gate_but_still_lints_when_only_docs_change(self) -> None:
        selected = quality.checks(
            Path("/repo"), "pre-commit", {"tools/claudex-agent-adapter/README.md"}
        )
        commands = [check.command for check in selected]
        self.assertFalse(any(command[0] in ("rustfmt", "rustup") for command in commands))
        self.assertIn(("cargo", "lint"), commands)

    def test_adapter_checks_always_run_from_the_crate_root(self) -> None:
        selected = quality.checks(
            Path("/repo"),
            "pre-commit",
            {"tools/claudex-agent-adapter/src/anthropic/stream/builder/mod.rs"},
        )
        self.assertTrue(selected)
        self.assertTrue(
            all(check.directory == "tools/claudex-agent-adapter" for check in selected)
        )

    def test_pins_the_toolchain_this_repository_actually_declares_for_the_adapter(self) -> None:
        repository = ROOT.parents[1]
        self.assertEqual(
            quality.cargo_toolchain(repository, "tools/claudex-agent-adapter"), "1.97.1"
        )
        selected = quality.checks(
            repository, "pre-commit", {"tools/claudex-agent-adapter/src/lib.rs"}
        )
        commands = [check.command for check in selected]
        self.assertIn(("cargo", "+1.97.1", "lint"), commands)
        self.assertIn(
            (
                "rustup",
                "run",
                "1.97.1",
                "rustfmt",
                "--edition",
                "2024",
                "--check",
                "--files-with-diff",
                "src/lib.rs",
            ),
            commands,
        )

    @mock.patch.object(quality.subprocess, "run")
    def test_runs_commands_in_their_directories(self, run: mock.Mock) -> None:
        quality.run_checks(Path("/repo"), [quality.Check("sub", ("tool", "check"))])
        run.assert_called_once_with(("tool", "check"), cwd=Path("/repo/sub"), check=True)


class ToolchainTests(unittest.TestCase):
    def test_reads_the_pinned_channel_and_falls_back_to_none(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "tools/example"
            crate.mkdir(parents=True)
            (crate / "rust-toolchain.toml").write_text(
                '[toolchain]\nchannel = "1.97.1"\n', encoding="utf-8"
            )
            self.assertEqual(quality.cargo_toolchain(root, "tools/example"), "1.97.1")
            self.assertIsNone(quality.cargo_toolchain(root, "tools/missing"))

    def test_pins_cargo_only_when_a_toolchain_is_known(self) -> None:
        self.assertEqual(quality.pinned_cargo("1.97.1", "lint"), ("cargo", "+1.97.1", "lint"))
        self.assertEqual(quality.pinned_cargo(None, "lint"), ("cargo", "lint"))


class TouchedRustFilesTests(unittest.TestCase):
    def test_keeps_only_touched_rust_paths_relative_to_the_crate(self) -> None:
        paths = {
            "tools/claudex-agent-adapter/src/lib.rs",
            "tools/claudex-agent-adapter/README.md",
            "tools/other/src/lib.rs",
        }
        self.assertEqual(
            quality.touched_rust_files(paths, "tools/claudex-agent-adapter"), ["src/lib.rs"]
        )


class RustfmtGateTests(unittest.TestCase):
    def test_skips_when_nothing_rust_changed(self) -> None:
        self.assertIsNone(quality.rustfmt_gate(Path("/repo"), "crate", "1.97.1", []))

    def test_builds_a_toolchain_pinned_files_with_diff_command(self) -> None:
        check = quality.rustfmt_gate(Path("/repo"), "crate", "1.97.1", ["src/lib.rs"])
        assert check is not None
        self.assertEqual(
            check.command,
            (
                "rustup",
                "run",
                "1.97.1",
                "rustfmt",
                "--edition",
                "2024",
                "--check",
                "--files-with-diff",
                "src/lib.rs",
            ),
        )
        self.assertEqual(check.allow_paths, {Path("/repo/crate/src/lib.rs").resolve()})

    def test_omits_the_toolchain_wrapper_when_none_is_pinned(self) -> None:
        check = quality.rustfmt_gate(Path("/repo"), "crate", None, ["src/lib.rs"])
        assert check is not None
        self.assertEqual(
            check.command,
            ("rustfmt", "--edition", "2024", "--check", "--files-with-diff", "src/lib.rs"),
        )


class RunChecksFilteringTests(unittest.TestCase):
    def make_check(self) -> quality.Check:
        return quality.Check(
            "crate",
            ("rustfmt", "x"),
            frozenset({Path("/repo/crate/touched.rs").resolve()}),
        )

    def test_ignores_diffs_reported_for_files_nobody_touched(self) -> None:
        completed = subprocess.CompletedProcess(
            ("rustfmt", "x"), 1, stdout="/repo/crate/other.rs\n", stderr=""
        )
        with mock.patch.object(quality.subprocess, "run", return_value=completed):
            quality.run_checks(Path("/repo"), [self.make_check()])

    def test_fails_when_a_touched_file_has_a_diff(self) -> None:
        completed = subprocess.CompletedProcess(
            ("rustfmt", "x"), 1, stdout="/repo/crate/touched.rs\n", stderr=""
        )
        with mock.patch.object(quality.subprocess, "run", return_value=completed):
            with self.assertRaises(subprocess.CalledProcessError):
                quality.run_checks(Path("/repo"), [self.make_check()])

    def test_fails_on_a_genuine_parse_error_even_without_a_files_with_diff_match(self) -> None:
        completed = subprocess.CompletedProcess(
            ("rustfmt", "x"), 1, stdout="", stderr="error: unclosed delimiter\n"
        )
        with mock.patch.object(quality.subprocess, "run", return_value=completed):
            with self.assertRaises(subprocess.CalledProcessError):
                quality.run_checks(Path("/repo"), [self.make_check()])

    def test_passes_cleanly_when_nothing_needs_reformatting(self) -> None:
        completed = subprocess.CompletedProcess(("rustfmt", "x"), 0, stdout="", stderr="")
        with mock.patch.object(quality.subprocess, "run", return_value=completed):
            quality.run_checks(Path("/repo"), [self.make_check()])


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
        hooks_path = subprocess.run(
            (
                "git",
                "config",
                "--file",
                str(repository / ".gitconfig"),
                "--get",
                "core.hooksPath",
            ),
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(hooks_path.returncode, 0)

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

    def test_main_returns_nonzero_when_quality_fails(self) -> None:
        with (
            mock.patch.object(quality, "git", return_value="/repo"),
            mock.patch.object(quality, "pre_commit_paths", return_value={"bad.json"}),
            mock.patch.object(
                quality,
                "validate_changed_files",
                side_effect=ValueError("invalid agent frontmatter"),
            ),
        ):
            self.assertEqual(quality.main(["pre-commit"]), 1)

    def test_main_returns_nonzero_when_check_command_fails(self) -> None:
        failed = subprocess.CalledProcessError(1, ("cargo", "lint"))
        with (
            mock.patch.object(quality, "git", return_value="/repo"),
            mock.patch.object(
                quality, "pre_commit_paths", return_value={"tools/x/a.rs"}
            ),
            mock.patch.object(quality, "validate_changed_files"),
            mock.patch.object(quality, "checks", return_value=[]),
            mock.patch.object(quality, "run_checks", side_effect=failed),
        ):
            self.assertEqual(quality.main(["pre-commit"]), 1)


if __name__ == "__main__":
    unittest.main()
