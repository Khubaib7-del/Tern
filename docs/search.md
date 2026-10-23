# Web search setup

Tern searches the web for quick asks, chat answers, and every deep-research worker, then reads the pages it found through Tern Crawl. Four search services are built in. Pick one in the desktop app under **Providers › Search**, or with `--search <id>` in the terminal.

| Service | Id | Needs | What it searches |
| --- | --- | --- | --- |
| Wikipedia | `wikipedia` | Nothing: no key, no server | Wikipedia articles only (English) |
| Tavily | `tavily` | An API key | The whole web |
| Brave Search | `brave` | An API key | The whole web |
| SearXNG | `searxng` | A SearXNG server you run, with JSON output on | Whatever engines your SearXNG uses |

## Which service Tern uses

1. The service you chose (desktop **Providers › Search**, `"search"` in `preferences.json`, or `--search`).
2. Otherwise the first of Tavily and Brave with a saved key.
3. Otherwise **Wikipedia**, the keyless default, so a fresh install can research without any setup.

Set the choice to `off` (`"search": "off"` in `preferences.json`, or `--search off`) to turn web search off. The desktop composer's status line names the service in use, for example `Ollama · llama3.2 · web: Wikipedia`; with search off, no `web:` part is shown.

Wikipedia is called through the MediaWiki Action API (`list=search`), which the Wikimedia API usage guidelines allow programs to use. Every request identifies Tern with a descriptive User-Agent, and rate limits (HTTP 429, `maxlag`) are reported, not retried in a loop. Why Wikipedia and not a scraped search page: [ADR 0008](decisions/0008-keyless-search.md). Wikipedia alone is thin for news, products, and niche topics; add Tavily, Brave, or SearXNG for those.

## Tavily or Brave

Desktop: **Providers › Search**, paste the key on the service's row. Terminal:

```
tern search add tavily      # or: brave
tern search test tavily
```

Tern runs one real search with the key and saves it to the OS keychain only if the search works.

## SearXNG

SearXNG must return JSON. A stock install has JSON turned off and answers Tern with HTTP 403. In SearXNG's `settings.yml`, under `search:`, add `json` to `formats`, then restart SearXNG:

```yaml
search:
  formats:
    - html
    - json
```

Then point Tern at it:

- **Desktop**: **Providers › Search › SearXNG**. Type the **Server address** (for example `http://localhost:8888` or `http://192.168.1.20:8080`) and press **Test**. Tern runs one search against that address and shows the result or the exact error. **Choose** stays off until Test passes. The address is saved as `"search_url"` in `preferences.json`.
- **Terminal**: `tern search test searxng --base-url http://localhost:8888`, then `--search searxng --search-url http://localhost:8888` on `tern chat`, `tern ask`, or `tern research`.

Without an address Tern uses `http://localhost:8080`.

## When search fails

Tern says what went wrong instead of "found nothing":

| What you see | Cause | Fix |
| --- | --- | --- |
| `SearXNG at http://… did not answer: Connection refused` | Nothing is listening at that address | Start SearXNG, or correct the Server address |
| `SearXNG at http://… refused JSON results (HTTP 403)` | JSON output is off in `settings.yml` | Add `json` to `search.formats` as above |
| `… refused the key (HTTP 401)` | The Tavily or Brave key is wrong or expired | Paste a new key |
| `… is rate limiting Tern` | The service's rate limit (HTTP 429) | Wait, or use another service |

Deep research runs one test search **before** the plan. If search is down, the run stops at once with "Web search isn't working, so deep research did not start: …", and no plan call is spent. If search fails during a run, each worker card says "search failed: …" with the service's own error, and the run ends with "No search service answered: …". Once search works again, **Retry failed workers** reruns only the workers that failed and keeps the ones that finished.

"Found nothing" now means only that a search succeeded and returned no results.
