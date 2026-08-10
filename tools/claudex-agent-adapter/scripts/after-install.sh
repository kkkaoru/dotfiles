#!/bin/sh
# Relink ~/.local/bin and apply the new build without ending Claude Code sessions.
# Handover-capable daemons warm-start then cut :port over; idle TUI stays connected.
# Legacy busy daemons get a current-build fallback + live.<port>.json + idle waiter
# (install invalidates any waiter on the old inode).
#
# Canonical install path is always "$CARGO_HOME/bin/claudex-agent-adapter".
# ~/.local/bin/claudex-agent-adapter must be a symlink to that path. cargo install
# may land in either CARGO_HOME (preferred) or --root ~/.local (real binary that
# we promote into CARGO_HOME before relinking).
set -eu

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin/claudex-agent-adapter"
local_bin="$HOME/.local/bin/claudex-agent-adapter"
hot_swap="$HOME/.local/bin/claudex-hot-swap"
cache_dir="${CLAUDEX_CACHE_DIR:-$HOME/.cache/claudex}"
listen="${CLAUDEX_ADAPTER_LISTEN:-127.0.0.1:8318}"

mkdir -p "$(dirname -- "$cargo_bin")" "$HOME/.local/bin"

# Prefer a freshly installed real binary under --root ~/.local over a stale
# CARGO_HOME copy. Symlinks already pointing at cargo_bin are left alone.
if [ -x "$local_bin" ] && [ ! -L "$local_bin" ]; then
  cp -f "$local_bin" "$cargo_bin"
  chmod +x "$cargo_bin"
fi

if [ ! -x "$cargo_bin" ]; then
  echo "claudex after-install: installed adapter is not executable: $cargo_bin" >&2
  exit 0
fi

ln -snf "$cargo_bin" "$local_bin"
build_id=$("$cargo_bin" build-id)
echo "claudex after-install: linked $local_bin -> $cargo_bin ($build_id)" >&2

# Drop notify dedupe so the post-install banner is not eaten by a recent
# interactive/test Complete still inside the 5-minute quiet window.
rm -f "$cache_dir"/hot-swap-notify.json \
  "$cache_dir"/hot-swap-notify.lock \
  "$cache_dir"/hot-swap-notify.*.json \
  "$cache_dir"/hot-swap-notify.*.lock 2>/dev/null || true

# Long-lived mcp-claudex-launch parents keep in-memory notify/dedupe code from
# before this install. Restart them so the next ensure re-execs the cargo-bin
# binary (Claude Code sessions on :8318 are not touched).
if command -v pkill >/dev/null 2>&1; then
  pkill -f '/claudex-agent-adapter mcp-claudex-launch' 2>/dev/null || true
fi

if [ ! -x "$hot_swap" ]; then
  echo "claudex after-install: claudex-hot-swap is missing; skip waiter arm" >&2
  exit 0
fi

# Keep the swap itself silent. Waiters inherit the parent env, so enabling
# banners during hot-swap produced promote + idle-replace double alerts.
# Emit exactly one banner after the swap attempt finishes.
if CLAUDEX_MACOS_NOTIFY=0 "$hot_swap"; then
  CLAUDEX_MACOS_NOTIFY=1 "$cargo_bin" __internal-notify complete \
    "$cache_dir" "$listen" "$build_id" || true
else
  echo "claudex after-install: hot-swap exited $?; idle waiter may need a later claudex / claudex-hot-swap" >&2
fi
