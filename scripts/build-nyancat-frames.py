#!/usr/bin/env python3
"""Cut klange/nyancat's frames into the Herdr Agents mod's SVGs.

Usage: scripts/build-nyancat-frames.py <nyancat>/src/animation.c
Writes .config/herdr-app/mods/agents-emoji/nyan/<layer>-<frame>.svg for each frame:
- cat-<palette>: the cat with one rainbow period in one of RAINBOWS, without the cells the mod colors;
- fur, frosting, sprinkles: one-color masks of those cells, which the mod tints with `svg()`;
- trail-<palette>: TAIL_PERIODS more rainbow periods that the mod repeats to fill a row; wide, so a
  row needs few images (NYAN_TAIL in the mod is their width).
The Agents mod's cats use the first NYAN_RAINBOWS (18). The tab bar clock (tabs-clock/clock.js) picks
24 of them, one per hour, from its own copies of all of them in .config/herdr-app/mods/tabs-clock/nyan:
the same layers without the night sky or its stars, so the cat runs over the bar itself.
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
    ('#d7fff5', '#87ffd7', '#5fd7af', '#00af87', '#008787', '#005f5f'),  # mint
    ('#ffff00', '#ffaf00', '#ff5f87', '#d75fd7', '#00d7ff', '#00af87'),  # tropical
    ('#ffd7af', '#ffaf87', '#d787af', '#875faf', '#5f5f87', '#3a3a5f'),  # twilight
    ('#ffafaf', '#ff5f5f', '#d70000', '#af0000', '#870000', '#5f0000'),  # ruby
    ('#ffff87', '#ffd700', '#d7af00', '#af8700', '#875f00', '#5f3a00'),  # gold
    ('#ff8700', '#ffd700', '#5fd75f', '#00afaf', '#5f87d7', '#875faf'),  # retro
    ('#ff00af', '#ff5f00', '#ffd700', '#00ff87', '#00afff', '#875fff'),  # electric
    ('#ff5fd7', '#ff5f87', '#ffaf5f', '#5fd7ff', '#5f87ff', '#af5fff'),  # synthwave
    ('#ffff00', '#afff00', '#ff8700', '#ff005f', '#d700af', '#5f00d7'),  # citrus-berry
    ('#00ffaf', '#00d7d7', '#0087ff', '#5f5fff', '#af00ff', '#ff00d7'),  # peacock
]
# The Agents mod's cats: the first 18 (its NYAN_RAINBOWS).
AGENT_RAINBOWS = 18
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


def rainbow_only(frame):
    """The frame with anything but sky and rainbow (the cat's tail reaches column 17 in some frames)
    taken from one period to its left, so a repeated period has no bits of cat."""
    period = TAIL[1] - TAIL[0]
    return [''.join(c if c in ',.' + BANDS or x < period else r[x - period] for x, c in enumerate(r)) for r in frame]


mods = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../.config/herdr-app/mods')
# (folder, with the night sky and its stars)
for out, sky, rainbows in ((f'{mods}/agents-emoji/nyan', True, RAINBOWS[:AGENT_RAINBOWS]),
                           (f'{mods}/tabs-clock/nyan', False, RAINBOWS)):
    os.makedirs(out, exist_ok=True)
    colors = COLORS if sky else {k: c for k, c in COLORS.items() if k != '.'}
    for i, f in enumerate(frames):
        base = {k: c for k, c in colors.items() if k not in MASKS.values()}
        for r, bands in enumerate(rainbows):
            open(f'{out}/cat-{r}-{i}.svg', 'w').write(svg(f, TAIL[0], right, {**base, **dict(zip(BANDS, bands))}, sky))
            open(f'{out}/trail-{r}-{i}.svg', 'w').write(svg(rainbow_only(f), *TAIL, {**colors, **dict(zip(BANDS, bands))}, sky, TAIL_PERIODS))
        for name, cell in MASKS.items():
            open(f'{out}/{name}-{i}.svg', 'w').write(svg(f, TAIL[0], right, {cell: '#000000'}, sky=False))
print(f'cat {right - TAIL[0]}x{bottom - top + 1}, trail {(TAIL[1] - TAIL[0]) * TAIL_PERIODS}x{bottom - top + 1}')
