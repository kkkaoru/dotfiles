#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RESOLVER = ROOT / "resolve-provider-local-config.py"


class ResolveProviderLocalConfigTests(unittest.TestCase):
    def test_returns_base_config_when_override_changes_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base = root / "providers.json"
            override = root / "providers.local.json"
            output = root / "cache" / "providers.json"
            base.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "piGatewayExtension": "../gateway.ts",
                        "mainProviders": ["gpt"],
                        "providers": [],
                    }
                ),
                encoding="utf-8",
            )
            override.write_text(json.dumps({"version": 1}), encoding="utf-8")

            result = subprocess.run(
                [sys.executable, str(RESOLVER), str(base), str(override), str(output)],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(Path(result.stdout.strip()), base)
            self.assertFalse(output.exists())

    def test_resolves_extension_paths_from_their_source_configs(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base_directory = root / "base"
            override_directory = root / "override"
            output = root / "cache" / "providers.json"
            base_directory.mkdir()
            override_directory.mkdir()
            base = base_directory / "providers.json"
            override = override_directory / "providers.local.json"
            base.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "piGatewayExtension": "../extensions/gateway.ts",
                        "mainProviders": ["gpt"],
                        "providers": [
                            {
                                "id": "gpt",
                                "model": "gpt",
                                "backend": "codex-app-server",
                                "piProvider": "openai-codex",
                                "piModel": "gpt",
                                "piExtensions": ["./base-provider.ts"],
                            },
                            {
                                "id": "cursor",
                                "model": "auto",
                                "backend": "configured-acp",
                                "piProvider": "cursor",
                                "piModel": "auto",
                            },
                        ],
                    }
                ),
                encoding="utf-8",
            )
            override.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "mainProviders": ["cursor"],
                        "providers": [
                            {
                                "id": "cursor",
                                "piExtensions": ["./cursor-provider.ts"],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )

            result = subprocess.run(
                [sys.executable, str(RESOLVER), str(base), str(override), str(output)],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(Path(result.stdout.strip()), output)
            resolved = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                resolved["piGatewayExtension"],
                str((base_directory / "../extensions/gateway.ts").resolve()),
            )
            providers = {provider["id"]: provider for provider in resolved["providers"]}
            self.assertEqual(
                providers["gpt"]["piExtensions"],
                [str((base_directory / "base-provider.ts").resolve())],
            )
            self.assertEqual(
                providers["cursor"]["piExtensions"],
                [str((override_directory / "cursor-provider.ts").resolve())],
            )


if __name__ == "__main__":
    unittest.main()
