#!/bin/bash
# Set up PLaMo 3 Translate 0.8B for tools/claude-plamo-translate and tools/pi-plamo-translate. Safe to run again:
# finished steps are skipped.
#   macOS:              mlx-lm and the MLX 4-bit model, served by scripts/plamo-mlx-server.py
#   Linux, Windows:     llama.cpp's Vulkan build (GeForce and Radeon alike) and the official GGUF
#   (Git Bash)

set -euo pipefail

SOURCE_ID="pfnet/plamo-3-translate-0.8b"
# The all-4-bit conversion is about twice as fast as PFN's MLX-4bit release, which leaves the
# embedding and the vocabulary head at higher precision (~0.98 GB read per token, not 478 MB).
FAST_ID="kkk4oru/plamo-3-translate-0.8b-mlx-4bit-fast"
MLX_LM_VERSION="0.32.2"
DATA_DIR="${HOME}/.local/share/plamo-translate"
MODEL_DIR="${DATA_DIR}/plamo-3-translate-0.8b-mlx-4bit"
BIN_DIR="${HOME}/.local/bin"
LLAMA_BUILD="b12360"
LLAMA_DIR="${DATA_DIR}/llama.cpp"
GGUF="plamo-3-translate-0.8b-Q4_K_M.gguf"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

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
    mkdir -p "$DATA_DIR"
    hf download "$FAST_ID" --local-dir "$MODEL_DIR"
  fi
  echo "ready: ${MODEL_DIR}"
  install_server
  # One warm-up run, so the first translation after an install does not pay for reading the weights,
  # the metallib and the tokenizer from disk (measured here: 9.5 s cold, 8.2 s with them cached).
  if ! "${BIN_DIR}/plamo-mlx-server" --plamo-prebuild --model "${MODEL_DIR}" --trust-remote-code \
    >/dev/null 2>&1; then
    echo "note: the warm-up run failed; the first translation will just take longer"
  fi
  echo "the mod starts ${BIN_DIR}/plamo-mlx-server on demand (port 18555 by default)"
  echo "benchmark it with scripts/bench-plamo-translate.py --port 18555"
}

# mlx_lm.server with a listen backlog that holds the mods' burst of eight requests: the stock one
# resets some of them, and a reset leaves that line untranslated.
install_server() {
  install -m 755 "${SCRIPT_DIR}/plamo-mlx-server.py" "${DATA_DIR}/plamo-mlx-server.py"
  cat > "${BIN_DIR}/plamo-mlx-server" <<EOF
#!/bin/sh
exec "\${HOME}/.local/share/uv/tools/mlx-lm/bin/python" "${DATA_DIR}/plamo-mlx-server.py" "\$@"
EOF
  chmod +x "${BIN_DIR}/plamo-mlx-server"
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
