# ADR 0005 · Quick ask: BYOK search, a guarded reader, checked citations, source icons

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-22 |
| Decider | Khubaib Nazeer |

## Context

Phase 3 ends with `tern ask`: a short answer in which every citation links to a page Tern actually fetched. That needs a search service, a reader that is safe to point at the open web, and a way to check citations. The owner also asked that every source and service show its own logo (a built-in icon for known services, the site's favicon otherwise), the way ChatGPT shows Firecrawl or GitHub beside a step.

## Decision

| Part | Decision |
| --- | --- |
| Search | Bring your own key: Tavily or Brave Search, or a self-hosted SearXNG with no key. Keys live only in the OS keychain (`search.<id>`), are tested with one real query before saving, and are managed with `tern search list/add/test/remove` |
| Reader | `tern-crawl::Fetcher`. http and https only, no credentials in links, every hop's host resolved and refused if private, loopback, link-local, shared, or reserved (IPv4 and IPv6, including mapped and NAT64 forms). Redirects are followed by hand, at most 5, each re-checked. 2 MiB cap, 20 s timeout. HTML is reduced to main text (article, main, or body without nav, header, footer, scripts, forms); plain text and JSON pass through; PDF is refused until phase 4 |
| Quick ask | One search, up to 4 reads in parallel (different sites first), one model call over numbered sources (6,000 characters each), temperature 0.2. Unreadable pages are listed with their reason, never cited |
| Citation check | Every `[n]` in the answer is parsed (`[1][2]`, `[1, 3]`, `[2-4]`). Numbers with no read source are reported as invalid. Sources read but not cited are marked. The final event lists the cited URLs as evidence |
| Chat tools | When a search service is set up, the chat TUI gets `web_search` and `read_url` (Network class, allowed by default; `read_url` targets the site) |
| Source icons | Every search hit and page carries `site` and `SourceIcon { builtin, favicon }`. Known services get a built-in glyph id (`github`, `arxiv`, `wikipedia`, `doi`, `semantic-scholar`, `pubmed`, and others); other sites get the page's declared icon or `/favicon.ico`. Search services have their own ids (`tavily`, `brave`, `searxng`), and Tern's reader is `tern-reader` |

## Consequences

- The desktop app draws each source row with its logo; the TUI shows the site name, since it cannot draw images. Favicons are fetched by the desktop app through the same guard and cached, never hotlinked from the web view.
- The `tern-reader` glyph and the other built-in glyphs are part of the custom icon set (UI definition §6). They are designed with the desktop app and need the owner's approval before they ship.
- The guard checks resolved addresses before each request, but the HTTP client resolves again; pinning the checked address (full DNS-rebinding protection) comes with the phase 4 reader.
- A model can still cite a real source for a claim that source does not make. Claim-level verification is phase 5 and later.
