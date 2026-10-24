# Assets

What Tern ships besides code, where it came from, and under which licence.

## Tern's own assets

The tern mark, the Tern Crawl logo, Tern's custom glyphs, and Scout are Tern's own work, under the repository's licence (see [LICENSE](../LICENSE)). Interface icons are [Lucide](https://lucide.dev) (ISC licence).

## Third-party brand logos

The desktop app shows each provider's and search service's own logo beside its name (the model picker, the Providers screen, the search chip). These files live in [`crates/tern-desktop/assets/brands/`](../crates/tern-desktop/assets/brands/) and are looked up by `brands.rs` in the desktop crate.

| | |
| --- | --- |
| Source | Mostly [Lobe Icons](https://github.com/lobehub/lobe-icons), package `@lobehub/icons-static-svg` 1.98.0; Context7 and SubQ are the brands' own files |
| Licence | Lobe Icons: MIT, © LobeHub. The licence and the per-file source are kept with the files in [`NOTICE.md`](../crates/tern-desktop/assets/brands/NOTICE.md) |
| Changes | Size attributes changed, the inline `style` and `<title>` removed. Where Lobe Icons only has a one-colour drawing of a brand that has colours (LM Studio, Jina, TypeSafe Jev, Groq), the shapes are filled with the colours of the brand's official icon |

Every provider, search service, crawler, decision model, and MCP template
row shows its brand's coloured mark. A one-colour mark is used only where
the brand itself is one colour (OpenAI, xAI, Ollama, OpenRouter, GitHub,
Notion, Voyage, MCP, Wikipedia); it is drawn at full strength in the
theme's ink (white on dark, near-black on light) so it never reads grey.
Custom providers, which have no brand file, show their initial on an
accent tile.

| File | Source | Drawn |
| --- | --- | --- |
| anthropic.svg | Lobe Icons claude-color | its colours |
| brave.svg | Lobe Icons brave-color | its colours |
| cerebras.svg | Lobe Icons cerebras-color | its colours |
| context7.svg | context7.com logo mark | its colours |
| deepseek.svg | Lobe Icons deepseek-color | its colours |
| figma.svg | Lobe Icons figma-color | its colours |
| firecrawl.svg | Lobe Icons firecrawl-color | its colours |
| fireworks.svg | Lobe Icons fireworks-color | its colours |
| github.svg | Lobe Icons github | theme ink |
| google.svg | Lobe Icons gemini-color | its colours |
| groq.svg | Lobe Icons groq | Groq orange |
| jev.svg | Lobe Icons typesafeai + typesafe.ai favicon colours | brand colours |
| jina.svg | Lobe Icons jina + jina.ai favicon colours | brand colours |
| lmstudio.svg | Lobe Icons lmstudio + lmstudio.ai app icon colours | brand colours |
| mcp.svg | Lobe Icons mcp | theme ink |
| mistral.svg | Lobe Icons mistral-color | its colours |
| notion.svg | Lobe Icons notion | theme ink |
| ollama.svg | Lobe Icons ollama | theme ink |
| openai.svg | Lobe Icons openai | theme ink |
| openrouter.svg | Lobe Icons openrouter | theme ink |
| searxng.svg | Lobe Icons searxng-color | its colours |
| subq.svg | subq.ai/favicon.svg | its colours |
| tavily.svg | Lobe Icons tavily-color | its colours |
| together.svg | Lobe Icons together-color | its colours |
| voyage.svg | Lobe Icons voyage | theme ink |
| xai.svg | Lobe Icons xai | theme ink |

Services without a file here (Wikipedia, sources found while researching) use a built-in glyph or the site's own icon, fetched through the network guard.

### Trademarks

The logos are trademarks of their owners. Tern shows them only to name each service; it is not affiliated with or endorsed by any of them. The MIT licence covers the drawings in Lobe Icons, not the trademarks.

### Adding a logo

Take the SVG from Lobe Icons (prefer the `-color` variant; if there is none and the brand has colours, fill the shapes with the colours of its official icon, or use the brand's own SVG), strip the inline `style` and `<title>`, add it to the folder, add a row to `NOTICE.md` and to the table above, and add a `brand!` line and the id to the lookup in `brands.rs`. Check it on both themes before committing.
