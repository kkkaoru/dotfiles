#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["coverage==7.15.2"]
# ///
"""Run Git hook tests with statement and branch coverage gates."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

from coverage import Coverage

MINIMUM_COVERAGE = 95.0
ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    coverage = Coverage(
        branch=True,
        data_file=None,
        source=[str(ROOT)],
        omit=[str(ROOT / "tests/*")],
    )
    coverage.start()
    tests = unittest.defaultTestLoader.discover(str(ROOT / "tests"), "test_*.py")
    result = unittest.TextTestRunner(verbosity=2).run(tests)
    coverage.stop()
    percentage = coverage.report(show_missing=True)
    if not result.wasSuccessful() or percentage < MINIMUM_COVERAGE:
        print(f"required coverage: {MINIMUM_COVERAGE:.0f}%", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
