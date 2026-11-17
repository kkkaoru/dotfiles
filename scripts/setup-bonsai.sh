#!/usr/bin/env bash
# Install everything the Bonsai 2 27B MLX checkpoint needs on the Mac that will serve it:
# the pinned mlx-vlm/mlx-lm environment in BONSAI_VENV and the weights in BONSAI_MODEL_DIR.
# Idempotent: an existing venv is reused and `hf download` skips complete files.
#   ./scripts/setup-bonsai.sh [--link <dir>]
#     --link <dir>  move an already-downloaded checkpoint there and leave a symlink behind,
#                   so the directory a project expects and pi's copy are the same bytes
#   BONSAI_MODEL_DIR  checkpoint location (default: ~/Models/Ternary-Bonsai-2-27B-mlx-2bit)
#   BONSAI_VENV       python environment (default: ~/.local/share/bonsai-2-27b/venv)
#   BONSAI_REPO       Hugging Face repo (default: prism-ml/Ternary-Bonsai-2-27B-mlx-2bit)
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

models="${BONSAI_MODEL_DIR:-$HOME/Models/Ternary-Bonsai-2-27B-mlx-2bit}"
venv="${BONSAI_VENV:-$HOME/.local/share/bonsai-2-27b/venv}"
repo="${BONSAI_REPO:-prism-ml/Ternary-Bonsai-2-27B-mlx-2bit}"
link=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --link)
      link="${2:-}"
      [ -n "$link" ] || {
        echo "--link needs a directory" >&2
        exit 1
      }
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# The demo pins mlx-vlm 0.7.8 with mlx-lm 0.32.0; other versions are untested with these packs.
if [ ! -x "$venv/bin/python" ]; then
  python3 -m venv "$venv"
fi
"$venv/bin/python" -m pip install -q --upgrade pip
"$venv/bin/python" -m pip install -q "mlx-vlm==0.7.8" "mlx-lm==0.32.0"
"$venv/bin/python" -c "import mlx_vlm, mlx_lm; print('mlx-vlm', mlx_vlm.__version__, '· mlx-lm', mlx_lm.__version__)"

# Reuse an existing download instead of fetching 8.6 GB again.
if [ -n "$link" ] && [ -d "$link" ] && [ ! -L "$link" ] && [ -f "$link/config.json" ]; then
  mkdir -p "$(dirname "$models")"
  if [ -d "$models" ]; then
    echo "$models already holds a checkpoint; leaving $link untouched" >&2
  else
    mv "$link" "$models"
    ln -s "$models" "$link"
    echo "moved $link -> $models (symlinked back)"
  fi
fi

mkdir -p "$models"
hf download "$repo" --local-dir "$models"
[ -f "$models/config.json" ] || {
  echo "bonsai: $models has no config.json after download" >&2
  exit 1
}
echo "bonsai ready: $models"
echo "start it with: $(dirname "$0")/bonsai-serve.sh"
