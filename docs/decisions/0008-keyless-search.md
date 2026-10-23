# ADR 0008 · Keyless web search: Wikipedia's API, not a scraped search page

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-23 |
| Decider | Khubaib Nazeer |

## Context

Deep research failed on every run in the PR #43 test build: each worker's searches failed and the run said "No worker found a page to cite". The desktop app had no search key, so it fell back to SearXNG, the only "usable without a key" service, and it always sent queries to `http://localhost:8080` because the desktop never passed an address. Nothing listened there (or a stock SearXNG answered 403, since JSON output is off in its default `settings.yml`). A fresh install has no working web search at all unless the user brings a Tavily or Brave key or runs SearXNG.

PR #44 makes the real error visible, adds a SearXNG address with a Test button, and checks search before a deep run spends a plan call. It also needs a search that works with **no key and no server**, without breaking any provider's terms.

## What was checked (23 October 2026)

| Candidate | What its terms say about programs querying it | Verdict |
| --- | --- | --- |
| DuckDuckGo (`html.duckduckgo.com`, the "lite" pages) | No public web-results API. The [Terms of Service](https://duckduckgo.com/terms) and [Acceptable Use Policy](https://duckduckgo.com/acceptable-use) require use "as authorized", forbid interfering with the services and framing or reselling them, and say service-specific terms apply. The HTML endpoint is a page for people, not an API offered to programs | Not used. Scraping a search page that is not offered to programs is not authorised use |
| Mojeek (HTML search) | The [Terms of Service](https://www.mojeek.com/about/terms.html) forbid using or accessing the services "by any automated means (unless you are an authorised Mojeek API user)" and say "scraping the Services without Our prior consent is expressly prohibited". The [Search API](https://www.mojeek.com/services/search/web-search-api/) is paid, with a key | Not used without a key. It could be a keyed service later, like Tavily and Brave |
| Wikipedia (MediaWiki Action API, `list=search`) | The [Wikimedia API usage guidelines](https://foundation.wikimedia.org/wiki/Policy:Wikimedia_Foundation_API_Usage_Guidelines) allow programs to call the APIs without prior permission, provided the client follows the User-Agent policy (a descriptive agent with contact information), honours throttling, stays at a normal rate, does not spread traffic over many agents, and does not white-label the API | **Used** as the keyless default |
| OpenAlex, arXiv, Crossref | Scholarly APIs with their own polite-use rules. Good for papers, not for general questions | Not in this PR; candidates for a scholarly search later |

## Decision

| Part | Decision |
| --- | --- |
| Keyless default | `wikipedia` joins the search catalog (Tavily, Brave, SearXNG, Wikipedia). It needs no key and no server. `search::connect` with no choice uses the first service with a saved key, else Wikipedia. Setting the search choice to `off` turns web search off |
| Calls | `GET https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch=…&srlimit=…&format=json&formatversion=2`. One query per search, results read as article links (`/wiki/<Title>`); pages are then read through Tern Crawl like any other source, with robots.txt and per-site pacing |
| Identification | Every search request carries `User-Agent: Tern/<version> (https://github.com/Khubaib7-del/Tern; research assistant)`, as the Wikimedia User-Agent policy asks. API errors (such as `maxlag`) and HTTP 429 are reported as they are, not retried in a loop |
| Labelling | The Providers row says "Free, no key needed · searches Wikipedia articles only", and the composer status names the service, so nobody mistakes it for a general web search. The empty chat says which service Tern searches with |
| Not done | No scraping of any search engine's result pages, now or as a fallback |

## Consequences

- A fresh install's deep research works (encyclopedic questions get cited Wikipedia articles) instead of failing at the first search.
- Answers on news, products, or niche topics are thin with Wikipedia alone; the Providers screen and the docs say to add Tavily, Brave, or a SearXNG address for full web search.
- Queries go to the Wikimedia Foundation when no other search is set. This is stated in the README; `search: "off"` in `preferences.json` stops it.
