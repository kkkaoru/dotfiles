#!/bin/sh
# Run Cargo with a per-invocation target directory and remove it on exit.
# This keeps builds/tests from accumulating Rust artifacts in the checkout or
# in a long-lived shared temporary directory.
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
tmp_root=${TMPDIR:-/tmp}

checkout_target_in_use() {
    # Direct Cargo/coverage invocations may still own the legacy checkout target.
    # Leave it untouched in that case; the next invocation will retry cleanup.
    ps -axo pid=,command= | awk -v root="$repo_root/target" -v self="$$" \
        '$1 != self && $2 !~ /(^|\/)(awk|ps|grep|rg)$/ && index($0, root) { found = 1 } END { exit !found }'
}

prune_checkout_target() {
    if [ ! -d "$repo_root/target" ] || checkout_target_in_use; then
        return 0
    fi
    # All normal wrapper builds use a private temporary target.  The checkout
    # target is therefore legacy output and can be removed as one unit,
    # including stale debug, release, coverage, and fixture artifacts.
    rm -rf -- "$repo_root/target"
}

prune_checkout_target
avail_kb=$(df -k -P "$tmp_root" | awk 'NR==2 { print $4 }')
if [ -n "$avail_kb" ] && [ "$avail_kb" -lt 2097152 ]; then
    echo "cargo-ephemeral: need at least 2GiB free on $tmp_root (have ${avail_kb}KiB)" >&2
    exit 1
fi
target_dir=$(mktemp -d "${tmp_root%/}/claudex-agent-adapter-target.XXXXXX")

# shellcheck disable=SC2329
cleanup() {
    exit_code=$?
    # The target is created exclusively by this invocation. It is safe to
    # remove even when Cargo fails, and avoids retaining partial artifacts.
    rm -rf -- "$target_dir"
    # Remove the legacy test-fixture root left by versions before the
    # temporary-fixture migration and any other stale checkout artifacts.
    prune_checkout_target
    exit "$exit_code"
}
trap cleanup EXIT HUP INT TERM

export CARGO_TARGET_DIR="$target_dir"
cat >"$target_dir/CACHEDIR.TAG" <<'EOF'
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by claudex-agent-adapter.
# For information about cache directory tags, see:
#	https://bford.info/cachedir/
EOF

cargo_subcommand() {
    for arg in "$@"; do
        case "$arg" in
            +*|--) continue ;;
            -*) continue ;;
            *)
                printf '%s\n' "$arg"
                return 0
                ;;
        esac
    done
    return 1
}

cargo "$@"
status=$?
if [ "$status" -eq 0 ] && [ "$(cargo_subcommand "$@")" = install ]; then
    # cargo install replaces the running waiter inode; re-arm for the new build.
    "$(dirname -- "$0")/after-install.sh" || true
fi
exit "$status"
