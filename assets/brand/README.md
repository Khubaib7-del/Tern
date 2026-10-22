# Tern brand assets

Reserved brand property. See the brand clause in [LICENSE](../../LICENSE). Do not modify or reuse these files without the owner's approval.

| File | What it is | Use |
| --- | --- | --- |
| `tern-mark.png` | The diving-tern mark, white on black, high resolution (1400 × 1112) | Source raster until the SVG trace lands |
| `tern-lockup-dark.png` | Mark + `TERN` caps + italic serif *Research*, white on black | Dark surfaces, slides |
| `tern-lockup-light.png` | Same lockup, black on white | Light surfaces, documents |
| `tern-lockup-on-dark.png` | Dark lockup on a transparent background, trimmed | README header in dark mode |
| `tern-lockup-on-light.png` | Light lockup on a transparent background, trimmed | README header in light mode |
| `tern-app-icon.png` | App icon, 512 × 512 | Desktop app icon (planned) |
| `tern-hero-dark.gif` | Animated dither scene, 1200 × 400, 320 frames, 16 s loop. A procedural 3D tern with jointed wings flies in, loops behind and in front of the wordmark, banks into its turns, then its last wingbeat eases to a stop, it glides into the logo position, and its silhouette morphs into the original mark. The sky stays live and a shimmer sweeps across the mark until it fades and the flight repeats | README header in dark mode |
| `tern-hero-light.gif` | Same hero, ink on paper | README header in light mode |
| `tern-hero-still-dark.png` / `-light.png` | One still frame of the hero | Social preview, reduced-motion fallback |
| `tern-crawl.svg` | **Tern Crawl**, Tern's own crawler and reader: a snail in a cyan-to-blue gradient (`#5CF2FF` to `#1D3BFF`) with film grain, no background | "Read N pages" rows, crawler docs, service listings |
| `tern-crawl-mono.svg` | The same mark in one colour | Print, tinted UI (the app's `tern-crawl` glyph) |
| `tern-crawl-tile.svg` | The colour snail on a black rounded square | Standalone app or plugin icon |
| `src/dither_hero.py` | Generator for the hero files (software 3D render of the bird and wordmark plane, then 8 × 8 Bayer ordered dither; needs numpy, pillow, scipy, numba) | Re-render after any mark or wordmark change |

## Rules

| Rule | Detail |
| --- | --- |
| Colour | Black and white only. The mark is never tinted, outlined, or put on a gradient. |
| Shape | Organic ink-cut silhouette: forked tail, swept wing, long leaf-shaped eye cut, single-point beak. Do not redraw. |
| Clear space | At least the height of the head on every side. |
| Motion | The mark animates only on a real event (launch, a run starting, a run finishing), with one curve: `cubic-bezier(.32,.72,0,1)`, 700 ms. |
| Mascot | Scout, the hooded scholar with a lantern and a scroll, is drawn in line and dither in the text colour. No coloured trim. |

## Still to do

- SVG trace of the mark and wordmark
- Simplified mark for favicon and 16 px sizes
- Scout rig (parts and states) for the desktop app
