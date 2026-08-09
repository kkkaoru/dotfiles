#!/bin/sh
# Relink ~/.local/bin and arm the idle hot-swap waiter after cargo install.
# Replacing ~/.cargo/bin/claudex-agent-adapter invalidates any waiter running
# that inode, so install itself must re-arm for the new build_id.
set -eu

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin/claudex-agent-adapter"
local_bin="$HOME/.local/bin/claudex-agent-adapter"
hot_swap="$HOME/.local/bin/claudex-hot-swap"

if [ ! -x "$cargo_bin" ]; then
  echo "claudex after-install: installed adapter is not executable: $cargo_bin" >&2
  exit 0
fi

mkdir -p "$HOME/.local/bin"
ln -snf "$cargo_bin" "$local_bin"
echo "claudex after-install: linked $local_bin -> $cargo_bin ($("$cargo_bin" build-id))" >&2

if [ ! -x "$hot_swap" ]; then
  echo "claudex after-install: claudex-hot-swap is missing; skip waiter arm" >&2
  exit 0
fi

if ! "$hot_swap"; then
  echo "claudex after-install: hot-swap exited $?; idle waiter may need a later claudex / claudex-hot-swap" >&2
fi
