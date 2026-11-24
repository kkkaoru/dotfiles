#!/usr/bin/env bash
# Install what scripts/inkling-mini.sh serves, once per machine: a standalone llama.cpp build with
# Inkling 2 support (looped layers, per-expert NVFP4 scales, Metal kernels) and the GGUF set.
# The model is not in Git. Homebrew's llama.cpp is left alone; this build lives in its own prefix.
#   ./scripts/setup-inkling-mini.sh [--link <dir>]
#   INKLING_LLAMA_BUILD  unslothai/llama.cpp release to unpack (default: b12649-mix-c7059d6)
#   INKLING_MODEL_DIR    where the GGUFs go (default: ~/Models/Inkling-2-Mini-GGUF)
#   INKLING_QUANT        model file to download (default: Inkling-2-Mini-UD-Q4_K_XL.gguf, 17.6 GB)
set -euo pipefail

build="${INKLING_LLAMA_BUILD:-b12649-mix-c7059d6}"
prefix="$HOME/.local/opt/llama-inkling-mini"
model_dir="$HOME/Models/Inkling-2-Mini-GGUF"
quant="${INKLING_QUANT:-Inkling-2-Mini-UD-Q4_K_XL.gguf}"

# --link moves an existing download here and leaves a symlink, like setup-bonsai.sh.
if [ "${1:-}" = "--link" ]; then
  src="${2:?usage: $0 [--link <dir>]}"
  [ -d "$src" ] || { echo "no directory $src" >&2; exit 1; }
  if [ -e "$model_dir" ] && [ ! -L "$model_dir" ]; then
    echo "$model_dir already exists; move it aside first" >&2
    exit 1
  fi
  rm -rf "$model_dir"
  mv "$src" "$model_dir"
  ln -s "$model_dir" "$src"
  echo "linked $model_dir <-> $src"
fi

if [ ! -x "$prefix/llama-server" ]; then
  echo "installing llama.cpp $build into $prefix"
  mkdir -p "$prefix"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -fL -o "$tmp/llama.tar.gz" \
    "https://github.com/unslothai/llama.cpp/releases/download/$build/llama-$build-bin-macos-arm64.tar.gz"
  tar -xzf "$tmp/llama.tar.gz" -C "$prefix" --strip-components=1
fi
"$prefix/llama-server" --version 2>&1 | head -2

mkdir -p "$model_dir"
# curl rather than `hf download`: the hub's xet transfer stalled mid-file on this Mac, and a plain
# resumable GET reconnects instead of hanging.
fetch() {
  local name="$1"
  if [ -f "$model_dir/$name" ]; then
    echo "$name already downloaded"
    return 0
  fi
  curl -fL --retry 5 --retry-delay 5 -C - -o "$model_dir/$name" \
    "https://huggingface.co/unsloth/Inkling-2-Mini-GGUF/resolve/main/$name?download=true"
}
echo "downloading $quant, mmproj-F16 and the DSpark drafter into $model_dir (~18 GB)"
for name in "$quant" mmproj-F16.gguf dspark-Inkling-2-Mini-Q8_0.gguf; do
  fetch "$name"
done
ls -lh "$model_dir"

echo
echo "next: ./scripts/inkling-mini.sh start && ./scripts/inkling-mini.sh install-idle"
