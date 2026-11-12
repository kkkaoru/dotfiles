#!/bin/bash
# Install mlx-lm, download PLaMo 3 Translate 0.8B and convert it to MLX 4-bit for the
# tools/claude-plamo-translate mod. Safe to run again: finished steps are skipped.

set -euo pipefail

SOURCE_ID="pfnet/plamo-3-translate-0.8b"
MLX_LM_VERSION="0.32.2"
MODEL_DIR="${HOME}/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
BIN_DIR="${HOME}/.local/bin"

for tool in uv hf; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing: ${tool} (install it first, e.g. brew install uv huggingface-cli)" >&2
    exit 1
  fi
done

# PLaMo's remote code needs numba for its tokenizer and imports torch while loading.
uv tool install "mlx-lm==${MLX_LM_VERSION}" --with numba --with torch

if [ -f "${MODEL_DIR}/config.json" ]; then
  echo "present: ${MODEL_DIR}"
else
  hf download "$SOURCE_ID"
  mkdir -p "$(dirname "$MODEL_DIR")"
  "${BIN_DIR}/mlx_lm.convert" --hf-path "$SOURCE_ID" --mlx-path "$MODEL_DIR" -q --q-bits 4 --trust-remote-code
fi

echo "ready: ${MODEL_DIR}"
echo "the mod starts ${BIN_DIR}/mlx_lm.server on demand (port 18555 by default)"
