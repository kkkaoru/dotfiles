#!/usr/bin/env python3
"""Cut klange/nyancat's frames into the Herdr Agents mod's SVGs.

Usage: scripts/build-nyancat-frames.py <nyancat>/src/animation.c
Writes .config/herdr-app/mods/agents-emoji/nyan/cat-<variant>-<frame>.svg, the cat with one rainbow
period, and tail-<frame>.svg, one more period (16 columns) that the mod repeats to fill a row.
Variants recolor the fur and the poptart; the rainbow has no cat in it, so tails are shared.
"""
import os
import re
import sys

# xterm-256 colors of nyancat.c's 256-color table.
COLORS = {',': '#00005f', '.': '#ffffff', "'": '#000000', '@': '#ffffd7', '$': '#d787af', '-': '#d70087',
          '>': '#ff0000', '&': '#ffaf00', '+': '#ffff00', '#': '#87ff00', '=': '#0087ff', ';': '#0000af',
          '*': '#585858', '%': '#d787af'}
# Fur ('*'), frosting ('$') and sprinkles ('-'); the first is the original.
VARIANTS = [
    ('#585858', '#d787af', '#d70087'),  # gray, strawberry
    ('#d7875f', '#875f5f', '#ffd7af'),  # ginger, chocolate
    ('#e4e4e4', '#87af5f', '#ffffff'),  # white, matcha
    ('#d7d7af', '#ffff87', '#ff8700'),  # cream, lemon
    ('#5f87af', '#87ffd7', '#ff5f87'),  # blue, mint
    ('#875f00', '#ffffff', '#00afff'),  # brown, vanilla
]
# The rainbow repeats every 16 columns from column 2; the cat starts at column 18 in every frame.
TAIL = (2, 18)

frames = [re.findall(r'"([^"]*)"', body) for body in re.findall(r'frame\d+\[\] = \{(.*?)\};', open(sys.argv[1]).read(), re.S)]
assert len(frames) == 12, len(frames)
drawn = [(y, x, c) for f in frames for y, r in enumerate(f) for x, c in enumerate(r) if c not in ',.']
assert min(x for _, x, c in drawn if c in "@$-*%") >= TAIL[1], "the cat reaches into the tail"
top, bottom = min(y for y, _, _ in drawn) - 1, max(y for y, _, _ in drawn) + 1
right = max(x for _, x, _ in drawn) + 2


def svg(frame, x0, x1, colors=COLORS):
    w, h = x1 - x0, bottom - top + 1
    rects = []
    for y in range(top, bottom + 1):
        row, x = frame[y][x0:x1], 0
        while x < len(row):
            n = len(row[x:]) - len(row[x:].lstrip(row[x]))
            if row[x] != ',':
                rects.append(f'<rect x="{x}" y="{y - top}" width="{n}" height="1" fill="{colors[row[x]]}"/>')
            x += n
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w * 4}" height="{h * 4}" '
            f'shape-rendering="crispEdges"><rect width="{w}" height="{h}" fill="{COLORS[","]}"/>{"".join(rects)}</svg>\n')


out = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../.config/herdr-app/mods/agents-emoji/nyan')
os.makedirs(out, exist_ok=True)
for i, f in enumerate(frames):
    for v, (fur, frosting, sprinkles) in enumerate(VARIANTS):
        colors = {**COLORS, '*': fur, '$': frosting, '-': sprinkles}
        open(f'{out}/cat-{v}-{i}.svg', 'w').write(svg(f, TAIL[0], right, colors))
    open(f'{out}/tail-{i}.svg', 'w').write(svg(f, *TAIL))
print(f'cat {right - TAIL[0]}x{bottom - top + 1}, tail {TAIL[1] - TAIL[0]}x{bottom - top + 1}')
