#!/usr/bin/env python3
"""Fail when a PlamoCore file is below the line-coverage threshold.

  coverage-gate.py <codecov.json> [--minimum 95] [--prefix Sources/PlamoCore/]
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

MINIMUM = 95.0
PREFIX = "Sources/PlamoCore/"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--minimum", type=float, default=MINIMUM)
    parser.add_argument("--prefix", default=PREFIX)
    args = parser.parse_args()

    report = json.loads(args.report.read_text(encoding="utf-8"))
    files = report["data"][0]["files"]
    failures = 0
    covered_total = 0
    counted_total = 0
    for entry in files:
        filename = str(entry["filename"])
        if args.prefix not in filename:
            continue
        lines = entry["summary"]["lines"]
        covered = int(lines["covered"])
        count = int(lines["count"])
        covered_total += covered
        counted_total += count
        percent = 100.0 * covered / count if count else 100.0
        name = filename.split(args.prefix)[-1]
        print(f"{'ok  ' if percent >= args.minimum else 'FAIL'} {percent:6.2f}% {covered:5}/{count:<5} {name}")
        failures += percent < args.minimum

    if not counted_total:
        print(f"coverage-gate: no files matched {args.prefix}", file=sys.stderr)
        return 1
    total = 100.0 * covered_total / counted_total
    print(f"{'ok  ' if total >= args.minimum else 'FAIL'} {total:6.2f}% {covered_total:5}/{counted_total:<5} total")
    if total < args.minimum:
        failures += 1
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
