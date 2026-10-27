#!/usr/bin/env python3
"""Rasterises Tern's folder logos (#73).

    python3 scripts/gen_folder_icons.py --src DIR

DIR holds folder SVGs from Material Icon Theme
(https://github.com/material-extensions/vscode-material-icon-theme,
MIT licence, icons/folder-*.svg). Each one in STEMS is drawn at 64x64 with
resvg (`pip install resvg_py`) into
crates/tern-desktop/assets/file-icons/<stem>.png. The name -> logo table
lives by hand in crates/tern-desktop/src/fileicon.rs (FOLDERS).
"""

import argparse
import io
import os

import resvg_py
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "crates/tern-desktop/assets/file-icons")
SIZE = 64
STEMS = [
    "folder-base", "folder-git", "folder-src", "folder-docs", "folder-node",
    "folder-github", "folder-images", "folder-resource", "folder-test",
    "folder-vscode", "folder-scripts", "folder-lib", "folder-dist",
    "folder-target", "folder-config", "folder-public", "folder-components",
    "folder-app",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    args = ap.parse_args()
    for s in STEMS:
        png = bytes(resvg_py.svg_to_bytes(svg_path=os.path.join(args.src, s + ".svg"),
                                          width=SIZE, height=SIZE))
        im = Image.open(io.BytesIO(png)).convert("RGBA")
        assert im.size == (SIZE, SIZE), (s, im.size)
        out = os.path.join(ASSETS, s + ".png")
        im.save(out, optimize=True)
        print(out)


if __name__ == "__main__":
    main()
