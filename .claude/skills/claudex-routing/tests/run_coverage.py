#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["coverage==7.15.2"]
# ///
"""Run routing tests and enforce statement/branch coverage."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

from coverage import Coverage

MINIMUM_COVERAGE = 95.0
SKILL_ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    coverage = Coverage(
        branch=True,
        data_file=None,
        source=[str(SKILL_ROOT / "scripts")],
    )
    coverage.start()
    tests = unittest.defaultTestLoader.discover(
        str(SKILL_ROOT / "tests"), pattern="test_*.py"
    )
    result = unittest.TextTestRunner(verbosity=2).run(tests)
    coverage.stop()
    percentage = coverage.report(show_missing=True)
    if not result.wasSuccessful():
        return 1
    if percentage < MINIMUM_COVERAGE:
        print(
            f"coverage {percentage:.2f}% is below {MINIMUM_COVERAGE:.0f}%",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
