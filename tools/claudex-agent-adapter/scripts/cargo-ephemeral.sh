#!/bin/sh
# Run Cargo with a per-invocation target directory and remove it on exit.
# This keeps builds/tests from accumulating Rust artifacts in the checkout or
# in a long-lived shared temporary directory.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp_root=${TMPDIR:-/tmp}
target_dir=$(mktemp -d "${tmp_root%/}/claudex-agent-adapter-target.XXXXXX")

cleanup() {
    exit_code=$?
    # The target is created exclusively by this invocation. It is safe to
    # remove even when Cargo fails, and avoids retaining partial artifacts.
    rm -rf -- "$target_dir"
    # Remove the legacy test-fixture root left by versions before the
    # temporary-fixture migration. Do not touch any other checkout target.
    if [ -d "$repo_root/target/t" ]; then
        rm -rf -- "$repo_root/target/t"
        rmdir "$repo_root/target" 2>/dev/null || true
    fi
    exit "$exit_code"
}
trap cleanup EXIT HUP INT TERM

export CARGO_TARGET_DIR="$target_dir"
cargo "$@"
