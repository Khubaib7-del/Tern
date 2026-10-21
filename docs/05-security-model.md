# 05 · Security model

> Status: planned, 21 October 2026. These are binding design rules for every crate. None is implemented yet. To report a vulnerability, see [SECURITY.md](../SECURITY.md).

## 1. What Tern protects

| Asset | Why it matters |
| --- | --- |
| Provider and search API keys | Direct financial exposure and account abuse |
| The user's research: vault, library, notes, drafts | Unpublished work, personal data, academic integrity |
| The user's machine and network | A research agent fetches untrusted content and could be steered into local services |
| The integrity of reports | A forged or misattributed citation is a real-world harm for a student |

## 2. Threats and controls

| Threat | Control | Crate |
| --- | --- | --- |
| Key leakage through logs, events, crash reports, or config files | Keys live only in the OS keychain. They are fetched at call time, never written to config, never logged, never persisted in plain text. The redactor strips key-shaped strings and auth headers before any event, artifact, or error is stored | `tern-platform`, `tern-core`, `tern-providers` |
| Prompt injection from fetched content | Web pages, PDFs, OCR text, transcripts, and MCP results enter an isolated, clearly delimited data channel. They cannot change the plan, call tools, or grant permissions | `tern-agent`, `tern-crawl` |
| SSRF and DNS rebinding through the crawler | Block loopback, link-local, private, carrier-grade NAT, and cloud-metadata ranges for IPv4 and IPv6; re-check every redirect hop; resolve once and pin the IP for the connection; restrict schemes to `http` and `https` | `tern-crawl` |
| The model doing more than the user agreed to | The model proposes; the policy gate authorises exact arguments; an executor acts. Effect classes, scoped and expiring approvals, and grouped previews | `tern-agent` |
| Approval fatigue | Same-purpose actions are grouped into one preview, but grouping never auto-approves | `tern-agent` |
| Runaway cost | Preflight estimates, per-run caps, safe pause at the boundary, no silent provider or model fallback | `tern-agent`, `tern-providers` |
| Data sent somewhere unexpected | Local-first storage; a notice before data leaves the machine naming the provider and what is sent; optional routing of sensitive material to a local model | `tern-agent`, `tern-providers` |
| Malicious or careless extensions | MCP servers and skills are installed with a trust preview; installing does not grant permission to execute; their output is untrusted data | `tern-agent` |
| Fabricated citations | Every claim maps to a fetched source span; DOIs and URLs must resolve; unverified claims stay marked unverified | `tern-research` |

## 3. Trust order

Higher entries can never be overridden by lower ones.

| Rank | Source |
| --- | --- |
| 1 | Tern's built-in system rules and the policy gate |
| 2 | The user's direct instructions and approvals |
| 3 | Project rules (`TERN.md`) and the goal ledger |
| 4 | Skill and tool guidance |
| 5 | Retrieved content: web, PDF, OCR, transcripts, MCP results, worker summaries |

## 4. Effect classes

| Class | Examples | Default |
| --- | --- | --- |
| Read | Read the vault, search the library | Allowed |
| Analysis | Summarise, extract claims, score | Allowed |
| Network | Fetch a URL, call a search API | Allowed within project scope; new domains can be set to ask |
| Write | Edit a vault note, save a report | Shown as a diff; accept or reject |
| Credentials | Read a key from the keychain | Only `tern-providers` and `tern-decision`, never the model |
| External side effect | Post, send, or submit anywhere | Ask every time |
| Destructive | Delete notes, bulk delete chats | Ask every time; checkpoint first |

An approval is bound to a tool class, a target, the run, and an expiry. A materially new target asks again. Tern never describes an approval as a sandbox; only an enforced isolation mechanism may be called one.

## 5. Data handling

| Data | Where it lives | Leaves the machine when |
| --- | --- | --- |
| API keys | OS keychain | Only in the auth header of a call to that key's own provider |
| Vault and library | User's disk | Excerpts are sent to a remote model only for a task the user started, after the data-flow notice |
| Event log and artifacts | App data directory | Never, unless the user exports them |
| Crawl cache | App data directory | Never |
| Telemetry | None planned | Not applicable |

## 6. Verification plan

| Check | When |
| --- | --- |
| Redaction unit tests with key-shaped fixtures from each provider format | Phase 2 |
| Keychain round-trip tests on Windows | Phase 2 |
| SSRF matrix: private ranges, IPv6, redirects, DNS rebinding | Phase 4 |
| Prompt-injection fixtures in web pages, PDFs, OCR text, and MCP results | Phases 4 and 10 |
| Policy gate tests: scoped grants, expiry, new targets | Phases 3 and 5 |
| External review before any public release | Before distribution |
