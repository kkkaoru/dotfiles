#!/usr/bin/env python3

import json
from copy import deepcopy
from pathlib import Path
import sys


def load_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a JSON object")
    return value


def merge_providers(merged: dict, override: list[dict]) -> bool:
    changed = False
    providers = list(merged.get("providers", []))
    if not isinstance(providers, list):
        raise ValueError("base provider config must define providers as an array")

    index: dict[str, dict] = {}
    for provider in providers:
        if isinstance(provider, dict):
            provider_id = provider.get("id")
            if isinstance(provider_id, str):
                index[provider_id] = provider

    for entry in override:
        if not isinstance(entry, dict):
            raise ValueError("override providers must be objects")
        provider_id = entry.get("id")
        if not isinstance(provider_id, str) or not provider_id:
            raise ValueError("override provider entries must include id")
        existing = index.get(provider_id)
        if existing is None:
            new_provider = dict(entry)
            index[provider_id] = new_provider
            providers.append(new_provider)
            changed = True
            continue
        before = dict(existing)
        existing.update(entry)
        if existing != before:
            changed = True

    merged["providers"] = providers
    return changed


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit(
            "usage: resolve-provider-local-config.py <base-config> <override> <output>"
        )

    base_path, override_path, output_path = (Path(path) for path in sys.argv[1:4])

    base = load_json(base_path)
    if base.get("version") != 1:
        raise ValueError("base provider config must set version to 1")

    override = load_json(override_path)
    if override.get("version") != 1:
        raise ValueError("local provider override must set version to 1")

    merged = deepcopy(base)
    changed = False

    if "mainProviders" in override:
        main_providers = override["mainProviders"]
        if not isinstance(main_providers, list) or any(
            not isinstance(provider, str) or not provider for provider in main_providers
        ):
            raise ValueError("override mainProviders must be a list of provider IDs")
        if merged.get("mainProviders") != main_providers:
            merged["mainProviders"] = list(main_providers)
            changed = True

    if "providers" in override:
        override_providers = override["providers"]
        if not isinstance(override_providers, list):
            raise ValueError("override providers must be an array")
        if merge_providers(merged, override_providers):
            changed = True

    if "fallback" in override:
        merged["fallback"] = override["fallback"]
        if merged.get("fallback") != base.get("fallback"):
            changed = True

    if changed:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(
            json.dumps(merged, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(output_path)
        return 0

    print(base_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
