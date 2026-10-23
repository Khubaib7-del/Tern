# Assets

What Tern ships besides code, where it came from, and under which licence.

## Tern's own assets

The tern mark, the Tern Crawl logo, Tern's custom glyphs, and Scout are Tern's own work, under the repository's licence (see [LICENSE](../LICENSE)). Interface icons are [Lucide](https://lucide.dev) (ISC licence).

## Third-party brand logos

The desktop app shows each provider's and search service's own logo beside its name (the model picker, the Providers screen, the search chip). These files live in [`crates/tern-desktop/assets/brands/`](../crates/tern-desktop/assets/brands/) and are looked up by `brands.rs` in the desktop crate.

| | |
| --- | --- |
| Source | [Lobe Icons](https://github.com/lobehub/lobe-icons), package `@lobehub/icons-static-svg` 1.98.0 |
| Licence | MIT, © LobeHub. The licence is kept with the files in [`NOTICE.md`](../crates/tern-desktop/assets/brands/NOTICE.md) |
| Changes | Size attributes changed, the inline `style` and `<title>` removed, and Groq's one-colour mark filled with Groq orange (#F55036) |

| File | Source icon | Drawn |
| --- | --- | --- |
| anthropic.svg | claude-color | in its colours |
| brave.svg | brave-color | in its colours |
| deepseek.svg | deepseek-color | in its colours |
| google.svg | gemini-color | in its colours |
| groq.svg | groq | Groq orange |
| lmstudio.svg | lmstudio | one colour, in the text colour |
| mistral.svg | mistral-color | in its colours |
| ollama.svg | ollama | one colour, in the text colour |
| openai.svg | openai | one colour, in the text colour |
| openrouter.svg | openrouter | one colour, in the text colour |
| searxng.svg | searxng-color | in its colours |
| tavily.svg | tavily-color | in its colours |
| xai.svg | xai | one colour, in the text colour |

Services without a file here (Wikipedia, sources found while researching) use a built-in glyph or the site's own icon, fetched through the network guard.

### Trademarks

The logos are trademarks of their owners. Tern shows them only to name each service; it is not affiliated with or endorsed by any of them. The MIT licence covers the drawings in Lobe Icons, not the trademarks.

### Adding a logo

Take the SVG from Lobe Icons (prefer the `-color` variant), strip the inline `style` and `<title>`, add it to the folder, add a row to `NOTICE.md` and to the table above, and add the id to the lookup in `brands.rs`.
