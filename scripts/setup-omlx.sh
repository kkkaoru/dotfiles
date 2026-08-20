#!/bin/bash
# Download mlx-community Qwen3.8 4-bit + DFlash2 and wire them into oMLX.

set -euo pipefail

DOTPATH=$(cd "$(dirname "$0")/.." || exit 1; pwd)
OMLX_HOME="${HOME}/.omlx"
OMLX_CLI="/Applications/oMLX.app/Contents/MacOS/omlx-cli"
TARGET_ID="mlx-community/Qwen3.8-27B-4bit"
DRAFT_ID="incoai/Qwen3.8-27B-DFlash2"
TARGET_DIR="${OMLX_HOME}/models/${TARGET_ID}"
DRAFT_DIR="${OMLX_HOME}/models/${DRAFT_ID}"

if [ ! -x "$OMLX_CLI" ]; then
  echo "missing: ${OMLX_CLI} (install oMLX.app with DFlash2, e.g. 0.6.2-dflash2)" >&2
  exit 1
fi

if [ ! -L "$OMLX_HOME" ]; then
  echo "run ./create-symlinks.sh first so ${OMLX_HOME} is a symlink into ${DOTPATH}/.omlx" >&2
  exit 1
fi

mkdir -p "${OMLX_HOME}/models" "${OMLX_HOME}/cache" "${OMLX_HOME}/logs"

if [ ! -f "${OMLX_HOME}/settings.json" ]; then
  python3 - "$DOTPATH/.omlx/settings.json.example" "${OMLX_HOME}/settings.json" "$HOME" <<'PY'
import json
import secrets
import sys
from pathlib import Path

src, dest, home = sys.argv[1], sys.argv[2], sys.argv[3]
data = json.loads(Path(src).read_text(encoding="utf-8"))
data["auth"]["secret_key"] = secrets.token_hex(32)
models_dir = str(Path(home) / ".omlx" / "models")
data["model"]["model_dir"] = models_dir
data["model"]["model_dirs"] = [models_dir]
Path(dest).write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
Path(dest).chmod(0o600)
PY
  echo "wrote ${OMLX_HOME}/settings.json"
fi

if [ ! -f "${OMLX_HOME}/model_settings.json" ]; then
  cp "${DOTPATH}/.omlx/model_settings.json" "${OMLX_HOME}/model_settings.json"
fi

download_repo() {
  local repo="$1"
  local dest="$2"
  if [ -f "${dest}/config.json" ]; then
    echo "present: ${dest}"
    return
  fi
  mkdir -p "$dest"
  hf download "$repo" --local-dir "$dest"
}

download_repo "$TARGET_ID" "$TARGET_DIR"
download_repo "$DRAFT_ID" "$DRAFT_DIR"

if [ ! -f "${TARGET_DIR}/config.json" ] || [ ! -f "${DRAFT_DIR}/config.json" ]; then
  echo "download failed: expected config.json under ${TARGET_DIR} and ${DRAFT_DIR}" >&2
  exit 1
fi

"$OMLX_CLI" restart --timeout 120 || "$OMLX_CLI" start --timeout 120

echo "oMLX target=${TARGET_ID} draft=${DRAFT_ID}"
echo "pi: pi --provider omlx --model Qwen3.8-27B-4bit --thinking low"
