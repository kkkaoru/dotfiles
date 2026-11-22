#!/usr/bin/env bash
# Everything this package must pass before it is installed:
#   format, lint, SourceKit diagnostics, a warning-free build, tests, and the coverage gate.
set -euo pipefail
cd "$(dirname "$0")/.."

echo '== swift format'
swift format lint --strict --recursive Sources Tests Package.swift

echo '== swiftlint'
swiftlint lint --strict --quiet

echo '== sourcekit'
python3 scripts/sourcekit-lint.py

echo '== build (warnings are errors)'
swift build -Xswiftc -warnings-as-errors

echo '== test'
swift test -Xswiftc -warnings-as-errors --enable-code-coverage

echo '== coverage'
python3 scripts/coverage-gate.py "$(swift test --show-codecov-path)"
