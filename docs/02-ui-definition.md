# 02 · UI definition

> Status: draft v1, 21 October 2026. Design rules for both surfaces. Open decisions are listed at the end. Nothing here is implemented.

## 1. Five rules

From a study of 33 agent products (desktop and terminal coding agents, cowork and research agents, and identity references), the premium ones follow the same five rules.

| # | Rule | What it means in Tern |
| --- | --- | --- |
| 1 | The process folds away; the result stays open | A turn's work is one line: `Searched 14 sources · read 6 · 3 claims verified ›`. The full log is one click away |
| 2 | The plan lives outside the chat | Plan, sources, and report sit in the right rail, not as checklists in the transcript |
| 3 | The accent covers under 1% of the screen | It marks only the one live thing: focus, primary button, active citation, running step |
| 4 | Status lives in one place, quietly | A spinner while running, a relative time when idle, one mark when it needs you. No badge chains |
| 5 | Panes open only when they have something in them | Right panels appear when a plan, source, report, or diff exists |

What counts as slop, concretely: gradient logos, sparkles, rotating witty status text, a card or badge on every row, multicolour tiles, generic SaaS blue or violet, emoji in UI, always-on credit counters, and anything that moves when nothing is happening.

## 2. Desktop layout (GPUI)

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ ◣ Tern   FYP-thesis ▾ / Lit review ▾ / Research ▾        ⌕   ◧ ◨   ⚙        │ title bar 36px
├───┬───────────────────┬──────────────────────────────────────┬───────────────┤
│ R │ SESSIONS          │                                      │ PLAN  SOURCES │
│ i │ ▸ Lit review   2m │   user message                       │ REPORT        │
│ b │   ↳ RAG eval      │                                      │ ───────────── │
│ b │ ▸ Viva prep    ◆  │   Searched 14 · read 6 · 3 verified ›│ 1 ✓ Scope     │
│ o │ ▸ Flashcards  1d  │                                      │ 2 ◌ Search    │
│ n │                   │   Answer text with citations [1] [2] │ 3   Verify    │
│   │ VAULT             │                                      │ 4   Write     │
│   │ ▸ notes/          │                                      │               │
│   │ ▸ papers/         │  ┌────────────────────────────────┐  │ budget ▮▮▯    │
│   │                   │  │ Ask, or steer the run…         │  │ $0.42 / $2    │
│   │                   │  │ Research ▾  model ▾  ▮▮▯  @ /  │  │               │
│   │              Scout│  └────────────────────────────────┘  │               │
├───┴───────────────────┴──────────────────────────────────────┴───────────────┤
│ ● running · step 2 of 4 · 18k / 200k ctx                       Windows · v0.1 │ status bar 24px
└──────────────────────────────────────────────────────────────────────────────┘
```

| Zone | Contents | Rules |
| --- | --- | --- |
| Ribbon (44px) | Five icons: Chat, Vault, Library, Study deck, Settings at the bottom | Monochrome, 16px; active = text colour plus a 2px left bar |
| Left sidebar (240px, collapsible) | Sessions by project, sub-sessions indented with `↳`, then the vault tree | Status only on the row: spinner, "2m", or one ◆ when it needs you |
| Title bar | Breadcrumb dropdowns `project ▾ / session ▾ / harness ▾`, search, pane toggles, settings | The harness switch lives here, not as a large toggle |
| Centre | Transcript, max width 720px | User messages on a tinted band; agent prose unboxed; work folded to one line per turn |
| Composer | A raised card with one chip row: harness, model, effort (3-segment meter), `@` context, `/` commands | Becomes "Steer" while running: Enter steers, Tab queues, Esc interrupts |
| Right rail (300px, only with content) | Tabs: Plan · Sources · Report · Graph · Context | Plan approval, evidence quotes, verdicts, checkpoints, and budget live here |
| Status bar (24px) | Run state, step, context use, platform | Text only; the one place cost and context are always visible |
| Vault editor | Tabs and breadcrumb, editor, right pane with Backlinks, Outline, Local graph | Plain `.md` and `[[links]]`, so the vault opens in Obsidian too |

## 3. How agent activity streams

One run, three views: **Summary** (default), **Detailed**, **Raw**. All three render the same event stream; only presentation changes.

| Element | Summary view | Detailed view |
| --- | --- | --- |
| A turn's work | `Searched 14 sources · read 6 · 3 claims verified · $0.08 ›` | Each step as one line, verb first |
| A tool call | Hidden in the rollup | `Read   arxiv.org/abs/…   p.3–7`, muted path |
| Thinking | `∴ Thought for 4s`, dim, collapsed | Plan, actions, and evidence; hidden reasoning is never streamed |
| Plan | Right rail, one line per step: ✓, spinner, or blank | Same, with per-step cost and comments |
| Needs you | One inline row: **1 Approve · 2 Approve and don't ask for this site · 3 No, and say what to do** | Same |
| Errors | One line in the error tone with "Retry step" | Detail in Raw |
| Final answer | Prose with numbered citations; hovering shows the exact quote | Claim list with verdict marks |

Verdict marks: ■ supported · ✕ contradicted · ○ unverified · ⌀ unreachable · — waived.

At the end of a run: "Keep all / Review each", with per-claim keep, reject, and comment.

Plain-language wording is fixed, for example: "Summarising earlier work to stay within the context limit", "The same step failed twice, so I paused", "2 of 5 claims verified; 3 not checked".

## 4. Terminal layout (ratatui)

| Rule | Detail |
| --- | --- |
| No boxes except dialogs | The transcript is unboxed; rounded corners only on the approval dialog |
| Launch card | A small dithered tern mark, version, and project, then gone |
| Task list | Current step plus a dim "Next:" line; full plan on `Ctrl+T` |
| Tool calls | `• Searched 14 sources └ read 6`, one batch per line |
| Footer | `? shortcuts · Research · model · 82% context left · $0.42` |
| Steering | Esc interrupt, Enter steer, Tab queue; queued messages shown as `↳` rows above the composer |
| Colour | One accent and three text levels (bold, default, dim); falls back from truecolour to 256 to 16 colours; never colour-only status |
| Capability probing | Truecolour, Unicode, mouse, bracketed paste, OSC 8 hyperlinks (for citations); never requires image protocols |
| Sizes | Works at 80×24; tested at 80×24 and 120×40; Windows Terminal first |
| Keys | `Space` expand, `Enter` inspect, `Esc` stop or back, `p` pause, `Ctrl+P` palette, `Ctrl+T` details, `?` help |
| Personality | Only the spinner, and it moves only while work runs |

## 5. Palette

A black and white base, one accent at or under 5% of pixels (target under 1%), desaturated status tones only on verdicts. Every main text and accent pair passes WCAG 4.5:1 in both modes.

| Token | A · Graphite & Bone | B · Ink & Paper | **C · Mono & Lichen (recommended)** |
| --- | --- | --- | --- |
| Background, dark / light | `#101010` / `#F4F2EE` | `#0E0F11` / `#F3F0E8` | `#0C0C0C` / `#F5F5F3` |
| Surface | `#161616` / `#FAF9F6` | `#141518` / `#F9F7F2` | `#131313` / `#FBFBFA` |
| Border | `#2A2A28` / `#DEDAD2` | `#282A2F` / `#DDD8CC` | `#272727` / `#DCDCD8` |
| Text | `#ECEAE5` / `#191917` | `#E8E7E3` / `#16171A` | `#EDEDED` / `#141414` |
| Muted | `#A3A09A` / `#5E5B55` | `#9FA1A6` / `#575A60` | `#A1A1A1` / `#5A5A57` |
| Accent | brass `#C2A878` / `#7D6535` | ink `#9AAABB` / `#3B4B5E` | lichen `#A3AE8E` / `#56603F` |
| Character | Warm, bookish | Cool, archival; may read as blue | Purest black and white, quiet green-grey |

Why C: it keeps the black and white mark dominant and avoids the hues the category overuses (orange, violet, SaaS blue). Brass is the runner-up.

Research and Study are told apart by tone and shape, not hue:

| | Research | Study |
| --- | --- | --- |
| Surface | Cooler graphite | Warmer paper |
| Markers | Square ■ | Round ● |
| Radius | 6px | 14–18px pill |
| Labels | Mono uppercase `RESEARCH` | Italic serif *Study*, echoing the lockup |
| Accent | Same | Same |

## 6. Type, icons, motion

| Area | Decision |
| --- | --- |
| UI font | IBM Plex Sans or Inter (open decision) |
| Mono | IBM Plex Mono or Lilex, for metadata, paths, costs, and the CLI |
| Serif | Instrument Serif italic for *Study* labels and headings, matching the lockup |
| Sizes | 12 / 13 / 15 / 20 px, editor 16px, line height 1.5 |
| Icons | Lucide at 1.5px stroke, 16px, plus about 12 custom glyphs (tern mark, Research, Study, five verdicts, checkpoint, watcher, steer, branch-reply) |
| Borders | 1px; no shadows except popovers |
| Motion curve | One curve, `cubic-bezier(.32,.72,0,1)`: 200ms for UI, 700ms for the mark |
| Motion rule | Moves only on a real event: a step starts, a source lands, a claim is verified, the run needs you. Respects reduced motion; never blocks input |
| Texture | Dither only on the launch card, empty states, and Scout |

## 7. Scout

Scout is the mascot: a small hooded scholar with a lantern and a scroll. It must feel alive, not like a handful of static images. It is built as a procedural rig (hood, eyes, lantern, scroll, with springs on the lantern and blink timers) that reacts to real event rates, drawn in line and dither in the text colour.

| State (priority order) | Triggered by | Behaviour |
| --- | --- | --- |
| Needs you | Plan approval, permission gate, question | Turns to face the user, lantern raised, holds |
| Blocked | Error, budget limit | Sits, scroll closed |
| Searching | Search events | Walks; lantern sweeps once per query |
| Reading | Fetch and PDF parse events | Scroll open; eyes track down the page as chunks stream |
| Verifying | Claim checks | Taps the scroll once per verdict |
| Ready | Run complete | Small nod, then idle |
| Idle | Nothing running | Breathing, random blinks every 3–7s, glances at the cursor when near |
| Study | Quiz or flashcard mode | Holds up a card; reacts to right and wrong |

A no-character mini mode and a still frame under reduced motion are required.

## 8. Diagrams

| Style | Use |
| --- | --- |
| Line-only, pointer-reactive figures | Live research map in the right rail's Graph tab: the flow lights up branch by branch as phases run, with true numbers in a corner readout |
| Solid isometric machines | Onboarding and docs art: research as a bench (funnel, sieves, magnifier, balance, press); study as a card box with drawers |

## 9. Adopt and avoid

| Adopt | Why |
| --- | --- |
| Summary / Detailed / Raw toggle for a run | One stream, two audiences |
| Inline approve, revise, deny on plan steps | Review before spend |
| Exact-quote evidence on hover, previous and next through quotes | Trust in citations |
| Reasoning timeline: icon, step title, one sentence | Readable progress |
| Steering while running, queued messages as `↳` rows | Control without restarting |
| Side chat that reads the thread but writes nothing back | Quick questions without clutter |
| Thread as a tree; edits fork instead of overwrite | Nothing is lost |
| Activity log where nothing vanishes silently | Auditability |
| Breadcrumb dropdowns in the title bar | Dense, keyboard-friendly navigation |
| 3-segment meters for effort and autonomy | Clear without numbers |
| Tick strip along the transcript for checkpoints | Fast navigation |

| Avoid | Why |
| --- | --- |
| Gradient logos and frames | Reads as generic AI |
| A card around every tool call | Noise |
| Rotating witty status text | Filler |
| Badge chains, avatars on every row | Noise |
| Multicolour tiles and verdict meters | Breaks the one-accent rule |
| Always-on credit counters | Anxiety; cost belongs in the status bar |
| Permanent TUI sidebar | Wastes columns at 80×24 |
| Emoji, sparkles, robot heads, SaaS blue, violet | Slop markers |

## 10. Open decisions

1. Palette: C (lichen), A (brass), or B (ink).
2. Fonts: IBM Plex throughout, or Inter with Plex Mono.
3. Map the author's hand sketches onto this layout, including usage limits, scaling, and level-selection screens.
4. Redraw the concept screens in the chosen palette before any UI code.
