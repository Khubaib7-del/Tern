<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/banner-light.webp">
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner.webp">
    <img alt="Tern: the diving-tern mark in moving dither beside the TERN Research · Study wordmark" src="docs/assets/banner.gif" width="100%">
  </picture>
</p>

<p align="center"><b>A local-first agent harness for research and study, with every claim cited and checked.</b></p>

<p align="center">Pre-alpha · private · not yet released</p>

---

Tern turns a rough question into a verified, cited report, and turns what you read into something you can learn from. It runs on your machine, with your own API keys or local models, and shows you what it is doing and what leaves your computer. It is being built as the research tool for its author's final-year project, on Windows, Linux and macOS.

<table>
  <tr>
    <td width="50%"><img src="docs/assets/screenshots/research-answer-and-rail.png" alt="A deep research answer with verdict marks on each cited sentence, the sources read, and the right rail on Plan › Workers"></td>
    <td width="50%"><img src="docs/assets/screenshots/empty-state.png" alt="The empty state: the dithered Tern mark, Start a research session, and three suggestion chips; Scout at the bottom left"></td>
  </tr>
  <tr>
    <td><img src="docs/assets/screenshots/study-flashcards.png" alt="Study: the flashcard stack with the Study rail showing what is due and the decks"></td>
    <td><img src="docs/assets/screenshots/settings-scout.png" alt="Settings › Scout: a live preview of Scout, show or hide, the trim, and every state"></td>
  </tr>
</table>

<sub>Frames from the scripted UI tour (`TERN_TOUR=1`) recorded by CI on Linux; demo data, no model calls.</sub>

## Highlights

**Research**
- **Cited answers that are checked.** Every cited sentence is checked against the text Tern read from its source: Supported, Partly supported, Not supported or No source, with the quoted passage. Each answer writes a Markdown and JSON run report.
- **Deep research.** You approve a plan first. Parallel workers then research the sub-questions, each with its own budget, and one answer is written and checked from their notes. You can steer while it runs ("focus on 2024 sources", "drop 3"), and it resumes from checkpoints after a crash or a stop.
- **Tern Crawl**, Tern's own crawler and reader: a network guard, robots.txt, per-site pacing, a page cache and PDFs read page by page. Firecrawl, Jina Reader or a plain fetch can be the reader instead.
- **Right rail**: **Plan** (Summary, Workers, Detailed, Decisions), **Sources (n)** (Ledger, Raw events), **Report (n)** (claim verdicts), **Graph** (the run map and spend over time) and **Context** (what fills the context window). It opens by itself once a run has something to show.

**Study**
- Add your materials (PDFs, notes, folders, vault notes, reports, links) and Tern splits them into topics with a mastery score per topic.
- **Tutor** (Explain, Socratic, Explain like I'm new), **quizzes** graded on one click with a progress bar, **viva practice** with a rubric report, and **flashcards** on SM-2 in a stack you swipe (right for Good, left for Again). Decks export as real Anki packages.
- A **Study rail** with your decks, what is due and recall.

**Explain in 3D**
- Turns a concept or a whole answer into a small isometric figure in five levels, from the basics to the advanced parts. Each node has its explanation, the passage it came from and its citations. A real camera lets you orbit, zoom and pan, and Prev, Next and Play walk the levels. It is in Research and Study alike.

**Workspace**
- **Built-in browser** beside the chat. The real site runs in a separate helper process, so a page can never freeze Tern. **Explain with citations** asks Tern about the page you are on.
- **Chat tabs** per harness, and **session restore**: the window reopens the way you left it (harness, tabs, Study tab, rail view, sidebar).
- **Timeline rail**: one tick per question at the chat's edge. Hover a tick to read the question, click it to glide there.
- **Live dictation**: the mic transcribes while you speak, phrase by phrase.
- **Command palette**: Ctrl+K (Cmd+K on macOS).
- **Continuity**: message IDs (`Q12.3`), cleaning a chat by ID range, sub-sessions, handoff packets, the Library, highlight actions and the side chat.
- **Extensibility**: MCP servers (tools, prompts, resources) behind a trust preview, skills, `TERN.md` rules, project folders, and `tern acp` for editors such as Zed.
- **Evaluation**: a five-rung test ladder with versioned suites, three trials, 95% intervals and published failures; CI runs the offline suites.

**Feel**
- **Scout**, a coloured clay sprite, floats over the window. Its ten states follow what Tern is doing (thinking, searching, reading, verifying, needs you, done, blocked, study, sleeping), with spring crossfades and a speech bubble. Drag it anywhere, hover it for New chat and Dictate, and hide it with Ctrl+. (Cmd+.). **Settings › Scout** previews every state and picks the trim.
- **Spring motion** throughout: sliding panels, the rail marker, tab underlines, segmented controls and toasts all move on springs, and buttons jiggle when pressed. Reduced motion is honoured (the OS setting on Linux and macOS, or `TERN_REDUCED_MOTION=1`).
- **Onboarding**: a first-run tour (provider, a key tested live, model, search, theme) that you can skip and rerun from Settings › About. The empty state offers three suggestions per harness.

**Reliability**
- **Explain in 3D no longer closes the app (#57).** Tern used to ask GPUI for the next frame from inside event listeners. Every frame request now goes through a safe scheduler (`view::frame::next_frame`), and a test fails if the bad pattern comes back.
- **Crash logs on Windows**: a crash that no panic hook sees is written, with its exit code and the last action, to `%LOCALAPPDATA%\Tern\data\logs\` (`crash.log`, `tern.log`, `stderr.log`, `last-action.txt`).
- **Demo mode**: `TERN_TOUR=1` runs a scripted tour of every screen with demo data in a throwaway folder (no keys, no model calls). The **UI tour** CI workflow records it on Linux and uploads the video.

## Install and run

There are no releases or installers yet. Test builds come from CI on each pull request (the Actions run's artifacts):

| OS | Artifact | Run |
| --- | --- | --- |
| Windows 10 and 11 (x64) | `tern-windows-x64` | Unzip and run `tern-desktop.exe` for the app, or `tern.exe` in a terminal. Logs and crash reports are in `%LOCALAPPDATA%\Tern\data\logs` |
| Linux (x64) | `tern-linux-x64` | `tar -xzf tern-linux-x64.tar.gz`, then `./tern-linux-x64/tern-desktop` or `./tern-linux-x64/tern` ([docs/linux.md](docs/linux.md)). It needs a Vulkan driver |
| macOS (Apple silicon) | `tern-macos-arm64` | Unzip, then right-click `Tern.app` and choose Open the first time (it is unsigned). `./tern` in a terminal ([docs/macos.md](docs/macos.md)) |

From source: `cargo run -p tern-desktop` for the app, or `cargo run -p tern-tui` for the terminal (see [Building from source](#building-from-source)). On first launch the tour asks for a provider and a key. Wikipedia search works with no key.

## What works today

| Area | Working now |
| --- | --- |
| Core | Stable IDs, versioned events, turn state machine, secret redaction, append-only SQLite event log with replay, `tern doctor` |
| Providers | 13 providers (11 remote, Ollama and LM Studio local) on Tern's own adapters, plus **custom providers**: any OpenAI-compatible or Anthropic-compatible server (vLLM, llama.cpp, LiteLLM, a company proxy) added in Settings › Providers with a name, base URL, optional key and headers, or by pasting its config (a single provider, a `models.json`, or an opencode provider map; every problem is listed before anything is saved), tested before it is kept and deletable from its row; keys tested before they are saved to the OS keychain (Windows Credential Manager, the Linux Secret Service, or the macOS Keychain); streaming replies |
| Model catalogue | Every model a key can use, every page of the provider's list (Anthropic, Gemini, OpenRouter with every output modality), nothing filtered out; failed calls show the provider's own message. Each model's capabilities (chat, vision, tools, reasoning, speech to text, text to speech, OCR, image, embeddings, safety, rerank) come from the provider's metadata first (Mistral capability flags, Anthropic capabilities, OpenRouter and xAI modalities, Gemini generation methods, Ollama capabilities, LM Studio types) and Tern's pattern table second. The model picker groups them (Chat, Vision, Speech, OCR, Image, Embeddings, Safety, Other) with badges and provider names ("Mistral Small 4" for `mistral-small-2603`), folds `-latest` aliases into the dated model, and hides deprecated models behind **Show retired**. A model that cannot chat never becomes the chat model: picking a speech-to-text model makes it the mic's model (marked with a mic in the list), an OCR model the reader for documents, and a toast says so. Picking a model from another provider connects it and swaps it in place; a pick made while an answer runs takes over when it finishes. Each provider's listing is kept for 10 minutes, so switching tabs in the picker is instant, and a listing that fails or takes over 10 s shows why with **Retry**. `tern provider models <id> [--all]`. See [docs/models.md](docs/models.md) |
| Voice and pictures routing | The mic records (cpal) and transcribes **while you speak**: speech is cut into phrases at pauses (or every 12 s), each phrase is sent to the speech-to-text model as it grows, the phrase being spoken shows under the composer text in grey with a trailing "…", and each finished phrase joins the composer text; stopping sends the last bit and commits it. While the mic listens, the model chip names the voice model (for example whisper-large-v3 · Groq); with no provider that lists one, a toast says "No voice model available — add Groq, OpenAI or Mistral" and the mic stays off. Attached audio files (the **+** menu's **Audio**, the file picker, or drag and drop: M4A, MP3, WAV, Ogg, FLAC) show a chip with the file's length while they are transcribed, by a local Whisper model when one is installed, else the provider's speech-to-text model, and the transcript rides with the next message; `.opus` says it is not supported yet, and with no transcription model a toast opens **Settings › Local models**. A picture attached for a chat model that cannot see is described by a vision model, or read by Mistral OCR, and its text passed on as untrusted data with a notice; when nothing can read it, the notice says what is missing. Per-role defaults `default_stt_model`, `default_vision_model`, `default_ocr_model` in `preferences.json` (set from the picker; automatic otherwise) |
| Turn loop | Bounded by model calls, tool calls, and tokens; loop detection; cancel; an empty reply is asked for once more and never kept in the chat's history (providers refuse every later request that carries an empty assistant message); policy gate with Allow, Ask, Deny and inline approvals. The gate tracks where web addresses came from: after a page or an MCP result enters the chat, a read of a domain that neither you nor search introduced asks first, and an address carrying a key- or session-shaped value asks, or is refused when it goes to such a domain |
| Web answers | `tern ask` and chat search through Tavily, Brave, SearXNG (your own server, with its address and a Test button), or **Wikipedia**, the keyless default that needs no key and no server, read pages behind a network guard, and cite them with a citation check. A search that fails says why (connection refused, SearXNG's JSON output off, a rejected key, a rate limit) instead of "found nothing". SearXNG chosen without a server address counts as not set up ("Not set up" on its row) and searches fall back to Wikipedia; when the chosen service is down, a notice offers **Switch to Wikipedia** or **Set address**. When no other service is set, queries go to the Wikimedia Foundation's API; `"search": "off"` in `preferences.json` stops that. Setup: [docs/search.md](docs/search.md) |
| Crawler | **Tern Crawl**, Tern's own crawler and reader, with its own colour logo on every "Read N pages" row: network guard, robots.txt, per-site pacing, a day-long page cache, Readability-style main text, and PDFs read page by page with `[page N]` markers. Settings › Search & Crawlers can make **Firecrawl** (hosted, renders JavaScript, needs a key), **Jina Reader** (hosted, works without a key at a low rate), or a **plain fetch** the default reader instead, each with its logo, a key that is tested before it is saved, a Test button, and an on/off switch; a hosted reader that fails falls back to Tern Crawl and the run says so |
| PDFs and pictures | Attach a PDF (up to 32 MB) and its text is read page by page; attach, drop, or paste (Ctrl/Cmd+V) a PNG, JPEG, GIF, or WebP (up to 5 MB) and it goes to the model as a picture (OpenAI-compatible, Anthropic, and Gemini formats), staying in the chat for follow-ups. Waiting pictures show a thumbnail in the composer. Pictures are converted to PNG or JPEG within 2048 px and 5 MB first (WebP, GIF, huge pictures; HEIC is refused with a reason). For a chat model that cannot see, readers are tried in order, each failure falling through: a local vision model, the local OCR model, a provider's vision model, a provider's OCR model; an info toast names the one that read them. When none can, the message goes with a note instead of failing, and a chat model that refuses pictures is remembered as not seeing and the turn reruns through the readers |
| Local models | Downloaded on demand, never bundled: Tern's approved list of 15 models, a Default and an Alternative per task (OCR: PP-OCRv6 small + Arabic-script recogniser, Tesseract; documents to Markdown: PaddleOCR layout + tables, Granite-Docling, PP-FormulaNet+ for maths on demand; describe pictures: SmolVLM 500M, Qwen3-VL 2B; live dictation: Moonshine Streaming, Parakeet TDT v3; audio files: Whisper small, Whisper large-v3 turbo; read aloud: Kokoro 82M, Supertonic 3; embeddings: Granite Embedding R2, EmbeddingGemma 2), each with its pinned Hugging Face files, sizes, SHA-256, licence and licence notes; resumable downloads with progress, cancel and hash checks into `<data>/models/<id>/`; Enable, **Use for this task**, **Use the alternative** when one fails, delete. **Settings › Local models** (with each maker's logo) and `tern models list\|download\|remove\|default\|enable\|read`. **OCR runs locally** (PP-OCRv6 + the Arabic-script recogniser on ONNX Runtime via `oar-ocr`, English and Urdu): it reads pictures for chat models that cannot see, and `tern models read <picture>`. **Whisper runs locally** (whisper.cpp, CPU) for attached audio files and `tern models transcribe <file>`, and **Moonshine Streaming runs locally** (ONNX Runtime) for the mic as you speak. **The other models download and verify but do not run yet** (describing pictures and read aloud are next). See [docs/local-models.md](docs/local-models.md) |
| Claim checks | Every cited sentence of an answer is checked against the text Tern Crawl read for its source: Supported, Partly supported, Not supported, or No source, with the quoted passage and a link to it. The chat's model judges; an offline check fills gaps: word overlap, then a look for a contradiction in the best passage (a different number in the same place, a negation, an antonym, swapped sides of a comparison, another name where the claim has its own), which makes the claim Not supported |
| Deep research | A plan first: the model splits the question into sub-questions (searches, expected source types, budget share) and you run, edit, skip, or cancel it before anything is searched. Workers then research the parts in parallel (3 at once by default), each with its own slice of tokens, pages, and time, reading through Tern Crawl; one answer is written from their notes with one set of citations, then checked and reported like any answer. One test search runs before the plan (also when the question waits for your approval), so a search service that is down stops the run with its real error before any model call is spent; the failure line names each worker's stop reason. `tern research --deep "question"` in the terminal; the composer's **Deep research** mode on desktop |
| Run reports | Each answer writes a Markdown and JSON report (question, steps, sources with robots and cache status, claim verdicts, budget and tokens, errors, and a section per worker for deep research); `tern report` prints or exports one |
| Resume | Research runs save a checkpoint at safe points (each tool result; for deep research the approved plan, each worker's reads and notes, each worker's end, and before the answer). A run that crashed, was force-quit, or was stopped picks up where it was, without repeating finished workers or reading their pages again: `tern resume [chat]`, or **Resume** on desktop. A run that failed offers **Retry failed workers** instead (only the failed workers run again; finished ones are kept), with the cause, progress counted from the workers ("2 of 5 done · 3 failed"), and local time. A run with nothing left to do offers nothing |
| Steering | Send a note while a run goes: "focus on 2024 sources" guides the rest of the run, "drop sub-question 3", "add: cost comparison", and "redirect 2: …" change a deep run's workers. Notes land at the next safe point and the log says what each did. Type during `tern research`, use `tern steer <chat> "…"` from another terminal, or press Enter in the desktop composer while a run goes |
| Research flow (v2) | The run reads as the v2 concept has it, step by step. **Steer**: notes you send while it runs sit over the composer as `↳` rows, "queued · next safe point" until the run applies them, then "applied" or "not applied". **Needs you**: a request to use a tool is one card, "◆ Tern needs you", with numbered answers (Allow once, Allow for this chat, No and tell Tern what to do); press 1, 2 or 3 while the composer is empty. **Checkpoints**: a strip of ticks on the transcript's left edge, one per safe point the run passed (the approved plan and each finished worker in deep research, each tool result in a quick turn); hover one for its pages, tokens and time, and the latest one restores the run when it stopped short. **Budget**: a run that hits its token limit pauses at a safe point and shows "Paused at a safe point" with what it used; raise the limit by half or double it and it resumes from the checkpoint (the chat's limits and a deep run's plan and worker budgets are raised together), or stop. **Citations**: the sources an answer cites sit under it as chips; hover one for the quoted passage, its page, the claim and its verdict, step through every passage quoted from that source with ‹ ›, and open it in Tern's browser. **Review**: the Report tab's claims can be kept all at once or reviewed one by one (keep, reject, comment); rejected claims are struck through, the review is saved beside the report, a toast says what changed, and your comments go to the composer as the next message for Tern's next pass. **Export**: the report with your review as Markdown, or as a printable HTML page to print to PDF, written beside the report and opened. **Vault diffs**: a note that changed on disk under the Editor (the agent's vault tools or another app) or an answer saved onto a note that already exists shows as a line diff with **Accept** or **Reject** (rejecting a change already on disk writes your text back); the Editor never overwrites a note that changed under it. On launch the Tern mark settles in the middle of the window and flies to its place in the title bar (not with reduced motion); the title bar shows the chat's project and title and a search button for the command palette; the right rail's tabs are underlined. Cost is shown in tokens: Tern has no price table yet, so there is no dollar budget |
| Watcher | Put a question on watch (daily, weekly, or any interval). The desktop app re-runs due watches while it is open, and `tern watch run` does from the terminal; each run is diffed against the last report: new sources, changed verdicts, new and dropped claims. There is no operating-system scheduler, so nothing runs while Tern is closed. `tern watch add\|list\|rm\|run` |
| Ledgers | Each run keeps a source ledger (every URL considered, read live or from the cache, or skipped for robots.txt, the budget, a duplicate, or the policy gate) and a budget ledger (tokens, pages, searches, and time per step and per worker), in the run report, `tern ledger`, and the desktop run panel's **Ledger** view |
| Study | Add your materials (PDFs read page by page, notes, folders, vault notes, a research report, links) and Tern splits them into topics with learning objectives and page anchors, and keeps a mastery score per topic (a rating moved by each graded answer, a Leitner box for when it is due, both explained in words). **Tutor**: Explain, Socratic, or Explain like I'm new, citing the pages it teaches from and flagging anything beyond your materials. **Quizzes**: multiple choice, true or false, fill in the blank, and short answer, with an answer key and source page per question; keyed answers are graded exactly, short answers by the model with a word-overlap fallback, in the verdict glyphs; results move mastery and are kept in a history. **Viva practice**: an examiner asks harder questions level by level, follows up on thin answers, and ends with a rubric report (clarity, correctness, depth, use of sources) saved as Markdown. Everything works offline from your materials when no model is set. `tern study add\|list\|rm\|topics\|history`, `tern tutor`, `tern quiz`, `tern viva`; the **Study** harness on desktop |
| Flashcards | Cards written from your materials per topic (basic, cloze `{{c1::…}}`, and "explain why"), each with the page or section it came from; by the model, or offline from definitions, key sentences, and stated reasons. Repeats are skipped; you can edit, suspend, and delete cards. Reviews run on SM-2, the scheduler Anki uses (Again, Hard, Good, Easy; learning steps of 1 and 10 minutes; ease and interval per card), with a daily queue (learning, then reviews due today, then up to 20 new cards), deck statistics, and each answer nudging its topic's mastery. Export a real Anki package (`.apkg`: SQLite collection and media, "Tern Basic" and "Tern Cloze" note types, one deck, tags per topic, the source in its own field, and review cards keeping their schedule), CSV, or Anki-ready TSV, and import CSV or TSV back. `tern cards gen\|review\|list\|stats\|edit\|suspend\|rm\|export\|import`; **Flashcards** on the desktop Study home, with a review screen (the card turns over on the motion curve, keys 1 to 4, Scout holds up a card and reacts) and a deck screen |
| Explain in 3D | A concept from your materials (a topic, or one you name) built as a small isometric machine, or **a whole chat answer mapped from the basics to the advanced parts**: 4 to 20 nodes in five levels (Foundations, Core idea, Mechanism, Details and edge cases, Advanced and implications), sized to the answer, with key-term sub-nodes, "builds on" links, and on every node a short explanation and the passage of the answer it comes from, citations kept. The whole answer is read (a long one is mapped in chunks and merged; nothing is cut off); the model writes the map when there is one and Tern maps it itself otherwise. Solid shaded faces with a hairline overlay in the monochrome palette and the one accent. On desktop a real camera: drag to orbit all the way round (the tilt is clamped), the wheel or a pinch to zoom about the pointer, right- or Shift-drag to pan, double-click a node to focus it, **Reset view**, arrows and + / − on the keyboard, with a damped spin after a drag; reduced motion jumps instead and never spins on. **Prev / Next / Play** walk the levels from the basics up, the camera gliding onto each step's nodes, and a side caption reads the step or the node you click: its level, explanation, passage and citations. The figure is framed to fit the stage on load and on Reset, an answer's levels read as rows across the screen (basics at the front), and labels never overlap: the step's nodes show their full label, nearby nodes theirs when there is room, the rest their step number, with a leader line when a label had to move; label size follows the zoom within limits, and the camera keys sit behind a **?**. Only the step's links are drawn in full, links between consecutive steps lightly, the rest nearly invisible. Labels are laid out once per step and once the camera settles, and ride with their nodes while it moves, so walking the steps and zooming stay smooth. The scene is built in the background with **Cancel**, the loading line saying where the build is and for how long; a slow model times out and Tern maps the answer itself, small models' near-JSON (code fences, single quotes, trailing text) is read, and a failed build shows an error card with **Try again** instead of taking the app down. Asked from a quiz question, it maps the last answer that explains something instead. **Explain in 3D** sits in the visible action row under every answer in Research (a tab of its own) and Study (the Explain tab), one renderer for both. Saved to the study library as JSON with an SVG snapshot, or to the vault as a note. `tern explain3d "concept" --svg FILE` |
| Decision model | A fast scorer beside the chat model for fixed choices, with hard checks that always override it. It ranks search results before Tern reads them (quick asks, chat searches, and every deep worker), hints the first tool for a chat message, pre-scores each claim before the model judge (which keeps the final word), starts deep workers and due watches most pressing first, and logs every decision with its model, probabilities, latency, and cost. **Local** (the default) is Tern's own offline scorer: word statistics and a few rules, free and deterministic. **Remote** is bring-your-own-key: TypeSafe **Jev** or Subquadratic **SubQ Snap** (the same System One API, every decision point) or **Voyage** rerank (source ranking and now-or-later), with a 4 s timeout and the local scorer as fallback. Each job (tool choice, page order, passage support) has its own switch, and the Settings page shows the last run's decisions. `tern decide "question" --choices a,b,c [--context FILE]`, `tern decide mode`, `tern decide key add`, `tern decide bench`; the desktop **Decision model** page and the run panel's **Decisions** view. Offline benchmark on the bundled 50 fixtures: fixed order 52%, local scorer 86% (in-sample; see [ADR 0007](docs/decisions/0007-decision-model.md)) |
| Continuity | Every turn has a stable ID (`Q12`), and sub-sessions hung under a message add a sub-ID (`Q12.3`) that resolves in `@` mentions, the CLI, `tern://chat/<id>/Q12.3` links, and Library search. **Clean a chat by ID range**: remove `Q3-Q5` to the chat's trash (restorable), or move messages with the context before them to a new session or a sub-session, with the original kept until you confirm (or undo). **Sub-sessions**: branch from a message, plan one with a question, or run a background branch; when it finishes it posts a summary back to the parent, which the parent's model reads too. **Handoff packets**: goal, decisions, open questions, key sources and claims, a ledger snapshot, and next steps as Markdown and JSON, and a new session that starts from the packet alone. `tern range`, `tern sub`, `tern show`, `tern handoff`, `tern new --from-handoff` |
| Library | Reports, sources, notes, decks, files, Explain-in-3D scenes, and sessions in one index with search (titles, details, and message IDs), filters, sort, paging, bulk delete to a trash that restores, and empty-trash; quick with thousands of items. `tern library list\|search\|rm\|restore\|empty-trash`; the **Library** page on desktop |
| Highlights and side chat | Select text in an answer for **Explain**, **Verify claim** (checked against the pages that answer read), **Add to notes** (the vault's Highlights note), **Make flashcard** (a cloze on its key term), or **Ask in side chat**. The side chat reads the thread and writes nothing back. `tern side "question"`; docked beside the chat on desktop or popped out into its own window |
| MCP servers | Add MCP servers over stdio or Streamable HTTP (or import another app's config). Tern speaks the current spec (`2026-07-28`) and falls back to the older `initialize` handshake for servers still on `2025-11-25` back to `2024-11-05`. A new server stays off until you read its **trust preview**: the command, its environment, every tool and its description, and anything suspicious flagged (invisible characters, hidden-instruction tags, text that hides actions from you or reaches for secrets). Each tool has its own switch and approval (Ask, Allow, Deny) behind the policy gate, and its output reaches the model as untrusted data that cannot give instructions. **Prompts** appear in the composer's `/` menu: pick one, fill in its arguments, and the server's text lands in the composer under a banner naming the server, with anything suspicious flagged; nothing is sent until you send it. **Resources** attach with `@` as Context, wrapped as untrusted data like tool output (templates are filled in first). The trust preview and the effective instructions list prompts and resources too, and a server that changes them needs a fresh review. Health checks start, ping, and list each server and time each step. Settings › MCP & Skills has a **templates gallery** with logos (Filesystem, Fetch, GitHub, Notion, Context7, Firecrawl, Brave Search and more; fill in what each needs), an **Add server** form (a command with arguments and environment variables, or an HTTP URL with headers), and **Paste JSON** for Claude Desktop/Cursor `mcpServers`, VS Code `servers`, or Zed `context_servers` configs; every added server still waits for its trust preview. `tern mcp add\|import\|list\|preview\|enable\|disable\|health\|rm\|prompts\|prompt\|resources\|read`; the desktop **Settings › MCP & Skills** page with health dots |
| ACP (editors) | `tern acp` runs Tern as an agent inside editors that speak the Agent Client Protocol (v1), such as Zed: the editor's folder becomes the project (its `TERN.md` and skills apply), replies and tool steps stream into the editor, past chats load again, and approvals appear as the editor's permission prompt. Setup: [docs/zed.md](docs/zed.md) |
| Skills | Folders with a `SKILL.md` (yours in Tern's data folder, a project's in `.tern/skills/`). Only names and descriptions go into the prompt; the model loads a skill's full instructions when a task needs it. Install from a folder or a git URL after a preview with the same suspicious-text scan, switch on and off. On desktop, **New skill** opens a `SKILL.md` editor (name, description, instructions, checked before saving) with templates to start from, and **Import from GitHub** installs a skill folder from a GitHub URL. `tern skills list\|install\|enable\|disable\|show\|rm`; **Settings › MCP & Skills** |
| Rules and instructions | `TERN.md` files (yours, folders above the project, and the project's own) merged in order with a later file winning. **Effective instructions** shows exactly what the model is told, piece by piece. **Settings › Instructions** shows Tern's built-in Research and Study prompts read-only (with Copy) and takes your own instructions for every chat, for Research, and for Study, each with Reset to default; yours are added after Tern's own prompt, and a project's `TERN.md` still applies. `tern instructions`; **Settings › MCP & Skills → Instructions** |
| Project folders | Projects hold chats and their sub-sessions; a project can link a folder on disk so its `TERN.md` and skills apply to its chats. Deleting a project moves its chats to the Inbox. `tern project new\|list\|mv\|rename\|root\|rm`; the desktop sidebar's project tree (drag a chat onto a folder or Move to…, collapse saved) |
| Evaluation | A test ladder with one command per rung: **unit** (`cargo test --workspace`, read back as a run), **synthetic** (bundled suites run offline in under a second: claim verdicts, research answers replayed from recordings, quiz grading, decision routing on a held-out split, and injection and tool safety: poisoned MCP descriptions, injected page text, tool calls an injection could ask for, and canary pages for a live model), **pinned** (real questions over pinned public pages, answered live), **public** (SciFact, read from your copy), and **human** (timed scripted tasks and the System Usability Scale). Metrics include citation accuracy, claim precision and recall, unsupported claims per 100 runs, constraint retention, tokens or cost per verified report, preflight forecast error (live), verdict and grading accuracy, Brier score, and scan, containment, and gate rates, each over at least three trials with a 95% interval, and every failure listed. Runs are saved as JSON and Markdown and compared metric by metric; CI runs the offline suites against a baseline and uploads the report. Offline scores after PR #43: claim verdicts 100% on the original suite and 83.3% on a held-out one (59.3% and 41.7% before the contradiction checks), research checker agreement 95.5%, quiz grading 87.0%, held-out routing 72.7% for the local scorer against 45.5% for fixed order, safety 94.7% with every gate case right. CI runs them on Windows, Linux, and macOS. `tern eval list\|run\|report\|compare\|usability`; the desktop **Evaluation** page. See [docs/evaluation.md](docs/evaluation.md) |
| Toasts | Tern's short notes (the model changed, a voice model saved, a steer sent, errors) appear as toasts beside the chat, in the free gutter right of the message column (or narrowed above the composer when the window is too narrow), never over the messages, instead of lines in the chat; provider errors read in plain words: info toasts leave after 4 s, errors stay with **Dismiss** and, where it helps, **Retry**; they stack, fade in and out on the house curve (200 ms), and appear at once with reduced motion. Old chats no longer show stored error lines |
| Timeline rail | A thin rail at the chat's right edge with one tick per question (from two questions up, in new and old chats, Research and Study): hover a tick for the question's first 80 characters and when it was asked, click to glide to it; the questions on screen are lit |
| Composer | A **+** menu (files, images, PDFs, a folder, a link or DOI, an MCP resource, paste), a **mode** picker (Chat, Plan, Deep research, Act; Tutor, Quiz, Flashcards, Viva, and Explain in Study), a compact **model** popover (providers with their logos on the left, that provider's models on the right; search; switches in place), a separate **thinking effort** chip with its own meter glyph (None to Max, mapped onto each provider's own knob: OpenAI `reasoning_effort`, Anthropic thinking budgets, Gemini thinking budget or level, OpenRouter, xAI, Groq; it opens a slider popover with six stops over a row of warm-lit ticks in a rounded box, a slim glowing bar handle that follows a drag and snaps on release, click, arrow keys, and reset; one click or drag is one change and the popover stays open until Esc or a click outside; greyed out for models that do not think), a **context** popover (a segmented donut of how full the model's context window is, split into system prompt, tools, attachments, messages, and a compaction summary, with **Compact now** and **New chat with summary**; window sizes from the provider's model list or Tern's table of known models), a separate **search** chip, `@` and `/` buttons, **Pause** for a deep run, and Tab to queue a message while a run goes. Plan mode lets no tool run; Act mode allows tools that change things, each still approved |
| Thinking and working | Models that stream their reasoning show a **Thinking** section live, folding to "Thought for 6s ›" when the answer starts. Tool work folds into one rollup line inside the answer ("Searched 14 · read 6 · 3 verified ›") that opens to Summary, Detailed, or Raw. Answers are revealed at a steady reading pace by block (**Show all** skips ahead; reduced motion shows them at once), and any answer can be folded. Hover a message for when it was sent, an answer for when it started and how long it worked |
| Chat workspace | Open a folder or one file as the chat's workspace (title-bar button; a chip at the top of the chat). The agent's `files_list`, `file_read`, and `file_search` tools work inside it, and `file_write` in Act mode with approval; relative paths that leave it, including through symlinks, are refused. **Files by full path**: "read C:\Users\me\notes.txt", `/home/me/x.md`, `~/x.md`, quoted or not, are read by `file_read` with or without a workspace; a file outside the workspace (or with none open) is approved first. Typing or pasting `@` and a full path offers that file in the `@` menu to attach |
| Archive and delete | A sidebar row's **⋯** menu: Rename, Move to project, Open in new tab, Hand off, Archive, Delete (confirmed, with Undo). **Archived chats** lists archived chats (Unarchive, Delete) and the trash (Restore, Delete for good, Empty trash). Deleting writes a tombstone; erasing writes a permanent purge record before any event is removed, so the event log stays append-only for every other chat ([ADR 0009](docs/decisions/0009-deleting-chats.md)) |
| Panels | Drag the inner edges of the sidebar and the right pane to resize them (double-click resets; widths are saved), maximise the right pane or show the chat alone (title-bar button, Ctrl+\\ and Ctrl+Alt+\\), the right pane always fits the window (its header's views overflow into a **⋯** menu, and the pane floats over the chat when both cannot fit; tested at 1024, 1280, 1366, and 1536 px), segmented controls with a sliding highlight, tabs that do not fit go into a **▾** menu, a watch is removed with a **Remove** button and Undo, and the light/dark switch parts the old theme from the centre out |
| Context | A segmented donut gauge of the context window (system, tools, attachments, conversation, summary; green, amber from 70 %, red from 90 %), and automatic compaction past 80 %: older turns fold into a pinned summary written by the chat's model, the last turns stay word for word, and every turn stays in the log ([docs/context.md](docs/context.md)). **Compact now** and **New chat with summary** in the popover |
| Browse from the terminal | `tern browse <address or words>` prints a page as Tern Crawl reads it with Tern's explanation and numbered citations, or search results for words; `--open` opens the page in the `tern-browser` window |
| Terminal | `tern` / `tern chat`: an 80×24 ratatui chat |
| Platforms | Windows, Linux, and macOS, each built, tested, and evaluated by CI with a test build to download (see the table below). Keys in each OS's keychain, data in each OS's folders, the OS reduced-motion setting honoured on Linux and macOS, the macOS menu bar and Cmd keys |
| Desktop app | GPUI app for Windows (the executables carry the Tern icon; an 8 MB main-thread stack; a crash no panic hook sees, such as a stack overflow, is written with its exit code and the last action to `%LOCALAPPDATA%\Tern\data\logs\crash.log`), Linux (X11 and Wayland), and macOS: chat with streaming answers (8192 output tokens by default; an answer cut off at the length limit shows a notice with **Continue**), a labelled **Stop** and Esc while a turn runs, chats saved only once they have a first message (no empty "Chat" rows), the window reopened the way it was left (the harness, each harness's chat tabs with the one in front, the Study tab, the right panel's view, and the sidebar; a chat deleted since falls back to the last chat with messages), the last chat reopened on launch (its transcript shows at once while the model connects in the background; if the model cannot be reached the chat stays readable and you can pick another), the model changed in place without leaving the chat, chat list (reopen, rename, archive, delete, Archived chats), a **Settings** screen (Ctrl+, or the gear; sections with icons that fold to icons on narrow windows; search over every setting; Back or Esc returns: Providers with keys, server addresses, and Test; Models with the catalogue by capability and a default model per job; Tools with on/off and approvals; Decision model; Evaluation; MCP & Skills; Search & Crawler with Tern Crawl; Appearance; Study; Privacy & data; About), a first-run tour (provider, key tested live, model, search, theme; skippable, shown once, again from About), thin overlay scrollbars everywhere (a list inside a page scrolls first with the wheel or touchpad and hands on to the page at its ends), source rows with site icons, context ring, right rail with tabs Plan (Summary, Workers for deep research, Detailed, Decisions), Sources (Ledger, Raw), Report, **Graph** and Context; Graph is FIG 1 · RUN MAP, the run as a hairline isometric figure lit phase by phase (search, screen, read, synthesize, write, verify) with a readout of the phase, sources, kept pages and spend, hover for a phase's numbers and click it for its steps, and a spend-over-time chart against the budget with a crosshair tooltip), chat tabs with a **+** popover (new chat, or reopen a recent one), kept per harness and restored on launch, a Decision model page (Off, Local, or Remote; Jev, SubQ Snap, and Voyage rows with their logos and keys; a Test button), a plan card for deep research (Approve, Edit, Skip plan, Cancel), steering from the composer while a run goes, a Resume banner for a stopped run, a Watches list in the sidebar with a Watches screen showing what changed, verdict badges on cited claims, attachments (files, folders, nested `.zip`, long paste), `@` references (attachments, sources, and MCP resources, `@prompt:` / `@context:`), a `/` menu (Tern's commands, skills, and MCP prompts, every row with its logo; an inline form for a prompt's arguments; the server's text shown in the composer before anything is sent), message actions (copy, reply with a quote, branch into a new chat, `Branch ▾` for sub-sessions and background branches, ask in side chat), message IDs with multi-select and range actions (move to a sub-session or a new session, remove, confirm or undo, the chat's trash), a sidebar sub-session tree (`↳ Q12` rows, planned and running state) and a handoff action, a highlight popover on answer text, the side chat (docked or popped out with the tern mark), the Library page (Reports, Sources, Notes, Decks, Files, Explain in 3D, Sessions, All, and Trash; grid or list; `New ▾`; search, sort, bulk select, paging), project folders in the sidebar (Projects, chats, `↳` sub-sessions, the Inbox; drag to file, collapse saved), Settings › MCP & Skills (Skills, MCP servers with the trust preview and health dots, Instructions), Settings › Evaluation (runs with their subject's logo or the tern mark, per-suite scores with intervals and the change since the previous run, failures, and a button that runs the offline suites), session tabs, a responsive layout (panels float over the chat on narrow windows), the locked design (palette C Mono & Lichen, IBM Plex Sans and Mono, Instrument Serif wordmark, Tern glyphs, the providers' and search services' own coloured logos from Lobe Icons, and Settings › Appearance: theme, accent colour, font, text size, density, and reduce motion, saved in preferences) and Scout, a clay-sprite pet floating over the window that reacts to the run (Settings › Scout), Git for the vault (checkpoints, branches, pull/push, open a pull request on GitHub, baseline files, branch chip in the title bar), a built-in browser beside the chat (tabs, an address bar that takes an address or words, back, forward, reload, and a **Page | Explanation | Side by side** toggle; the page is the real site in the system web view (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux) running in its own helper process, `tern-browser`, docked over the pane, so a slow or crashing page can never freeze Tern: if the helper does not start within 5 s or stops, the pane says why, offers **Restart**, and shows Tern's reader view meanwhile ([ADR 0011](docs/decisions/0011-browser-helper-process.md)); the Explanation side shows the page as Tern Crawl reads it, or search results to open for words, and **Explain with citations** asks Tern about it; citations and source rows open there; [ADR 0010](docs/decisions/0010-browser-engine.md)), the Editor and vault (Markdown notes, `[[links]]` and backlinks, autosave, Save to vault, agent vault tools), and the motion pass (working line with the moving tern mark and shimmer, sliding panels, the soft light/dark switch), and the Study harness (one Study window with a sliding Chat | Flashcards | Quiz | Viva | Explain in 3D tab bar, the composer able to switch and start a mode ("quiz me on paging"), empty states with Add material, Upload PDF, Use the last report and Paste link; its own Study chat separate from the Research chat, each harness reopening its last chat, draft and panels from a switcher at the top of the sidebar, a Start here card when no materials are added, a Study home with materials and their icons, topics with mastery rings, Tutor, Quiz, Viva, and Flashcards with the due count; a tutor chat with mode chips; a quiz card with instant feedback and a score summary; a viva screen with a timer and the rubric report; a flashcard review and deck screen; Study settings), and Explain in 3D tabs |

### Platforms

| OS | Desktop app | CLI | Keys | Data | CI test build | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Windows 10 and 11 (x64) | GPUI (DirectX) | Yes | Credential Manager | `%LOCALAPPDATA%\Tern\data` | `tern-windows-x64`: `tern-desktop.exe`, `tern.exe` | The reduced-motion setting is not read yet; use `TERN_REDUCED_MOTION=1` |
| Linux (x64 in CI) | GPUI on X11 or Wayland (Vulkan) | Yes | Secret Service; `TERN_KEYRING=keyutils` for the kernel keyring | `~/.local/share/tern`, settings in `~/.config/tern`, cache in `~/.cache/tern` (XDG) | `tern-linux-x64`: `.tar.gz` with both programs, a `.desktop` entry, and the icon | [docs/linux.md](docs/linux.md) |
| macOS (Apple silicon in CI) | GPUI on Metal | Yes | Keychain | `~/Library/Application Support/Tern`, cache in `~/Library/Caches/Tern` | `tern-macos-arm64`: unsigned `Tern.app` and `tern`, zipped | First launch: right-click `Tern.app` and choose Open. [docs/macos.md](docs/macos.md) |

Linux and macOS are built, tested, and evaluated by CI on every pull request, but have not yet been tried by hand on a desktop. There are no releases or installers yet.

## Two harnesses, one core

| | Research | Study |
| --- | --- | --- |
| For | Finding out what is true and who said it | Learning it well enough to explain it |
| Produces | Cited reports, literature reviews, evidence tables, fact-check boards, comparisons | Tutoring, flashcard decks, quizzes, viva practice, study plans |
| Method | Plan → search → read → verify → write, with a run report | Explain → check understanding → practise → grade |
| Checked by | Claim verdicts against exact source spans: ■ supported · ✕ contradicted · ○ unverified · ⌀ unreachable · — waived | The same verdict vocabulary, applied to answers |
| Looks like | Cooler surface, square markers, mono labels | Warmer surface, round markers, italic serif labels |

Both share the same projects, library, vault, memory, providers, and permissions. Switch harness per session.

## Planned capabilities

| Area | Planned |
| --- | --- |
| Models | More OpenAI-compatible endpoints and capability probing (13 providers, local Ollama and LM Studio, and keychain storage are built) |
| Decision model | A small local cross-encoder for claim checks, and measured numbers for the chat model and the remote decision models on real runs (the local scorer, the Jev, SubQ Snap, and Voyage adapters, `tern decide bench`, and a held-out evaluation split are built) |
| Control | Safe pause at budget limits, drift detection, and a scheduler that runs watches while Tern is closed (loop detection, steering, run checkpoints, and the in-app watcher are built) |
| Memory | Stable IDs for files and sources, and a goal and decision ledger that survives compaction (message IDs, sub-IDs, handoff packets, and context compaction are built) |
| Extensibility | Watching single MCP resources, and MCP sampling, roots, and elicitation (MCP tools, prompts, and resources, the ACP server, skills, `TERN.md` rules, and project folders are built) |
| Workspace | The Study playground built on Explain in 3D (Explain in 3D and the built-in browser are built) |
| Vault | Agent edits shown as diffs (plain Markdown notes, `[[links]]`, backlinks, and Git for the vault are built) |
| Study | Study plans, and FSRS scheduling once a deck has enough reviews to fit it (flashcards with SM-2 and Anki export, and Explain in 3D, are built) |
| Library | A model-written handoff summary (chat ranges, sub-sessions, handoffs, the Library with bulk delete, highlights, the side chat, and project folders are built) |
| Evaluation | Live, pinned, and public scores run with a key and published, and more held-out cases (the ladder, the suites, the metrics, reports, comparison, the CI baseline, and a held-out claims suite are built) |
| Platforms | Windows' reduced-motion setting, hand testing on Linux desktops and Macs, arm64 Linux and Intel Mac builds in CI (Windows, Linux, and macOS builds are done) |

Full detail: [docs/01-product-spec.md](docs/01-product-spec.md).

## Principles

| # | Principle |
| --- | --- |
| 1 | **Evidence before claims.** A claim stays unverified until a source or check confirms it. |
| 2 | **Local-first.** Your vault, library, and logs live on your machine. |
| 3 | **Your keys, your models.** Remote or local, your choice; no silent provider switching. |
| 4 | **Show what leaves the machine.** Before data goes to a provider, Tern says what and to whom. |
| 5 | **The process folds away, the result stays open.** One quiet line per turn; the full log one click away. |
| 6 | **Calm by default.** Black and white, one restrained accent, motion only on real events. |
| 7 | **The user steers.** Plans first; pause, redirect, and resume at any point. |
| 8 | **One core, many surfaces.** The core owns state and security; UIs only render its events. |

## Architecture

A single Rust workspace. The headless core owns execution and security; the TUI and desktop app subscribe to its events.

| Crate | Role |
| --- | --- |
| `tern-core` | Shared types, stable IDs, versioned events, turn state machine, secret redaction |
| `tern-agent` | Turn engine, planner, tool registry, policy gate, workers, watcher, steering, checkpoints, budgets, message IDs, range cleaning, sub-sessions, handoff packets, Library sync, side chat, MCP client, ACP server, skills, `TERN.md` rules, evaluation runs on Tern's real pieces |
| `tern-providers` | BYOK remote providers and local models, capability probing, usage accounting, MCP's HTTP transport |
| `tern-decision` | The decision model: `score(context, question, choices)`, a local offline scorer, Jev, SubQ Snap, and Voyage adapters, hard checks, cache, decision log, benchmark fixtures |
| `tern-crawl` | Fetch, extract, and chunk web pages and PDFs, behind an SSRF and DNS-rebinding guard |
| `tern-store` | SQLite, append-only event log, sub-sessions, project folders, the Library index (search, filter, sort, paging, trash), Anki package export; full-text and vector indexes next |
| `tern-research` | Research pipeline, scholarly sources, claim verification, citations, run reports |
| `tern-eval` | Evaluation: versioned task suites, offline and live runners, metrics with 95% intervals, reports, run comparison, the CI baseline, the SciFact reader, and SUS scoring; no I/O or UI |
| `tern-study` | Materials and anchors, topics, mastery, tutor, quizzes and grading, viva practice, flashcards and SM-2 scheduling, CSV and TSV, Explain in 3D scenes and their projection; study plans next |
| `tern-tui` | The `tern` CLI and ratatui terminal UI |
| `tern-desktop` | The GPUI desktop app |
| `tern-platform` | Keychain, paths, and OS integration behind traits |

Details and dependency rules: [docs/03-architecture.md](docs/03-architecture.md). Founding decision: [ADR 0001](docs/decisions/0001-rust-gpui-windows-first.md).

## Platform order

| Order | Platform | Status |
| --- | --- | --- |
| 1 | Windows | Done |
| 2 | Linux | Built (PR #43), CI green; not yet tried by hand |
| 3 | macOS | Built (PR #43), CI green; not yet tried by hand |

Every OS difference sits behind `tern-platform`, so ports stay small.

## Roadmap

| Phase | Focus | Status |
| --- | --- | --- |
| 0 | Repository baseline | Done |
| 1 | Design lock: palette, fonts, concept screens, SVG mark | Done |
| 2 | Core skeleton, keychain, store, `tern doctor` | Done |
| 3 | Providers and the first cited answer in the TUI | Done |
| 4 | Crawler and claim verification | Mostly done |
| 5 | Deep research: plans, workers, checkpoints, steering | Mostly done |
| 6 | Desktop app | In progress (research flow v2 in #59) |
| 7 | Study harness | Mostly done |
| 8 | Decision model | Mostly done |
| 9 | Continuity and library | Mostly done |
| 10 | Extensibility: MCP, ACP, skills | Done |
| 11 | Evaluation | Mostly done |
| 12 | Linux and macOS | Mostly done |

Next pull requests:

| PR | Planned |
| --- | --- |
| #60 | Images, OCR and voice, and local models |
| #61 | Video |
| #62 to #64 | The review buffer |
| #65 | Local RAG over your files |
| #66 | Context, memory and cost |
| #67 | Learning loops |
| #68 | Packaging and the Microsoft Store |

Phase detail and exit checks: [docs/04-roadmap.md](docs/04-roadmap.md).

## Repository layout

```text
.
├── .github/              issue forms, PR template, code owners
├── assets/
│   └── brand/            mark (PNG and SVG), lockups, app icon, animated dither hero
├── crates/               the Rust workspace (12 crates, see Architecture)
├── docs/
│   ├── assets/           README banner (animated WebP, GIF fallback, light variant) and screenshots
│   ├── 00-vision.md
│   ├── 01-product-spec.md
│   ├── 02-ui-definition.md
│   ├── 03-architecture.md
│   ├── 04-roadmap.md
│   ├── 05-security-model.md
│   ├── evaluation.md     the test ladder, suites, metrics, and how to add cases
│   ├── linux.md          building and running on Linux
│   ├── macos.md          building and running on macOS
│   ├── zed.md            using Tern as an ACP agent in Zed
│   └── decisions/        architecture decision records (0001 to 0007)
├── AGENTS.md             rules for AI coding agents
├── CHANGELOG.md
├── CODE_OF_CONDUCT.md
├── CONTRIBUTING.md
├── LICENSE
├── README.md
└── SECURITY.md
```

## Building from source

| OS | Before `cargo build` |
| --- | --- |
| Windows | Rust (MSVC toolchain) and the Visual Studio C++ build tools |
| Linux | Rust and the desktop libraries: on Debian or Ubuntu `sudo apt install build-essential pkg-config libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev libssl-dev` (Fedora and Arch in [docs/linux.md](docs/linux.md)); a Vulkan driver to run the app |
| macOS | Rust and the Xcode command-line tools (`xcode-select --install`) |

| Command | Does |
| --- | --- |
| `cargo build` | Builds the workspace |
| `cargo test --workspace` | Runs the tests |
| `cargo run -p tern-desktop` | Opens the desktop app |
| `TERN_TOUR=1 cargo run -p tern-desktop` | Plays the scripted UI tour with demo data (no keys, nothing saved) |
| `cargo run -p tern-tui -- doctor` | Checks paths, keychain, and store |
| `cargo run -p tern-tui -- provider add groq` | Adds a provider key (hidden prompt, tested before saving) |
| `cargo run -p tern-tui -- search add tavily` | Adds a web search key |
| `cargo run -p tern-tui -- ask "your question"` | A cited answer from the web |
| `cargo run -p tern-tui -- research --deep "your question"` | Deep research: a plan to approve, then parallel workers |
| `cargo run -p tern-tui -- report` | Prints the last run report (`--json`, `--out FILE`, `--list`) |
| `cargo run -p tern-tui -- resume <chat>` | Resumes a stopped research run |
| `cargo run -p tern-tui -- watch add "question" --every weekly` | Puts a question on watch (`watch list\|rm\|run`) |
| `cargo run -p tern-tui -- study add notes.pdf` | Adds study materials (`study topics`, `tutor`, `quiz`, `viva`) |
| `cargo run -p tern-tui -- cards gen` | Writes flashcards from your materials (`cards review`, `list`, `stats`, `edit`, `suspend`, `rm`) |
| `cargo run -p tern-tui -- cards export --apkg deck.apkg` | Exports the deck for Anki (`--csv FILE`, `--tsv FILE`; `cards import FILE` reads CSV or TSV) |
| `cargo run -p tern-tui -- explain3d "concept" --svg out.svg` | Builds an Explain in 3D scene and writes a snapshot (`--topic`, `--step N`, `--vault`) |
| `cargo run -p tern-tui -- decide "Which source first?" --choices a,b,c` | Asks the decision model (`--context FILE`, `--kind source\|tool\|claim\|triage`, `--json`) |
| `cargo run -p tern-tui -- decide mode remote --provider typesafe` | Chooses the decision model (`off`, `local`, `remote`; `decide key add typesafe\|subq\|voyage`, `decide status`) |
| `cargo run -p tern-tui -- decide bench` | Scores the bundled fixtures: fixed order and the local scorer (`--reasoning` adds the chat model, `--remote` the remote decision model) |
| `cargo run -p tern-tui -- range rm Q3-Q5` | Removes messages to the chat's trash (`range mv Q3-Q5 [--sub]`, `confirm`, `undo`, `restore`, `trash`; `--chat <id>`) |
| `cargo run -p tern-tui -- sub new Q12 "question"` | Plans a sub-session under Q12 (`sub list`, `sub run Q12.1` runs it and reports back) |
| `cargo run -p tern-tui -- show Q12.3` | Prints a message by its ID or `tern://` link |
| `cargo run -p tern-tui -- handoff --out packet` | Writes `packet.md` and `packet.json`; `new --from-handoff packet.md` starts a session from it alone |
| `cargo run -p tern-tui -- library list` | Lists the Library (`--kind`, `--sort`, `--page`, `--trash`; `library search`, `rm`, `restore`, `empty-trash`) |
| `cargo run -p tern-tui -- side "question"` | A throwaway question about the last chat; nothing is saved |
| `cargo run -p tern-tui -- mcp add notes -- npx -y notes-mcp` | Adds an MCP server, off until reviewed (`--url` for HTTP; `mcp import FILE`, `list`, `preview NAME --trust`, `enable`, `disable`, `health`, `rm`) |
| `cargo run -p tern-tui -- mcp prompts` | Lists the prompts reviewed servers offer (`mcp prompt notes review --arg code=…` prints the server's filled-in text; nothing is sent) |
| `cargo run -p tern-tui -- mcp resources` | Lists resources and templates (`mcp read notes notes://week-1`, `--arg name=… ` for a template, `--out FILE`, `--wrapped` to see it as the model would) |
| `cargo run -p tern-tui -- eval run` | Runs the offline evaluation suites, 3 trials (`--level unit\|synthetic\|pinned\|public`, `--live`, `--scifact DIR`, `--check-baseline`; `eval list`, `report [--list]`, `compare`, `usability`) |
| `cargo run -p tern-tui -- acp` | Serves the Agent Client Protocol on stdio for an editor (see [docs/zed.md](docs/zed.md)) |
| `cargo run -p tern-tui -- skills list` | Lists skills (`install PATH\|URL`, `enable`, `disable`, `show`, `rm`) |
| `cargo run -p tern-tui -- instructions` | Prints exactly what the model is told, piece by piece |
| `cargo run -p tern-tui -- project new "Thesis" --root D:/thesis` | Makes a project folder (`project list`, `mv <chat> <project>`, `rename`, `root`, `rm`) |
| `cargo run -p tern-tui` | The terminal chat (`-- chat --provider ollama` for a local model; `/` completes commands and MCP prompts with Tab) |

Motion turns off in the desktop app when the OS asks for reduced motion (GNOME or KDE on Linux, Accessibility on macOS), or with `TERN_REDUCED_MOTION=1` or `"reduced_motion": true` in `preferences.json` on any OS. On Linux without a Secret Service, set `TERN_KEYRING=keyutils` to keep keys in the kernel keyring until reboot.

## Documentation

| Doc | Covers |
| --- | --- |
| [Vision](docs/00-vision.md) | Why Tern exists and what success looks like |
| [Product spec](docs/01-product-spec.md) | Harnesses, features, chat behaviour, vault |
| [UI definition](docs/02-ui-definition.md) | Layouts, streaming, palette, motion, Scout |
| [Architecture](docs/03-architecture.md) | Crates, contracts, memory, platforms |
| [Changelog](CHANGELOG.md) | Everything merged so far |
| [Roadmap](docs/04-roadmap.md) | Phases and exit checks |
| [Security model](docs/05-security-model.md) | Threats, controls, trust order |
| [Evaluation](docs/evaluation.md) | The test ladder, suites, metrics, scores, adding cases |
| [Web search](docs/search.md) | Wikipedia (keyless default), Tavily, Brave, SearXNG with its address and JSON output, search errors |
| [Linux](docs/linux.md) · [macOS](docs/macos.md) | Build dependencies, running, OS details, known limits |
| [Contributing](CONTRIBUTING.md) · [Agent rules](AGENTS.md) · [Security policy](SECURITY.md) | How work happens here |

## Licence

Copyright (c) 2026 Khubaib Nazeer. All rights reserved. Tern is proprietary software; no licence is granted to use, copy, modify, or distribute it. The Tern name, the diving-tern mark, the wordmark, and the Scout mascot are reserved brand assets. See [LICENSE](LICENSE).
