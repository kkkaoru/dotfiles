#!/bin/bash
# Set up PLaMo 3 Translate 0.8B for the tools/claude-plamo-translate mod. Safe to run again:
# finished steps are skipped.
#   macOS:              mlx-lm, the model converted to MLX 4-bit
#   Linux, Windows:     llama.cpp's Vulkan build (GeForce and Radeon alike) and the official GGUF
#   (Git Bash)

set -euo pipefail

SOURCE_ID="pfnet/plamo-3-translate-0.8b"
MLX_LM_VERSION="0.32.2"
DATA_DIR="${HOME}/.local/share/plamo-translate"
MODEL_DIR="${DATA_DIR}/plamo-3-translate-0.8b-mlx-4bit"
BIN_DIR="${HOME}/.local/bin"
LLAMA_BUILD="b12360"
LLAMA_DIR="${DATA_DIR}/llama.cpp"
GGUF="plamo-3-translate-0.8b-Q4_K_M.gguf"

require() {
  for tool in "$@"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "missing: ${tool} (install it first)" >&2
      exit 1
    fi
  done
}

setup_mlx() {
  require uv hf
  # PLaMo's remote code needs numba for its tokenizer and imports torch while loading.
  uv tool install "mlx-lm==${MLX_LM_VERSION}" --with numba --with torch

  if [ -f "${MODEL_DIR}/config.json" ]; then
    echo "present: ${MODEL_DIR}"
  else
    hf download "$SOURCE_ID"
    mkdir -p "$DATA_DIR"
    "${BIN_DIR}/mlx_lm.convert" --hf-path "$SOURCE_ID" --mlx-path "$MODEL_DIR" -q --q-bits 4 --trust-remote-code
  fi
  echo "ready: ${MODEL_DIR}"
  echo "the mod starts ${BIN_DIR}/mlx_lm.server on demand (port 18555 by default)"
}

# $1: the release asset's platform part, e.g. ubuntu-vulkan-x64.tar.gz or win-vulkan-x64.zip
install_llama() {
  local asset="llama-${LLAMA_BUILD}-bin-$1" tmp
  if [ -f "${LLAMA_DIR}/.build-${LLAMA_BUILD}" ]; then
    echo "present: llama.cpp ${LLAMA_BUILD}"
    return
  fi
  tmp="$(mktemp -d)"
  curl -fL -o "${tmp}/${asset}" "https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_BUILD}/${asset}"
  rm -rf "$LLAMA_DIR"
  mkdir -p "$LLAMA_DIR"
  # The Linux archive holds one llama-<build>/ folder; the Windows zip is flat. The server
  # stays beside its shared libraries.
  case "$asset" in
    *.tar.gz) tar -xzf "${tmp}/${asset}" --strip-components=1 -C "$LLAMA_DIR" ;;
    *.zip) unzip -q "${tmp}/${asset}" -d "$LLAMA_DIR" ;;
  esac
  rm -rf "$tmp"
  touch "${LLAMA_DIR}/.build-${LLAMA_BUILD}"
}

setup_llama() {
  require curl "$2"
  install_llama "$1"
  if [ -f "${DATA_DIR}/${GGUF}" ]; then
    echo "present: ${DATA_DIR}/${GGUF}"
  else
    curl -fL -o "${DATA_DIR}/${GGUF}.part" "https://huggingface.co/${SOURCE_ID}-GGUF/resolve/main/${GGUF}"
    mv "${DATA_DIR}/${GGUF}.part" "${DATA_DIR}/${GGUF}"
  fi
  echo "ready: ${DATA_DIR}/${GGUF}"
  echo "the mod starts ${LLAMA_DIR}/llama-server on demand (port 18555 by default)"
  echo "it needs a Vulkan driver: the GPU vendor's driver on Windows, Mesa or the NVIDIA driver on Linux"
}

case "$(uname -s)" in
  Darwin) setup_mlx ;;
  Linux) setup_llama ubuntu-vulkan-x64.tar.gz tar ;;
  MINGW* | MSYS* | CYGWIN*) setup_llama win-vulkan-x64.zip unzip ;;
  *)
    echo "unsupported system: $(uname -s)" >&2
    exit 1
    ;;
esac
