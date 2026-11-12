#!/bin/bash
# Build "UDEV Gothic <weight> 35NFLG": UDEV Gothic 35NFLG with JetBrains Mono <weight> for the
# Latin glyphs, and install it into ~/Library/Fonts. <weight> is the first argument: Light
# (default), ExtraLight or Thin. BIZ UDGothic ships no Light weight, so the
# Japanese glyphs stay Regular. Bold and BoldItalic keep JetBrains Mono's own weights.
#
# Needs Homebrew (installs fontforge) and uv (runs fonttools). Safe to run again.

set -euo pipefail

UDEV_VERSION="v2.2.0"
JETBRAINS_VERSION="2.304"
WEIGHT="${1:-Light}"
case "$WEIGHT" in
  Light | ExtraLight | Thin) ;;
  *) echo "usage: $0 [Light|ExtraLight|Thin]" >&2; exit 1 ;;
esac
FAMILY="UDEV Gothic ${WEIGHT}"
VARIANT="35NFLG"
FONTS_DIR="${HOME}/Library/Fonts"
WORK_DIR="${TMPDIR:-/tmp}/udev-gothic-${WEIGHT}"

if ! command -v fontforge >/dev/null 2>&1; then
  brew install fontforge
fi
if ! command -v uv >/dev/null 2>&1; then
  echo "missing: uv (brew install uv)" >&2
  exit 1
fi

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
git clone --quiet --depth 1 --branch "$UDEV_VERSION" https://github.com/yuru7/udev-gothic.git "$WORK_DIR/udev"
curl -fsSL -o "$WORK_DIR/jetbrains.zip" \
  "https://github.com/JetBrains/JetBrainsMono/releases/download/v${JETBRAINS_VERSION}/JetBrainsMono-${JETBRAINS_VERSION}.zip"
unzip -q -o "$WORK_DIR/jetbrains.zip" 'fonts/ttf/*' -d "$WORK_DIR/jetbrains"

cd "$WORK_DIR/udev"
# The build reads "<name>-Regular/Italic.ttf"; hand it the chosen cuts under those names.
for prefix in JetBrainsMono JetBrainsMonoNL; do
  cp "$WORK_DIR/jetbrains/fonts/ttf/${prefix}-${WEIGHT}.ttf" "source/${prefix}-Regular.ttf"
  cp "$WORK_DIR/jetbrains/fonts/ttf/${prefix}-${WEIGHT}Italic.ttf" "source/${prefix}-Italic.ttf"
done
sed -i '' "s/^FONT_NAME = .*/FONT_NAME = ${FAMILY}/" build.ini
# Recent fontforge invalidates the cached Nerd Fonts source once the first style's font closes,
# so the second style fails ("Glyph object is not valid"). Reopening the same path returns that
# same font, so every style opens its own copy. Closing it would invalidate the merged glyphs.
python3 - <<'PY'
from pathlib import Path

script = Path("fontforge_script.py")
source = script.read_text(encoding="utf-8")
merge = "    jp_font.mergeFonts(nerd_font)\n"
if merge not in source:
    raise SystemExit("fontforge_script.py changed upstream: Nerd Fonts merge not found")
opener = 'fontforge.open(f"{SOURCE_FONTS_DIR}/SymbolsNerdFont-Regular.ttf")'
if opener not in source:
    raise SystemExit("fontforge_script.py changed upstream: Nerd Fonts open not found")
copy = 'fontforge.open(shutil.copy(f"{SOURCE_FONTS_DIR}/SymbolsNerdFont-Regular.ttf", tempfile.mkstemp(suffix=".ttf")[1]))'
source = "import shutil\nimport tempfile\n" + source.replace(merge, merge + "    nerd_font = None\n").replace(opener, copy)
script.write_text(source, encoding="utf-8")
PY

fontforge -lang=py -script fontforge_script.py --35 --nerd-font --liga
uv run --no-project --with fonttools==4.40.0 --with ttfautohint-py==0.5.1 --with "setuptools<81" python fonttools_script.py "${VARIANT}-"

mkdir -p "$FONTS_DIR"
cp build/"${FAMILY// /}${VARIANT}"-*.ttf "$FONTS_DIR/"
ls -1 "$FONTS_DIR"/"${FAMILY// /}${VARIANT}"-*.ttf
echo "installed: ${FAMILY} ${VARIANT}"
