# UI audit and agentic UI plan

Status: planned. This is a docs-only audit. It folds the fixes into existing roadmap items (#91, #97, #98, #99, #102, #112), so no planned PR number changes.

## Why

The answers Tern produces are well structured. The interface around them is not yet at the same level. It shows layout breaks, state bugs, plain links, and no motion where motion would explain what is happening. This file lists every issue found in a desktop walkthrough (2026-11-04), the principle each one breaks, and the PR that fixes it.

## Principles (agentic UI)

1. **Less text, more inline UI.** When the agent offers choices, show them as actionable rows or buttons, not as a bullet list to retype.
2. **Context travels with replies.** Quote a block or an option into the composer as a chip, so a reply knows what it answers.
3. **A link is an object.** A URL or source becomes a rich container (favicon or real logo, title, site, page count or year), never a bare underline or a bare `1 2 3`.
4. **Every state looks different and moves gently.** Queued, searching, crawling, reading, writing, retrying, waiting and done each get their own glyph and a small motion (a fade with reduced motion).
5. **Streaming feels like typing.** Text arrives smoothly at a steady pace, never in jerky whole-paragraph jumps.
6. **Responsive at every level.** Blocks, and the text inside them, reflow at every panel width. No overflow, no bloated line lengths (about 60–80 characters), no clipped labels.
7. **Every view has a way back.** Any generated surface (3D, quiz, flashcards) has Back, Reset and Close.
8. **Real marks only.** Real brand logos (Lobe Icons, MIT, plus official marks) for sources and services, never generic or outdated icons.
9. **Proof is visible.** Verification and authenticity scores are first-class, clean and readable, bound to evidence ids (never set by the model).

## Issues found and where they are fixed

| # | Issue | Principle | Fix in |
|---|---|---|---|
| U1 | Blocks and the text inside them are not responsive; long lines and bloated text | 6 | #91 (layout tokens, max line length, reflow tests at 3 widths), #112 |
| U2 | Explain in 3D in Study has no Back, Reset or Close once generated | 7 | #98 (now), #117 (v2 tour with Back / Next / Play) |
| U3 | When you switch from Research to Study, the sidebar still shows the Research folders. In Study they are empty: opening one shows no chats | state | #91 (the sidebar is scoped to the current mode and built from that mode's real chats; a folder appears only where it holds chats in that mode; switching modes swaps the list cleanly, with no stale entries) |
| U4 | Not only the tool-call ledger: every option list in the side panel is plain (a bare `#`, plain text rows) and unstructured | 3, 4 | #91 (structured rows across the whole side panel: icon, label, short meta and state; the ledger row adds target, duration and result), #97 |
| U5 | Retry shows only `2/4`, with no micro-animation | 4 | #91 (attempt dots with backoff countdown; one row updated in place) |
| U6 | Options and buttons are not uniform or clean (right panel, Study chat, everywhere). The view that opens when you click one is not clean either | 1, 6 | #97 (one OptionRow component everywhere: clean, consistent buttons; the panel or screen it opens is calm and distinctive, easy to read, and lets the user focus on the content without effort) |
| U7 | Chat text appears too fast and in jerks (a block at a time) | 5 | #91 (a smoothed token pacer: steady characters per frame, catches up without jumps, off with reduced motion) |
| U8 | Web crawler, searching, creating, loading, retrying and queued states look alike | 4 | #91 (state glyph set plus shimmer, sweep and pulse motions) |
| U9 | Citations are hard to find (`1 2 3`, underlined only) | 3 | #91 (citation chips: favicon or logo, `Author Year`, quote on hover), #97 (SourceCard rich container) |
| U10 | Icons are not the real logos | 8 | #91 (audit every icon against the logo rule) |
| U11 | Chats and text need cleaner organisation | 6 | #91 (sidebar grouping by date and folder, pinned, search), #112 |
| U12 | Flashcards, quiz and the side panel need a redesign | 1, 6 | #98 (Flashcard and Quiz components), #102 (tutor surfaces) |
| U13 | Verification and authenticity scores are not yet engaging | 9 | #99 (ClaimBoard verdict meters, per-claim evidence strip), #95 (scores from code) |
| U14 | Reply has no quoted context | 2 | #97 (quote-to-composer chip on any block or option) |

## Reference libraries (patterns only)

Tern's desktop UI is GPUI (Rust). These are React and Tailwind libraries, so Tern cannot use them directly. We study their patterns and rebuild them natively in Tern's own design.

| Library | What to study | Licence note |
|---|---|---|
| [BoardUI](https://www.boardui.com/) | Agent thinking, agent progress, web search stream, composer loader, composer attachments, agent limits, task list, AI chat template | Free tier MIT on GitHub (BoardUI/boardui); Pro is paid. Ideas only, no Pro code. |
| [Arc UI](https://uiarc.dev/) | AI chat block (streaming, reasoning, tools, sources), command palette, toast stack, morph select, tabs, reduced-motion behaviour | 121 free components; Pro is paid. Ideas only unless the licence allows. |
| Codex agent UI pattern (reel) | Fold bullet options into actionable buttons, add context to replies, turn a URL into a rich container | Pattern only, not identity |

## Later

- **Proof UI:** when the evidence engine matures (#95, #99), a clean "proof" view: each claim, its quote, its source, its verification score, and an overall authenticity summary.
- **A third Tern mode (idea, unscoped):** after Research and Study, a harness where people bring any project, software or their own field, and discuss it with modes (technical, and other domain modes). Name and scope are open. It is not on the numbered roadmap yet.
