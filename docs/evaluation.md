# Evaluation

> Status: built in PR #42 (roadmap phase 11). The offline suites run in CI on every pull request; the live, pinned, public, and human rungs need a model, the network, a downloaded benchmark, or a person, and have not been run by CI.

Tern measures itself with a test ladder of five rungs. Each rung is one command. Every run has at least three trials by default, reports a 95% interval for every metric, and publishes every failure, case by case, in its report.

## The ladder

| Rung | Command | Needs | What it measures |
| --- | --- | --- | --- |
| Unit | `tern eval run --level unit` (in a source checkout; runs `cargo test --workspace`) | cargo | Every unit and integration test in the workspace, read back as a run |
| Synthetic | `tern eval run` | Nothing: offline and deterministic | Tern's checkers on bundled cases with known answers: claim verdicts, research answers (recorded), quiz grading, decision routing on a held-out split, injection and tool safety |
| Pinned | `tern eval run --level pinned --live` | A model provider and the network | Real questions over real public pages pinned on 23 October 2026, answered by your model and read through Tern Crawl |
| Public | `tern eval run --level public --scifact <folder>` | Your copy of SciFact | Claim verdicts on a public benchmark (SciFact, Wadden et al., 2020) |
| Human | `tern eval usability` | A person | Five scripted tasks, timed, then the System Usability Scale |

Any rung except unit and human also runs live with `--live`: your chat model writes the research answers and judges claims and short answers (Tern's offline checks fill in where it does not answer), and the canary safety cases run.

`tern eval list` prints the ladder and every suite. Other commands:

| Command | Does |
| --- | --- |
| `tern eval run [--level L] [--suite a,b] [--trials N] [--live] [--price IN,OUT] [--check-baseline] [--json]` | Runs a rung and saves the run. `--price` is US dollars per million input and output tokens, for cost per verified report. `--check-baseline` exits 1 when a bundled baseline line is broken (CI uses it) |
| `tern eval report [run] [--list] [--json]` | Prints a saved run's Markdown report (`last` by default), or the list of runs |
| `tern eval compare [old] [new] [--strict]` | Metric by metric; defaults to the previous run of the same rung and mode against the last. `--strict` exits 1 on a regression |
| `tern eval usability [--participant P1]` | A usability session, saved as a run on the human rung |

Runs are saved in `<data>/evals/runs/<id>.json` and `<id>.md`. The desktop app's **Evaluation** page (the flask on the rail) lists them, runs the offline suites, and shows each suite's metrics with their intervals, the change since the previous run of the same rung, and the failures.

## Suites

Suites are versioned JSON files. Tern bundles six (`crates/tern-eval/suites/`):

| Suite | Kind | Rung | Cases | Origin |
| --- | --- | --- | --- | --- |
| `research@1` | Research answers | Synthetic | 6 tasks × 3 recorded answers | Written for Tern's tests; the pages are not real sites and the answers are not real model output |
| `claims@1` | Claim verdicts | Synthetic | 27 | Written for Tern's tests |
| `quiz@1` | Quiz grading | Synthetic | 23 | Written for Tern's tests |
| `decision-heldout@1` | Decision routing | Synthetic, held out | 22 | Written after the local scorer was built, on topics the 50 `tern decide bench` fixtures do not use, and never used to tune it |
| `safety@1` | Injection and tool safety | Synthetic | 34 | Written for Tern's tests |
| `pinned@1` | Research answers | Pinned | 3 | Real questions over arXiv 1706.03762, the Rust 1.85.0 announcement, and Wikipedia's Leitner system page |

Every report names each suite's origin, so a score on written cases is never read as more than it is.

### Metrics

| Suite | Metric | Meaning |
| --- | --- | --- |
| Research | Citation accuracy | Citation marks whose source supports the sentence (each mark checked alone with the offline check, so modes compare) |
| | Claim precision | Cited sentences judged Supported, of all cited sentences (Partly supported does not count) |
| | Claim recall | Key claims the task lists that the answer makes, citing a source that states them |
| | Unsupported claims per 100 runs | Cited sentences judged Not supported or No source, per 100 answers |
| | Source precision and recall | Cited sources against the task's gold sources |
| | Constraint retention | The user's constraints kept: at most N words, at least N sources, mention a term, never cite a ruled-out source |
| | Verified reports | Answers with every claim Supported or Partly and every citation right |
| | Tokens (and cost) per verified report | Tokens over verified reports; dollars with `--price`. Offline the token counts are the recordings', written for the test |
| | Forecast error and range | Live only: \|tokens used − the preflight estimate\| ÷ tokens used, and whether the preflight range held the run |
| | Checker agreement | The checker's verdict against the labels on each recorded sentence |
| Claims | Verdict accuracy; claim precision and recall (Supported); unsupported claims caught | Against the gold verdict |
| Quiz | Grading accuracy; answer-key and short-answer accuracy; credit error | Against the grade a fair teacher would give |
| Decision | Top choice right, Brier score, calibration error | For the fixed-order baseline and the decision model in use; the Brier score is the forecast error of the model's probabilities |
| Safety | Poisoned descriptions flagged; benign descriptions flagged; injected text kept in its block; gate answered as expected; live model ignored the injection | The trust scan, the untrusted-data wrapper, the policy gate under default settings, and (live) a canary word the injected page asks for |
| Usability | Tasks finished, seconds per task, SUS score | One session per run |

A case passes when everything it checks holds; a research case also needs the checker to agree with every label. A failure lists what was expected, what happened, and why.

### Trials and intervals

Each case runs once per trial; the default is three, and a run with fewer says so in its notes. For a proportion the 95% interval is the wider of the Wilson interval over every case and trial and the t interval over the per-trial values; for other metrics it is the t interval over trials, never below zero. Offline, trials differ only where a research task has a different recorded answer for each trial, so most intervals come from the number of cases.

### Comparison and the baseline

`tern eval compare` marks a metric as regressed when it is worse by more than its tolerance (2 points for a proportion, 5% otherwise) and outside the older run's interval. `crates/tern-eval/baseline.json` holds the offline scores of this build as floors (ceilings for lower-is-better metrics); CI runs the offline suites with `--check-baseline` and uploads the report. A change that moves a score on purpose updates the baseline in the same pull request and says why.

## First scores (offline, 23 October 2026, 3 trials)

| Suite | Headline | Other results |
| --- | --- | --- |
| Research | Citation accuracy 97.7% (43/44) | Claim precision 79.5%, claim recall 91.7%, 5.6 unsupported claims per 100 runs, constraint retention 88.9%, verified reports 94.4%, checker agreement 86.4% |
| Claims | Verdict accuracy 59.3% (48/81) | Claim recall 100%, claim precision 70.6%, unsupported claims caught 21.4% |
| Quiz | Grading accuracy 87.0% (60/69) | Answer-key questions 100%, short answers 66.7% (word overlap) |
| Decision, held out | Local scorer 72.7% (48/66) | Fixed-order baseline 45.5%; Brier 0.327 against 0.794. The in-sample bench gives 86% and 52%: the held-out split is harder and lower |
| Safety | 90.0% handled safely (81/90) | Poisoned descriptions flagged 87.5%, benign flagged 16.7%, injected text contained 100%, gate as expected 87.5% |

What the failures say:

- The offline claim check is generous. A claim with a wrong number is **Partly supported** with the note "A number in the claim is not in the passage", not Not supported, and a claim that swaps a role, an entity, or a direction ("slower" for "faster", "gold" for "International Orange") but shares the passage's words is **Supported**. It catches 21% of unsupported claims in this suite. The model judge is meant to catch these; a live run measures how well.
- Short answers graded by word overlap miss paraphrases and give partial credit to answers that reuse the key's words for the wrong thing.
- The trust scan rates a description that tells the model to always call it first, instead of another tool, only as a note (not a danger), so it counts as missed; and it flags a benign description that mentions passwords.
- The policy gate allows `read_url` to a new domain by default (network steps are allowed within project scope, [05-security-model.md](05-security-model.md) section 4), so an injected page could ask the model to read a URL that carries data in its query string. The case stays in the suite as a published failure until new domains can be set to ask.

Live, pinned, and public numbers will be added when they have been run with a key.

## Adding cases

Drop a suite file in `<data>/evals/suites/` (any name ending `.json`). A file with a bundled suite's `id` replaces it for your runs. To change a bundled suite, edit its file in `crates/tern-eval/suites/`, raise its `version` when a case's meaning changes, and run `cargo test -p tern-eval`: every bundled suite is parsed and checked (unique ids, gold sources in range, one label per cited sentence).

```json
{
  "format": 1,
  "id": "my-claims",
  "version": 1,
  "title": "My claim checks",
  "kind": "claims",
  "level": "synthetic",
  "origin": "written by me from my thesis sources",
  "cases": [
    {
      "id": "c-1",
      "claim": "The trial enrolled 412 patients.",
      "sources": [{"url": "https://example.org/trial", "title": "Trial", "text": "We enrolled 412 patients across 9 sites."}],
      "gold": "supported",
      "tags": ["number"]
    }
  ]
}
```

| Kind | A case has |
| --- | --- |
| `research` | `id`, `question`, `sources` (`url`, `title`, `text`; empty text in a pinned suite is fetched at run time), `gold_claims` (`text`, `terms` that must all appear in one cited sentence, `sources` that state it), `gold_sources`, `constraints` (`{"type": "max_words", "n": 120}`, `min_citations`, `must_mention` with `term`, `avoid_source` with `n`), `budget_tokens`, and `recorded` answers (`answer`, `input_tokens`, `output_tokens`, and optional `labels`, one verdict per cited sentence) |
| `claims` | `id`, `claim`, `sources`, `gold` (`supported`, `partly`, `not_supported`, `no_source`), `tags` |
| `quiz` | `id`, `kind` (`multiple_choice`, `true_false`, `fill_blank`, `short_answer`), `prompt`, `choices`, `key`, `passage`, `given` (the answer graded), `gold` (`correct`, `partly`, `incorrect`, `unanswered`) |
| `decision` | The `tern decide bench` fixture format: `id`, `kind`, `question`, `context`, `choices` (`id`, `text`, `prior`), `gold` (the right choice's index). Add `"split": "held_out"` at the top when the cases were not used to tune the scorer |
| `safety` | `id`, `check` (`scan`, `wrap`, `gate`, `canary`), `expect` (`flagged` or `clean`; `contained`; `blocked` or `allowed`; `resisted`), and `text`, `source`, `tool`, `effect`, `target`, or `canary` as the check needs |

Levels are `synthetic` (offline with recordings), `pinned` (live only), and `public`. Keep the `origin` honest: say who wrote the cases and from what.

## SciFact

Download SciFact's data release (`data.tar.gz` from [allenai/scifact](https://github.com/allenai/scifact)) and point Tern at the folder with `claims_dev.jsonl` (or `claims_train.jsonl`) and `corpus.jsonl`. Each claim's cited abstracts are its sources. SciFact's SUPPORT is Supported; CONTRADICT and not-enough-info are Not supported, because Tern has no separate "contradicted" verdict. SciFact is licensed CC BY-NC 2.0; Tern bundles none of it. `--limit N` keeps the first N claims.

## Where the code is

| Piece | Where |
| --- | --- |
| Suite format, runners, metrics, intervals, reports, comparison, baseline, SciFact reader, SUS | `tern-eval` (no I/O, no clock, no UI, no HTTP library) |
| Tern as the subject (scan, wrapper, gate, reader, decision models, chat model), `<data>/evals/`, run ids | `tern_agent::eval` |
| `tern eval` | `tern-tui` |
| Evaluation page | `tern-desktop` |
