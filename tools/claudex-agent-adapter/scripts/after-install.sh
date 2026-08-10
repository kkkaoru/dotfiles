#!/bin/sh
# Relink ~/.local/bin and apply the new build without ending Claude Code sessions.
# Handover-capable daemons warm-start then cut :port over; idle TUI stays connected.
# Legacy busy daemons get a current-build fallback + live.<port>.json + idle waiter
# (install invalidates any waiter on the old inode).
#
# cargo install may land in either:
#   - default CARGO_HOME/bin (then we only symlink ~/.local/bin), or
#   - --root ~/.local (a real binary in ~/.local/bin that must be copied into
#     CARGO_HOME before the symlink, or after-install would discard the fresh build).
set -eu

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin/claudex-agent-adapter"
local_bin="$HOME/.local/bin/claudex-agent-adapter"
hot_swap="$HOME/.local/bin/claudex-hot-swap"

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
echo "claudex after-install: linked $local_bin -> $cargo_bin ($("$cargo_bin" build-id))" >&2

if [ ! -x "$hot_swap" ]; then
  echo "claudex after-install: claudex-hot-swap is missing; skip waiter arm" >&2
  exit 0
fi

if ! "$hot_swap"; then
  echo "claudex after-install: hot-swap exited $?; idle waiter may need a later claudex / claudex-hot-swap" >&2
fi
