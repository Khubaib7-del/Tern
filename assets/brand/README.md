# Tern brand assets

Reserved brand property. See the brand clause in [LICENSE](../../LICENSE). Do not modify or reuse these files without the owner's approval.

| File | What it is | Use |
| --- | --- | --- |
| `tern-mark.png` | The diving-tern mark, white on black, high resolution (1400 × 1112) | Source raster until the SVG trace lands |
| `tern-lockup-dark.png` | Mark + `TERN` caps + italic serif *Research*, white on black | Dark surfaces, slides |
| `tern-lockup-light.png` | Same lockup, black on white | Light surfaces, documents |
| `tern-lockup-on-dark.png` | Dark lockup on a transparent background, trimmed | Former README header (dark); the README now uses `docs/assets/banner.webp` |
| `tern-lockup-on-light.png` | Light lockup on a transparent background, trimmed | Former README header (light); the README now uses `docs/assets/banner-light.webp` |
| `tern-app-icon.png` | App icon, 512 × 512 | Desktop app icon (planned) |
| `tern.ico` | Windows icon: the mark across about 94% of a rounded black tile's width (22% corner radius) at 16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 px, its strokes thickened more the smaller the size (#71: the taskbar mark looked small and thin). From 32 px up the tile has a faint 1 px light rim so it separates from dark taskbars; 16 and 20 px are drawn by hand so the forked tail and eye cut survive. Built by `src/make_ico.py` (which keeps the hand-drawn sizes); `src/tern-ico-sizes.png` shows every size. The executable's icon resource and the window's taskbar icon are the same file (GPUI loads resource 1) | Embedded into `tern-desktop.exe`, `tern.exe` and `tern-browser.exe` by each crate's `build.rs` |
| `tern-hero-dark.gif` | Animated dither scene, 1200 × 400, 320 frames, 16 s loop. A procedural 3D tern with jointed wings flies in, loops behind and in front of the wordmark, banks into its turns, then its last wingbeat eases to a stop, it glides into the logo position, and its silhouette morphs into the original mark. The sky stays live and a shimmer sweeps across the mark until it fades and the flight repeats | Former README header (dark); the README now uses `docs/assets/banner.webp` |
| `tern-hero-light.gif` | Same hero, ink on paper | Former README header (light); the README now uses `docs/assets/banner-light.webp` |
| `tern-hero-still-dark.png` / `-light.png` | One still frame of the hero | Social preview, reduced-motion fallback |
| `tern-crawl.svg` | **Tern Crawl**, Tern's own crawler and reader: a snail in a cyan-to-blue gradient (`#5CF2FF` to `#1D3BFF`) with film grain, no background | "Read N pages" rows, crawler docs, service listings |
| `tern-crawl-mono.svg` | The same mark in one colour | Print, tinted UI (the app's `tern-crawl` glyph) |
| `tern-crawl-tile.svg` | The colour snail on a black rounded square | Standalone app or plugin icon |
| `src/make_ico.py` | Generator for `tern.ico` (needs pillow) | Re-run after any mark change |
| `src/dither_hero.py` | Generator for the hero files (software 3D render of the bird and wordmark plane, then 8 × 8 Bayer ordered dither; needs numpy, pillow, scipy, numba) | Re-render after any mark or wordmark change |

## Rules

| Rule | Detail |
| --- | --- |
| Colour | Black and white only. The mark is never tinted, outlined, or put on a gradient. |
| Shape | Organic ink-cut silhouette: forked tail, swept wing, long leaf-shaped eye cut, single-point beak. Do not redraw. |
| Clear space | At least the height of the head on every side. |
| Motion | The mark animates only on a real event (launch, a run starting, a run finishing, the empty state appearing, a harness switch, or a hover on the empty-state mark), with one curve: `cubic-bezier(.32,.72,0,1)`, 700 ms. In the empty state the wing-beat loop plays two to three times when it appears (and again on a harness switch or hover), then rests on the still frame; with reduced motion it is the still frame only. |
| Mascot | Scout, the hooded scholar with a lantern and a scroll, is a coloured 3D clay sprite (charcoal cloak, lime trim, warm lantern), at the user's request; it replaces the earlier line-and-dither rule. In Study the trim turns to the warm paper tone, never pink (pink is not a brand colour). The mark itself stays black and white. Sprites live in `crates/tern-desktop/assets/scout/`. |

## Still to do

- SVG trace of the mark and wordmark
- Simplified mark for favicon (the 16 px icon size is hand drawn)
