# ADR 0007 · The decision model: a local scorer, System One and rerank adapters, hard checks first

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-23 |
| Decider | Khubaib Nazeer |

## Context

Phase 8 puts a decision model beside the reasoning model. A decision model does not write text: it is given a question and a fixed set of choices and returns a probability for each, in a fraction of the time and cost of asking the chat model. The contract in [03-architecture.md](../03-architecture.md) is `score(context, question, choices) -> probabilities`, and hard checks always override its score. The roadmap asks for a local fallback that works offline, a first remote integration only after its API and pricing are verified, BYOK like any provider, wiring into source ranking, next-tool choice, claim pass or fail, and now-or-later, and a measurement against the reasoning-model-only baseline. The product spec's open question 3 was which provider to integrate first.

## What was checked (23 October 2026)

Every row below was read on the provider's own documentation, not a reseller or a summary.

| Provider | Product | Public API | Request and answer | Price (input; output free unless noted) | Integrated |
| --- | --- | --- | --- | --- | --- |
| TypeSafe | Jev (`jev-latest` = `jev-1.13.0`; `jev-preview` = `jev-1.14.0`) | Yes. `POST https://api.typesafe.ai/v1/systemone`, Bearer key from console.typesafe.ai ([API reference](https://docs.typesafe.ai/api), [Choice](https://docs.typesafe.ai/primitives/choice.md)) | A `state` and named typed questions (`noul`, `choice`, `score`); a `choice` answer carries `choice`, `probabilities` over every option (summing to 1), and `confidence`; `usage.input_tokens` | $0.042 per million tokens ($42 per billion) for both 1.13 and 1.14 ([Models](https://docs.typesafe.ai/models)). 64k tokens per request, 32k per question by default | Yes |
| Subquadratic | SubQ Snap (`subq-snap-1.1`) | Yes, public preview (waitlist when capacity is reached). `POST https://api.subq.ai/v1/systemone`, the same System One shape ([Decisions API](https://docs.subq.ai/endpoints/decisions-api)) | Same as Jev: 1 to 128 named questions, `choice` with 2 to 255 options, probabilities and confidence; `output_tokens` always 0 | $0.04 per million tokens on the models page ([Models](https://docs.subq.ai/using-the-api/models/)); the launch post on 12 October said $0.05 for the earlier `subq-snap-preview` ([launch](https://subq.ai/introducing-subq-snap)). 384K-token window | Yes |
| Voyage AI | Rerank (`rerank-2.5-lite`, `rerank-2.5`) | Yes. `POST https://api.voyageai.com/v1/rerank` ([reference](https://docs.voyageai.com/reference/reranker-api)) | A query and up to 1,000 documents; `data[]` of `index` and `relevance_score`, `usage.total_tokens` | $0.02 (`rerank-2.5-lite`) or $0.05 (`rerank-2.5`) per million tokens, first 200 million tokens free per account ([pricing](https://docs.voyageai.com/docs/pricing)) | Yes, for ranking only |
| Cohere | Rerank 5 | Yes. `POST https://api.cohere.com/v2/rerank` ([reference](https://docs.cohere.com/reference/rerank)) | Query and documents, `relevance_score` per result | Not verified: the [pricing page](https://cohere.com/pricing) shows only Model Vault instance prices and defines a "search unit" without a public per-search price | No |
| Jina AI | Reranker | Yes. `POST https://api.jina.ai/v1/rerank` | Query and documents, `relevance_score` | Not verified: the pricing page is rendered by script and could not be read | No |

Both founder-named models, Jev and SubQ Snap, have public APIs, and they share one wire format, so one adapter serves both. Third-party sites that resell Jev under other domains were ignored.

## Decision

| Part | Decision |
| --- | --- |
| Crate | `tern-decision`, GPUI-free, depending only on `tern-core` (plus `reqwest`, `serde`, `tokio` time). The dependency-rules test already lets it reach the network; it reads no keys itself |
| Contract | `DecisionModel::score(&DecisionRequest) -> Decision`. A request has a kind (source ranking, next tool, claim check, now or later, general), context, a question, and choices (id, text, optional prior). A decision has one probability per choice summing to 1, the chosen index, confidence (1 minus normalised entropy, or the provider's own), a rationale, the model and provider, latency, cost (input tokens and dollars at the published price), any fallback reason, any hard-check overrides, and whether it was cached |
| Local scorer | `local-lexical-1`: BM25 and word coverage between the question and each choice, plus per-kind rules (site authority tiers for sources; best-passage coverage with the number and negation checks for claims; cue words for tools; urgency and "later" cues and the caller's prior for triage), turned into probabilities with a softmax. Deterministic, offline, free; no model download |
| Remote | One System One adapter for Jev and SubQ Snap (a single `choice` question; options `c1`, `c2`, ... with the choice text as their description; the context as `state`); one rerank adapter for Voyage (relevance scores through a sharp softmax). The reranker says it cannot answer claim checks or tool choice, and those fall back to local |
| Decider | Off, Local (the default), or Remote. A remote error, a timeout (4 s by default, 0.2 to 30 s in preferences), or an unsupported kind falls back to the local scorer with the reason logged. An in-memory cache of 256 decisions; a log of 500 |
| Hard checks | `Rule::RuleOut` and `Rule::Force` are applied after scoring and always win: pages already read are ruled out of source ranking; a claim with no readable source is never scored (it stays No source); a number in the claim missing from the best passage, or a flipped negation, rules out "supported" |
| Keys | BYOK in the OS keychain as `decision.<id>`, tested with one small real decision before they are saved (`tern decide key add`, the desktop page's Add). A provider that echoes a key back has it removed before the error is shown |
| Wiring | Quick ask and every deep worker rank search results before picking pages to read; chat's `web_search` tool returns results in the decision model's order; a first-tool hint (`web_search`, `read_url`, vault search, or answer now) is added to the system prompt when the model is at least 50% sure; claims are pre-scored after word overlap (a confident pre-score may move a verdict by one step, as method `decision`) and, when there are more than 16 claims, the least certain go to the model judge, which keeps the final word; deep workers start in the decision model's order when there are more sub-questions than slots; due watches run most pressing first |
| Log | Each decision is a `decision.made` event (or rides in the `web_search` tool's metadata, or in `claims.checked`), read into `Run::decisions` for the run panel's Decisions view |
| Measurement | `tern decide bench` scores a bundled fixture set (50 items: 14 source ranking, 16 claim checks, 10 tool choices, 10 now-or-later) for the fixed-order baseline, the local scorer, and, when set up, the chat model asked as multiple choice (`--reasoning`) and the remote decision model (`--remote`) |

## Results of the offline run

`tern decide bench` on 23 October 2026, offline (no provider keys on the build machine):

| Model | Accuracy | Claim check | Next tool | Source ranking | Now or later | Brier | ECE | p50 latency | Cost |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline: fixed order (search order, plan order, first tool, word-overlap verdict) | 52% (26/50) | 9/16 | 3/10 | 7/14 | 7/10 | 0.723 | 0.278 | 0 ms | $0 |
| Local decision model | 86% (43/50) | 9/16 | 10/10 | 14/14 | 10/10 | 0.291 | 0.129 | 0 ms | $0 |

How to read it:

- The fixture set is small and was written alongside the local scorer, so the local row is an in-sample number and optimistic. It shows the scorer works and is calibrated better than fixed order (lower Brier and expected calibration error); it is not evidence of how it does on real runs. Phase 11 builds the real evaluation.
- On claims the local scorer ties the word-overlap check (9/16): it uses the same features, so paraphrases ("dispensing with recurrence" for "without recurrence") and partial attributions defeat both. This is where a remote decision model or the model judge is needed.
- The reasoning-model and remote rows were not run: they need a provider key and a decision-model key on the machine. `tern decide bench --reasoning --remote` adds them, with latency, tokens, and cost per row.

## Consequences

- The decision model is on by default in Local mode, so search results are re-ordered and watches and workers may start in a different order than before. Off restores the old behaviour exactly.
- Remote decisions send the question, the choices' text, and the context (a passage, at most 80,000 characters for Jev, 200,000 for SubQ Snap, 24,000 for Voyage) to the chosen provider. Remote is never chosen by default.
- The rerank adapter covers only source ranking and now-or-later; a System One model covers every decision point.
- Prices are recorded in the catalog as of this date; they will drift and must be re-checked when the catalog changes.

## Revisit when

- A real evaluation (phase 11) shows the local scorer hurts any decision point.
- Jev or SubQ Snap leave preview, change their wire format or prices, or Cohere or Jina publish a verifiable per-token price.
- A small local cross-encoder can run without a large download, which would replace the lexical scorer for claim checks.
