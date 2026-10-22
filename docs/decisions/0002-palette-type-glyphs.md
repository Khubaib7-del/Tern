# ADR 0002 · Palette, type, glyphs, wordmark, and Scout

| Field | Value |
| --- | --- |
| Status | Accepted, pending the owner's review in the app (Appearance screen) |
| Date | 2026-10-22 |
| Decider | Khubaib Nazeer |

## Context

`docs/02-ui-definition.md` §5 to §7 left four choices open: the palette (A, B, or C), the fonts, the custom glyph set, and how Scout is built. Rejected earlier: lime `#C6F24E`, pink `#FF7AA8`, and any purple. The owner wants a restrained interface that does not look machine-made, with the black and white mark in charge.

## Decision

### Palette: C, Mono & Lichen

| Token | Dark | Light | Use |
| --- | --- | --- | --- |
| Background | `#0C0C0C` | `#EFEFEC` | Rail |
| Sidebar | `#101010` | `#F5F5F3` | Chats, Scout's corner |
| Surface | `#131313` | `#FBFBFA` | Canvas |
| Raised | `#1C1C1C` | `#EFEFEC` | Cards, bubbles |
| Hover | `#222222` | `#E7E7E3` | Hover, selected pill |
| Border | `#272727` | `#DCDCD8` | 1 px rules |
| Text | `#EDEDED` | `#141414` | Body |
| Muted | `#A1A1A1` | `#5A5A57` | Secondary text |
| Faint | `#707070` | `#8C8C87` | Hints, timestamps (never the only carrier of meaning) |
| Accent, lichen | `#A3AE8E` | `#56603F` | Focus ring, selection, selected harness marker, context ring, Scout's lantern flame |

Text is at least 7:1 and muted and accent at least 4.5:1 on every surface, in both modes (`theme.rs` tests check this). The accent stays well under 5% of the pixels. Status tones (context filling and full, verdicts) stay desaturated and are never the only signal.

### Type

| Role | Face | Licence |
| --- | --- | --- |
| Interface | IBM Plex Sans (400, 500, 600, italic) | SIL OFL 1.1 |
| Mono | IBM Plex Mono 400 | SIL OFL 1.1 |
| Serif | Instrument Serif italic, for *Research* and *Study* | SIL OFL 1.1 |

The fonts are bundled in `crates/tern-desktop/assets/fonts` with their licences, so Tern looks the same on Windows, Linux, and macOS. The Plex Sans files are the Latin subset; other scripts fall back to the system font.

### Wordmark

The title bar shows the lockup: the tern mark, **TERN** in Plex Sans semibold, and the harness in Instrument Serif italic (*Research* or *Study*), which changes with the harness toggle.

### Glyphs

Twelve custom glyphs, 16 px on a 16 grid, 1.5 px stroke in `currentColor`, matching Lucide: `research` (square), `study` (round), five verdicts (`supported`, `partial`, `contested`, `unsupported`, `unverified`), `checkpoint`, `watcher`, `steer`, `branch-reply`, and `tern-crawl`, the logo of **Tern Crawl**, Tern's own crawler and reader (a snail, because it crawls: a spiral shell over a long foot with two eye stalks, no background). Tern Crawl is the one mark allowed colour: two vibrant gradients, an amber-to-flame shell (`#FFB000` to `#FF3D00`) on an emerald body (`#3ECF8E` to `#0F9D63`), because it stands alone as a service logo the way other crawlers' logos do. The app shows the colour logo on "Read N pages" rows and keeps the one-colour glyph for tinted contexts. Brand files: `assets/brand/tern-crawl.svg` (colour), `tern-crawl-mono.svg`, `tern-crawl-tile.svg`. Research and Study keep the square and round markers from §5.

Built-in service logos, one colour, from Simple Icons (CC0 data; each mark belongs to its owner): GitHub, arXiv, Wikipedia, DOI, Semantic Scholar, PubMed, Brave, DuckDuckGo, Google Scholar, ORCID, Zotero, ResearchGate. A source row shows the built-in logo when the site is one of these, otherwise its favicon (ADR 0005).

### Scout

Scout is a procedural rig, not a set of pictures (`crates/tern-desktop/src/scout.rs`). It is drawn as paths in the text colour: an ink hood and cloak, a face opening with two eyes, a lantern on a spring in one hand, a scroll in the other.

| Mood | From | Motion |
| --- | --- | --- |
| Needs you | Approval asked | Lantern raised and lit, faces you |
| Blocked | Provider offline, error | Sits lower, eyes half shut, no blinking |
| Searching | Search steps | Lantern lit, drifts, swings once on each new step |
| Reading | Fetch and read steps | Scroll opens, eyes run down the page |
| Verifying | Claim checks (phase 4) | Scroll open, taps on each verdict |
| Thinking | Model calls, writing | Still, eyes up |
| Ready | A run ends | One nod, then idle |
| Idle | Nothing running | Breathes and blinks every 2.5 to 6 s |

Moves only on real events: while a mood is changing it draws every frame; at rest it sleeps until its next blink. Reduced motion shows each pose without movement. Scout sits at the foot of the chats sidebar with nothing printed under it; the tooltip names the mood. The Appearance screen shows every mood for review.

## Consequences

- Colours live only in `theme.rs`, glyphs and logos only in `assets.rs`.
- The glyph set and Scout are the owner's to approve; changes are edits to those two files, not new systems.
- A heavier dither texture for Scout and the launch card is left for the final polish PR if the owner wants it.
