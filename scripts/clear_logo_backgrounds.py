#!/usr/bin/env python3
"""Clears the opaque white square behind some file-type logos.

    python3 scripts/clear_logo_backgrounds.py [--src DIR] [--sheet OUT.png]

About twenty logos (Go, Perl, CMake, APL, Objective-C, OCaml, Gleam,
Erlang, Scheme, HDF5, …) came with a white square behind them, which
showed as a white box on Tern's dark theme. For each logo in LOGOS this
makes that background transparent:

- a flood fill from the image's border through near-white (and already
  transparent) pixels finds the background; only what the fill reaches is
  cleared, so white inside the logo (the Go gopher's eyes, letters on a
  coloured shape) stays;
- the pixels along the cleared edge lose their white matte (alpha from
  how white they are, colour un-blended from white), so edges stay soft
  instead of keeping a light fringe.

--src DIR   the approved 256 px PNGs (as for gen_file_icons.py); each is
            cleaned at 256 px, then downscaled to 64 px as gen_file_icons.py
            does. Without it, the committed 64 px PNGs are cleaned in place.
--sheet     also writes a contact sheet of the cleaned logos on the dark
            theme's canvas, to check that nothing was eaten.

Run scripts/gen_file_icons.py afterwards (a logo that is now mostly black
gets the light tile on a dark page), then `cargo fmt --all`. Needs Pillow.
"""

import argparse
import os
from collections import deque

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "crates/tern-desktop/assets/file-icons")
SIZE = 64

# Found by measuring how much near-white the border fill reaches, then
# checked by eye: these have a white square; pdf, clojure, racket, julia,
# less, lean and prolog are drawn that way on purpose and are left alone.
LOGOS = [
    "hdf5-netcdf",
    "bibtex-biblatex",
    "scheme",
    "cmake",
    "go",
    "erlang",
    "common-lisp",
    "gleam",
    "ris-endnote-citations",
    "camera-raw",
    "apl",
    "certificate-key",
    "restructuredtext",
    "makefile",
    "perl",
    "ocaml",
    "latex",
    "objective-c",
    "cobol",
    "d",
    "elm",
]

WHITE_MIN = 205  # every channel at least this
WHITE_SPREAD = 30  # and no more colourful than this
SPECK = 40  # pieces smaller than this (pixels at 256 px) left in the background go
EDGE_FROM = 140  # an edge pixel's whiteness counts from this level


def is_white(p):
    r, g, b, a = p
    return a >= 128 and min(r, g, b) >= WHITE_MIN and max(r, g, b) - min(r, g, b) <= WHITE_SPREAD


def is_grey_edge(p):
    r, g, b, a = p
    return min(r, g, b) >= 150 and max(r, g, b) - min(r, g, b) <= WHITE_SPREAD


def clear(im):
    im = im.convert("RGBA")
    w, h = im.size
    px = im.load()
    bg = [[False] * w for _ in range(h)]
    q = deque()
    for x in range(w):
        q.append((x, 0))
        q.append((x, h - 1))
    for y in range(h):
        q.append((0, y))
        q.append((w - 1, y))
    while q:
        x, y = q.popleft()
        if x < 0 or y < 0 or x >= w or y >= h or bg[y][x]:
            continue
        p = px[x, y]
        # Near the edge a scanned grey border line counts as background too.
        edge = min(x, y, w - 1 - x, h - 1 - y) < max(2, w // 40)
        if not (p[3] < 128 or is_white(p) or (edge and is_grey_edge(p))):
            continue
        bg[y][x] = True
        q.extend(((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)))
    # The ring along the background (two pixels at 256 px).
    ring = set()
    reach = 2 if w >= 128 else 1
    for y in range(h):
        for x in range(w):
            if bg[y][x]:
                continue
            near = any(
                0 <= x + dx < w and 0 <= y + dy < h and bg[y + dy][x + dx]
                for dx in range(-reach, reach + 1)
                for dy in range(-reach, reach + 1)
            )
            if near:
                ring.add((x, y))
    for y in range(h):
        for x in range(w):
            if bg[y][x]:
                px[x, y] = (0, 0, 0, 0)
    # Specks: tiny pieces the fill went round (compression noise in the
    # white), never part of the logo.
    speck = max(2, SPECK * w * h // (256 * 256))
    seen = [[False] * w for _ in range(h)]
    for y0 in range(h):
        for x0 in range(w):
            if seen[y0][x0] or bg[y0][x0]:
                continue
            piece, q = [], deque([(x0, y0)])
            seen[y0][x0] = True
            while q:
                x, y = q.popleft()
                piece.append((x, y))
                for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                    if 0 <= nx < w and 0 <= ny < h and not seen[ny][nx] and not bg[ny][nx]:
                        seen[ny][nx] = True
                        q.append((nx, ny))
            # A sliver cut off at the image's edge (a cropped caption) goes
            # too; the logo itself is far larger.
            at_edge = any(x in (0, w - 1) or y in (0, h - 1) for x, y in piece)
            if len(piece) < speck or (at_edge and len(piece) < w * h // 60):
                for x, y in piece:
                    px[x, y] = (0, 0, 0, 0)
                    bg[y][x] = True
                    ring.discard((x, y))
    for x, y in ring:
        r, g, b, a = px[x, y]
        white = max(0.0, min(1.0, (min(r, g, b) - EDGE_FROM) / (255 - EDGE_FROM)))
        if white <= 0:
            continue
        keep = 1.0 - white
        na = int(round(a * keep))
        if na < 8:
            px[x, y] = (0, 0, 0, 0)
            continue
        un = lambda c: max(0, min(255, int(round((c - 255 * white) / keep))))
        px[x, y] = (un(r), un(g), un(b), na)
    return im


def sheet(names, out):
    cell, cols = 96, 7
    rows = (len(names) + cols - 1) // cols
    s = Image.new("RGBA", (cols * cell, rows * cell), (19, 19, 19, 255))
    d = ImageDraw.Draw(s)
    for i, n in enumerate(names):
        im = Image.open(os.path.join(ASSETS, n + ".png")).convert("RGBA")
        x, y = (i % cols) * cell + 16, (i // cols) * cell + 8
        s.alpha_composite(im, (x, y))
        d.text(((i % cols) * cell + 4, y + 68), n[:15], fill=(190, 190, 190, 255))
    s.save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src")
    ap.add_argument("--sheet")
    args = ap.parse_args()
    for n in LOGOS:
        dst = os.path.join(ASSETS, n + ".png")
        if args.src:
            im = clear(Image.open(os.path.join(args.src, n + ".png")))
            im = im.resize((SIZE, SIZE), Image.LANCZOS)
        else:
            im = clear(Image.open(dst))
        im = im.quantize(256, method=Image.Quantize.FASTOCTREE)
        im.save(dst, optimize=True)
    if args.sheet:
        sheet(LOGOS, args.sheet)
    print(f"cleared {len(LOGOS)} logos")


if __name__ == "__main__":
    main()
