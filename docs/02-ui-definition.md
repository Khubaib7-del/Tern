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

Revised after the owner's review of the v2 concept video, with the ChatGPT desktop app and Hoplite as structure references.

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ ← →  ◧  File Edit View Help                                    ⇪  ⧉  ◨      │ title bar 36px
├───┬───────────────────┬──────────────────────────────────────┬───────────────┤
│ ⌂ │ Tern ▾      ⍾  ⌕  │          ┌─────────────────────┐     │ PLAN  SOURCES │
│ ▤ │ ✎ New session     │          │ Research │  Study   │     │ REPORT  GRAPH │
│ ◷ │ PINNED ▾          │          └─────────────────────┘     │ ───────────── │
│ ✎ │   Lit review      │  Chat · Report · Sources · +         │ 1 ✓ Scope     │
│ ⚙ │ PROJECTS          │                                      │ 2 ◌ Search    │
│ ⋯ │ ▸ FYP-thesis      │   user message                       │ 3   Verify    │
│   │     Lit review    │   Worked for 4m 12s · 14 sources  ›  │ 4   Write     │
│   │     ↳ RAG eval    │   Answer text with citations [1] [2] │               │
│   │ RECENTS           │  ┌────────────────────────────────┐  │               │
│   │                   │  │ Ask, or steer the run…         │  │               │
│   │ [Scout]           │  │ +  Approve plan ▾  model·Medium ◔ ↑│ │               │
│ ◉ │                   │  │ Project · Files · Sources       │  │               │
└───┴───────────────────┴──┴────────────────────────────────┴──┴───────────────┘
```

| Zone | Contents | Rules |
| --- | --- | --- |
| Title bar | Back, forward, sidebar toggle, File Edit View Help on the left; share/export, open in new window, right-rail toggle on the right | Same cluster as the ChatGPT desktop app on Windows |
| Icon rail (44px) | Home, Library, Scheduled, Editor, Customize, More; account avatar at the bottom | Lucide 16px, 1.5px stroke; active = filled tile behind the icon, no colour |
| Sidebar (260px, collapsible) | `Tern ▾` workspace switcher, notifications, search; New session; Pinned; Projects (folders → sessions → `↳` sub-sessions); Recents; Scout in the bottom corner | Status only on the row: spinner, "2m", or one ◆ when it needs you. Nothing is printed under Scout; its pose carries the run state |
| Harness toggle | A segmented pill, **Research │ Study**, centred at the top of the chat area | Moved out of the composer; mirrors ChatGPT's Chat │ Work toggle |
| Session tabs | Chat · Report · Sources · `+` (Graph, Editor, Browser, Subagents, Explain in 3D) | Tabs can split right or down |
| Centre | Transcript, max width 720px | User messages on a tinted band; agent prose unboxed; work folded to one line per turn ("Worked for 4m 12s · 14 sources") |
| Composer | `+`, permission chip, model · effort, context ring, mic, send; under-row chips Project · Files · Sources | Becomes "Steer" while running: Enter steers, Tab queues, Esc interrupts. See §2.2 for the responsive rules |
| Right rail (300px, only with content) | Plan · Sources · Report · Graph · Context | Plan approval, evidence quotes, verdicts, checkpoints, and budget live here |
| Account menu | Usage and budget (monthly %, reset time), Show Scout (toggle with a hotkey), Settings, Help, Log out | Opens from the avatar |
| Vault editor | Opened from the Editor rail item: vault tree in the sidebar, tabs and breadcrumb, editor, right pane with Backlinks, Outline, Local graph | Plain `.md` and `[[links]]`, so the vault opens in Obsidian too |

There is no separate terminal window in the desktop app and no sparkle or star icon anywhere. A popped-out side chat window uses the tern mark top-left.

### 2.1 Pages from the icon rail

| Page | Contents |
| --- | --- |
| Home | Empty state: "What should we research?" with the composer and a few starter cards |
| Library | Reports, Sources, Notes, Decks, Files, All; grid or list; `New ▾` (note, report, deck, folder, upload) |
| Scheduled | Topic watches, weekly paper digests, spaced-repetition reminders; suggestion cards when empty |
| Customize | Skills, MCP servers, Providers |
| Settings | General, Providers, Models, Decision model, Privacy, Integrations (Git and GitHub, MCP, Browser), Study, Appearance, Shortcuts, About. Scout sits beside each page title |

### 2.2 Responsive composer (acceptance test)

With both side panels open the composer content must never float out of its card.

| Centre width | Behaviour |
| --- | --- |
| ≥ 720px | Full chip labels |
| 520–719px | Chips collapse to icons with tooltips |
| < 520px | Secondary chips move into a `⋯` overflow; the right rail becomes an overlay sheet |

Every screen is checked at 1280, 1440, and 1920px wide with both side panels open.

### 2.3 Providers screen

Each provider is a row, not a bare name: a 36px rounded tile with the provider's monochrome logo, the name in text colour, and a muted status line underneath ("Not connected.", "Connected · 12 models", "Key invalid."). A 1px-bordered card holds the rows, separated by 1px rules.

| State | Right-hand control |
| --- | --- |
| Not connected | `Add` button (opens the key sheet: paste key, Test, models listed) |
| Connected | The standard toggle used elsewhere in Settings (enabled or disabled), plus a `⋯` menu (edit key, test, remove) |
| Local (Ollama, LM Studio, OpenAI-compatible) | `Add` with an endpoint field instead of a key |

No "Install" wording: providers are connected, not installed. Logos come from an MIT-licensed icon set (for example Lobe Icons) and are drawn in one colour. A refresh icon at the top re-probes all connected providers.

### 2.4 Model picker

From Hoplite: a search field ("Search all N models"), a provider column with model counts, a model column, and a hover card with neutral Speed and Intelligence bars and the context size. Favourites and keyboard hints sit in the footer. Effort is a menu from None to Max (None, Low, Medium, High, X-high, Max).

### 2.5 Context display

Context shows in two places from the same data.

| Place | Form |
| --- | --- |
| Sidebar | The context view the owner approved in the v2 video, shown in the session header |
| Composer | A 16px ring in the composer's chip row, no floating text. Click opens a popover: one bar split into System · Sources · Chat · Memory with token counts, the compaction threshold marked (default 60%), and `Compact now` |

### 2.6 Git and GitHub

Each project vault can be a git repository. Integrations › Git and GitHub sets the git name and email, the GitHub connection, and baseline templates (README, LICENSE, SECURITY, CONTRIBUTING). In a project: a branch chip in the title bar shows the branch and the count of changes; actions are commit (also a checkpoint), branch, push, and open a pull request.

### 2.7 Working line

While the agent works, the turn shows one live line: the tern mark on the left, then a status phrase whose text carries a soft shimmer (a light band sweeping left to right, about 1.6 s, text colour to muted and back), then elapsed time. It collapses into the rollup ("Worked for 4m 12s · 14 sources ›") when the turn ends.

| State | Mark animation (16px, one colour) | Phrase |
| --- | --- | --- |
| Thinking | Slow glide, wings level, a 2px bob every 2 s | "Thinking" |
| Planning | Wing lifts once per plan step written | "Planning 3 steps" |
| Searching | Quick wingbeats while queries are in flight | "Searching · 6 queries" |
| Reading | Gliding, head dips once per source fetched | "Reading 4 of 9 sources" |
| Verifying | Holds still; one beat per verdict | "Checking claims · 7 of 12" |
| Writing | Steady cruise, small beats in time with streamed tokens | "Writing the report" |
| Needs you | Stops and turns to face the user; shimmer stops | "Needs your approval" |
| Paused or blocked | Folds wings, mark dims to muted | "Paused at budget" |
| Done | One dive and the line folds into the rollup | — |

Animation is driven by real events, never by a timer alone. Under reduced motion the mark is static and the shimmer becomes a plain muted label.

### 2.8 Message actions

| Action | How |
| --- | --- |
| Reply to a message | Swipe a message right (as in WhatsApp) or use the hover Reply button; the composer shows a quoted strip with the message ID, removable with `×`. Swiping left works too. Works on streaming replies as well |
| Copy | Hover Copy on every message, and on every query block and code block the user sent; copies the raw Markdown |
| Branch | Hover `Branch ▾` → New sub-session from here, or Move to sub-session. Select several messages to branch them at once |
| Edit, regenerate, versions, pin, delete | As in the product spec |

Hover buttons are 16px Lucide icons in a 1px-bordered pill that appears at the message's top-right, muted until hovered.

### 2.9 Composer input

| Element | Behaviour |
| --- | --- |
| Attach (`+` or paperclip) | Pick files, images, folders, or `.zip` archives; drag and drop and paste work too. Attachments show as 1px-bordered chips above the input |
| Long paste | Text above about 4,000 characters (setting) is turned into a `.txt` attachment chip ("Pasted text · 312 lines") instead of filling the box |
| Role of an attachment | Each chip has a role: **Context** (read as material, the default) or **Prompt** (treated as instructions). Type `@` to reference an attachment, file, or source; mark one as a prompt with `@prompt:name` or from the chip menu |
| Mic | Shown but locked until a speech-to-text model is configured. Hover: "Voice input needs a transcription model. Set one in Settings › Voice." |
| Steering | While a run is going: Enter steers, Tab queues, Esc interrupts |

Settings › Voice: choose a local transcription model or a transcription API key. A first-party Tern transcription model is planned for later.

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

Every source and service row shows its logo, the way ChatGPT shows Firecrawl or GitHub beside a step: a built-in glyph for services Tern knows (GitHub, arXiv, Wikipedia, DOI, Semantic Scholar, PubMed, the search services, and Tern's own reader), otherwise the site's favicon, fetched through the network guard and cached. The TUI shows the site name instead. See ADR 0005.

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

| Token | A · Graphite & Bone | B · Ink & Paper | **C · Mono & Lichen (chosen, ADR 0002)** |
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
| UI font | IBM Plex Sans (ADR 0002) |
| Mono | IBM Plex Mono, for metadata, paths, costs, and the CLI |
| Serif | Instrument Serif italic for *Study* labels and headings, matching the lockup |
| Sizes | 12 / 13 / 15 / 20 px, editor 16px, line height 1.5 |
| Icons | Lucide at 1.5px stroke, 16px, plus about 12 custom glyphs (tern mark, Research, Study, five verdicts, checkpoint, watcher, steer, branch-reply), a `tern-reader` glyph for Tern's own reader, and built-in logos for common research sources and search services |
| Borders | 1px; no shadows except popovers |
| Motion curve | One curve, `cubic-bezier(.32,.72,0,1)`: 200ms for UI, 700ms for the mark |
| Motion rule | Moves only on a real event: a step starts, a source lands, a claim is verified, the run needs you. Respects reduced motion; never blocks input |
| Texture | Dither only on the launch card, empty states, the README hero, and Scout |
| Smoothness | The v2 concept video is the benchmark. Panels slide and fade together on the one curve; nothing jumps or reflows mid-animation; 60 fps at 1080p on mid-range Windows hardware |
| Theme switch | Light and dark cross over as a soft circular wipe from the toggle, about 600 ms, with every token interpolated at once. The owner's favourite moment in the video; keep it exactly this calm |

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
| **Explain in 3D** (separate mode) | Its own session tab, separate from the Graph tab. Takes a concept from the session and builds an interactive isometric explainer: an Anatomy-style solid machine with a Hairline line-figure overlay, pointer-reactive, step-through, with captions tied to cited sources. Saved to the Library; also the base of the Study playground |

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
| Always-on credit counters | Anxiety; cost belongs in the context popover and the account menu |
| Permanent TUI sidebar | Wastes columns at 80×24 |
| Emoji, sparkles, robot heads, SaaS blue, violet | Slop markers |
| A status line printed under Scout | The pose already says it |
| "Install" buttons for providers | Providers are connected with a key, not installed |

## 10. Open decisions

1. Palette: C (lichen), A (brass), or B (ink).
2. Fonts: IBM Plex throughout, or Inter with Plex Mono.
3. Hand sketches (usage limits, scaling, level selection) are parked by the owner.
