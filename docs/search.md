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

## Relevance guard and the Wikipedia notice (#67)

A keyword index answers what it does not know with pages that match one common word: "SubQ Snap 1.1 AI model definition or meaning" came back from Wikipedia with six pages about Japanese words and medicine, and "harness2 by Google AI model or project" found nothing at all. So every `web_search` result is held to the question's main terms (`crates/tern-agent/src/relevance.rs`):

- **Strong terms**: quoted phrases, words with digits (`harness2`, `gpt-4o`), a name with its version (`Snap 1.1`), and words with an inner capital (`SubQ`, `LangChain`).
- **Names**: other capitalised words that do not start the question (`Google`, `Lahore`).
- **Keywords**: the rest, without filler and without generic words (`model`, `project`, `definition`, `meaning`, `AI`…).

A result (title, snippet and link) must carry one term of the strongest kind the question has, or it is dropped. A generic word alone never lets a page through, and a version number alone (`1.1`) is never a term. When nothing is left, Tern tries shorter keyword queries (never a lone generic word) and then says so. `tern.log` records each search: `web_search: service=Wikipedia query=… used=… tried=N found=F filtered=D results=K terms=[…]`.

When the service is **Wikipedia** and nothing on-topic is left, the tool tells the model plainly that Wikipedia has nothing on it (so it does not invent an answer), and the desktop shows a notice over the composer: *"Wikipedia has nothing on this. Add a free Tavily key (1,000 searches/month) or Brave under Settings › Search & Crawlers for full-web results."* with **Add a Tavily key**, which opens that row with its key field ready. While only Wikipedia is set up, Settings › Search & Crawlers and the first-run tour's search step recommend Tavily.

The `web_search` tool asks the model for a short keyword query of 2–6 words (names, products, versions), not the user's whole sentence, and to call it once per separate topic. The status line names the service ("Searching Wikipedia…"), and the answer's working rollup says which service answered and how many off-topic pages were dropped ("Searched 1 on Wikipedia · 6 off-topic dropped").

## Compound names (#68)

A name made of several words, or one with a version (`SubQ Snap 1.1`), is matched as a whole. A result must contain the name (`SubQ Snap`), or every one of its distinctive words (`SubQ` and `Snap`). One of them alone is not enough: on the Windows test, Wikipedia's medical pages matched `Snap` and got through. When the question has two or more strong words, the keyword fallback never searches just one of them. The fixture `crates/tern-agent/tests/fixtures/wiki-subq-medical.json` holds the pages from that log, and its test checks that none of them is kept.

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

## Keyword fallback

Wikipedia and SearXNG are keyword indexes: a whole typed sentence, typos included ("can you provide me infor about how langchain erupted and who was the developre"), matches nothing, while "LangChain" matches thousands of pages. When a search comes back empty, `web_search` tries shorter queries in turn before it reports nothing, and stops at the first that finds results:

1. the question's keywords, filler and asking words removed (at most six: `langchain erupted developre creator`),
2. its first three, two and one keywords,
3. its longest word alone (a name usually is).

A query is never tried twice. The results say which query found them, and `tern.log` gets one line per search (`web_search: service=… query=… used=… tried=N results=N`). When every query finds nothing, the model is told the search found nothing and not to claim it cannot reach the web.

## When the model will not use the results

Messages that plainly ask for the web ("use the web tool", "search online", "google it", "look it up", "browse about that", typos and quotes included) are searched before the first model call; one that only points back ("browse about that") searches the previous question. Results reach the model framed as live results retrieved just now. If a model still answers that it cannot browse or does not know, or repeats its previous reply word for word (40 characters or more), Tern searches the question itself once, gives the model the results and asks again (`web.safety_net` in the run's events). A copied reply is never shown twice.

