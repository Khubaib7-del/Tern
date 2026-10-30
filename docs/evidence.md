# Evidence chain and claim verification (#83)

Tern decides what counts as evidence; the model only chooses what to look for and what to say. This page describes how a citation in an answer is tied to text Tern read, and how each claim is weighed across sources.

## Quote ids

When Tern reads a page, any passage it quotes gets an id: six hex digits hashed from the page's address (without its fragment) and the passage's words (lower case, spacing squashed). The same passage always gets the same id, and an id cannot be guessed without the text. `paper_read` shows each passage with its id and locator, for example `[q:3f9a1c · p. 4 · 3 Method]`, and lists the quotes in its result.

## Citations

An answer cites a source as `[n]` (the n-th page the turn read, or the number its closing link list gives a page Tern read) or a passage as `[[q:3f9a1c]]`.

| Status | Meaning | Shown as |
| --- | --- | --- |
| Bound | Tied to a quote Tern minted from that page, about the claim | A chip; hover shows the quote |
| Unbound | Tern read the page, but no passage of it bears on the claim | A chip; the Report lists it |
| Unread | No page the turn read has that number | Listed as "Cites a source Tern never read" |
| Forged | A quote token whose id Tern never minted | Stripped from the answer, listed in the Report |

A quote token becomes `[n]` only when the id is one Tern minted from a page the turn read.

## Stances and agreement

For each claim, Tern compares every page the turn read (up to 8 per claim; the cited ones always) and gives each a stance:

| Stance | When |
| --- | --- |
| Supports | A passage shares most of the claim's words, with its numbers and the same polarity |
| Refutes | A passage about the same thing gives another number in the same place, the opposite polarity, an antonym, swapped sides of a comparison, or another name where the claim has its own |
| Mixed | One passage supports it and a different passage refutes it |
| Not enough info | No passage settles it |

These are the same offline checks as the claim verdicts, so they run with any model, including small local ones, and cost nothing. When claims are judged by the chat's model (the default in the app), the model looks again at up to 8 cards (counter-search pages first) and its answer replaces the offline stance; it cannot turn a passage that shares under half of the claim's words into a refutation.

The **agreement** counts sources per independent site, never passages: two pages on one site count once. The claim's stance across sources is Supports or Refutes when every source that takes a side agrees, Mixed when they disagree, and Not enough info when none takes a side. Under three sources taking a side, the Report says "few sources".

## Counter-search

Searches are usually phrased to confirm, so key claims get one search for evidence against them before the report is final:

- which claims: the answer's first claim, a claim with a number, or a strong one ("always", "never", "the first", "causes"); never a hedged claim ("may", "suggests") or one a source already disputes;
- budget: two claims per answer, two new pages each, through the chat's search service; only when claims are judged by the model (the word-overlap mode stays offline);
- query: the claim's key words, then `disputed OR incorrect OR "no evidence"` (a negated claim is searched without its negation); a query that finds nothing is tried again with the key words alone.

The pages it reads join the claim's cards, marked counter-search, and the agreement is counted again. The run shows a **Counter-searched** step with what it found.

## Where it shows

- **Report › Review claims**: each claim's verdict, its stance chip, the agreement bar (solid for support, mid-grey for mixed, outlined for refute), one card per source with its quote, page or section, logo and stance, citations Tern could not back, and the counter-search line.
- **Citation chips**: hover an `[n]` for the quotes the answer's claims rest on in that source, each claim's stance and agreement; a chip for a page Tern never read says so.
- **Run report**: an Evidence section in the Markdown and the `evidence` field in the JSON.

## Code

`tern-research::evidence` (pure: minting, rendering, binding, stances, agreement, counter-search planning, the stance judge's prompt and reply); `tern-agent::verify` and `ChatSession::check_last_turn` (pages, search, model calls); the desktop's Report view and `richmd` chips. The `evidence` evaluation suite is described in [evaluation.md](evaluation.md).
