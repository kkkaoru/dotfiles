#!/usr/bin/env python3
"""Cut klange/nyancat's frames into the Herdr Agents mod's SVGs.

Usage: scripts/build-nyancat-frames.py <nyancat>/src/animation.c
Writes .config/herdr-app/mods/agents-emoji/nyan/<layer>-<frame>.svg for each frame:
- cat: the cat with one rainbow period, without its rainbow and the cells the mod colors;
- fur, frosting, sprinkles: one-color masks of those cells, which the mod tints with `svg()`;
- rainbow-<palette>: the rainbow cells of `cat` in one of RAINBOWS;
- trail-<palette>: TAIL_PERIODS more rainbow periods that the mod repeats to fill a row; wide, so a
  row needs few images (NYAN_TAIL in the mod is their width).
The mod's NYAN_RAINBOWS must match len(RAINBOWS).
"""
import os
import re
import sys

# xterm-256 colors of nyancat.c's 256-color table.
COLORS = {',': '#00005f', '.': '#ffffff', "'": '#000000', '@': '#ffffd7', '$': '#d787af', '-': '#d70087',
          '>': '#ff0000', '&': '#ffaf00', '+': '#ffff00', '#': '#87ff00', '=': '#0087ff', ';': '#0000af',
          '*': '#585858', '%': '#d787af'}
# Rainbow bands top to bottom; the first is the original.
BANDS = '>&+#=;'
RAINBOWS = [
    ('#ff0000', '#ffaf00', '#ffff00', '#87ff00', '#0087ff', '#0000af'),  # original
    ('#ffafaf', '#ffd7af', '#ffffaf', '#d7ffaf', '#afd7ff', '#d7afff'),  # pastel
    ('#ff5555', '#ffb86c', '#f1fa8c', '#50fa7b', '#8be9fd', '#bd93f9'),  # Dracula
    ('#ff005f', '#ff5f00', '#ff8700', '#ffaf00', '#d7005f', '#870087'),  # sunset
    ('#00ffff', '#00d7ff', '#00afff', '#0087ff', '#005fd7', '#0000af'),  # ocean
    ('#ffffff', '#d0d0d0', '#a8a8a8', '#808080', '#585858', '#303030'),  # mono
    ('#ffff5f', '#ffd700', '#ffaf00', '#ff8700', '#ff5f00', '#d70000'),  # fire
    ('#d7ff87', '#afff5f', '#87d75f', '#5faf5f', '#3a875f', '#005f00'),  # forest
    ('#ffd7ff', '#ffafd7', '#ff87d7', '#ff5faf', '#d75fd7', '#af5fff'),  # candy
    ('#5fffaf', '#00ffd7', '#00d7ff', '#5f87ff', '#875fff', '#d75fff'),  # aurora
    ('#ff00ff', '#ff0087', '#00ffff', '#00ff87', '#ffff00', '#5f00ff'),  # neon
    ('#ffffd7', '#ffd787', '#d7af5f', '#af875f', '#875f5f', '#5f3a3a'),  # coffee
    ('#ffffff', '#ffd7ff', '#ffafd7', '#ff87af', '#d75f87', '#af5f87'),  # sakura
    ('#ffffff', '#d7ffff', '#afffff', '#87d7ff', '#5fafff', '#5f87d7'),  # ice
    ('#ffd700', '#ff8700', '#ff0000', '#af0000', '#5f0000', '#262626'),  # lava
    ('#ffd75f', '#ff875f', '#d75f00', '#af5f00', '#875f00', '#5f3a00'),  # autumn
    ('#ffffff', '#d7afff', '#af5fff', '#5f00ff', '#3a00af', '#00005f'),  # galaxy
    ('#00ff00', '#00d700', '#00af00', '#008700', '#005f00', '#003a00'),  # matrix
]
# The cells the mod tints: fur, frosting, sprinkles.
MASKS = {'fur': '*', 'frosting': '$', 'sprinkles': '-'}
# The rainbow repeats every 16 columns from column 2; the cat starts at column 18 in every frame.
TAIL = (2, 18)
TAIL_PERIODS = 8

frames = [re.findall(r'"([^"]*)"', body) for body in re.findall(r'frame\d+\[\] = \{(.*?)\};', open(sys.argv[1]).read(), re.S)]
assert len(frames) == 12, len(frames)
drawn = [(y, x, c) for f in frames for y, r in enumerate(f) for x, c in enumerate(r) if c not in ',.']
assert min(x for _, x, c in drawn if c in "@$-*%") >= TAIL[1], "the cat reaches into the tail"
top, bottom = min(y for y, _, _ in drawn) - 1, max(y for y, _, _ in drawn) + 1
right = max(x for _, x, _ in drawn) + 2


def svg(frame, x0, x1, colors=COLORS, sky=True, repeat=1):
    w, h = (x1 - x0) * repeat, bottom - top + 1
    rects = []
    for y in range(top, bottom + 1):
        row, x = frame[y][x0:x1] * repeat, 0
        while x < len(row):
            n = len(row[x:]) - len(row[x:].lstrip(row[x]))
            if row[x] != ',' and row[x] in colors:
                rects.append(f'<rect x="{x}" y="{y - top}" width="{n}" height="1" fill="{colors[row[x]]}"/>')
            x += n
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w * 4}" height="{h * 4}" '
            f'shape-rendering="crispEdges">{f'<rect width="{w}" height="{h}" fill="{COLORS[","]}"/>' if sky else ""}{"".join(rects)}</svg>\n')


out = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../.config/herdr-app/mods/agents-emoji/nyan')
os.makedirs(out, exist_ok=True)
for i, f in enumerate(frames):
    open(f'{out}/cat-{i}.svg', 'w').write(svg(f, TAIL[0], right, {k: c for k, c in COLORS.items() if k not in BANDS + ''.join(MASKS.values())}))
    for r, bands in enumerate(RAINBOWS):
        open(f'{out}/rainbow-{r}-{i}.svg', 'w').write(svg(f, TAIL[0], right, dict(zip(BANDS, bands)), sky=False))
        open(f'{out}/trail-{r}-{i}.svg', 'w').write(svg(f, *TAIL, {**COLORS, **dict(zip(BANDS, bands))}, repeat=TAIL_PERIODS))
    for name, cell in MASKS.items():
        open(f'{out}/{name}-{i}.svg', 'w').write(svg(f, TAIL[0], right, {cell: '#000000'}, sky=False))
print(f'cat {right - TAIL[0]}x{bottom - top + 1}, trail {(TAIL[1] - TAIL[0]) * TAIL_PERIODS}x{bottom - top + 1}')
