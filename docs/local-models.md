# Local models

Tern can run OCR, document parsing, vision, speech, and embedding models on your own computer. **Nothing is bundled**: the app stays small and a model is downloaded only when you ask for it, from Hugging Face, into Tern's data folder.

> **Runtimes are not linked in yet.** Every model below downloads, resumes, and verifies against its SHA-256 today, and you can pick, enable, and delete them, but this build cannot run them: their rows say "This build cannot run … models yet". The engines (ONNX Runtime, llama.cpp, whisper.cpp, Tesseract) plug in through `LocalEngine` in later PRs, starting with OCR.

## The models

Each task has a **Default** (what Tern uses unless you pick another) and an **Alternative** (the backup: pick it with **Use for this task**, and Tern offers it when the default fails). Optional add-ons download only when you want them. Files come from each model's Hugging Face repo at a pinned commit; a model can be made of files from several repos.

| Task | Role | Model (`id`) | Size | Licence | Runtime |
| --- | --- | --- | --- | --- | --- |
| Text in pictures (OCR) | Default | PP-OCRv6 small + Arabic-script recogniser (`pp-ocrv6-small`) | 39 MB | Apache-2.0 | ONNX Runtime |
| Text in pictures (OCR) | Alternative | Tesseract (English + Urdu) (`tesseract-eng-urd`) | 6 MB | Apache-2.0 | Tesseract |
| Documents to Markdown | Default | PP-DocLayoutV3 + PP-OCRv6 + SLANet+ (`paddle-doc-markdown`) | 169 MB | Apache-2.0 | ONNX Runtime |
| Documents to Markdown | Optional add-on | Math formulas (PP-FormulaNet+ L, on demand) (`pp-formulanet-plus-l`) | 730 MB | Apache-2.0 | ONNX Runtime |
| Documents to Markdown | Alternative | Granite-Docling 258M (Q8) (`granite-docling-258m`) | 282 MB | Apache-2.0 | llama.cpp |
| Describe pictures | Default | SmolVLM 500M Instruct (Q8) (`smolvlm-500m`) | 546 MB | Apache-2.0 | llama.cpp |
| Describe pictures | Alternative | Qwen3-VL 2B Instruct (Q4_K_M) (`qwen3-vl-2b`) | 1.6 GB | Apache-2.0 | llama.cpp |
| Live dictation | Default | Moonshine Streaming Small (`moonshine-streaming-small`) | 170 MB | MIT | ONNX Runtime |
| Live dictation | Alternative | Parakeet TDT 0.6B v3 (int8) (`parakeet-tdt-0.6b-v3`) | 671 MB | CC-BY-4.0 | ONNX Runtime |
| Transcribe audio files | Default | Whisper small (q5_1) (`whisper-small-q5_1`) | 190 MB | MIT | whisper.cpp |
| Transcribe audio files | Alternative | Whisper large-v3 turbo (q5_0) (`whisper-large-v3-turbo-q5_0`) | 574 MB | MIT | whisper.cpp |
| Read aloud | Default | Kokoro 82M (int8) (`kokoro-82m-int8`) | 94 MB | Apache-2.0 | ONNX Runtime |
| Read aloud | Alternative | Supertonic 3 (`supertonic-3`) | 399 MB | OpenRAIL-M | ONNX Runtime |
| Search by meaning (embeddings) | Default | Granite Embedding 97M multilingual R2 (int8) (`granite-embedding-97m-r2`) | 123 MB | Apache-2.0 | ONNX Runtime |
| Search by meaning (embeddings) | Alternative | EmbeddingGemma 2 (Q8) (`embeddinggemma-2-q8`) | 310 MB | Apache-2.0 | llama.cpp |

Notes on the picks:

- **OCR**: PP-OCRv6 small (text detector and recogniser) plus PaddleOCR's PP-OCRv5 Arabic-script recogniser, for English and Urdu. Tesseract's fast English and Urdu LSTM data is the lighter backup.
- **Documents**: PP-DocLayoutV3 finds the page layout, PP-OCRv6 reads it and SLANet+ rebuilds tables, to write Markdown; PP-FormulaNet+ L (730 MB) turns formulas into LaTeX and is only needed for maths. Granite-Docling 258M is a one-model alternative that reads a page image straight to DocTags/Markdown.
- **Live dictation** (the mic, as you speak) and **Transcribe audio files** are separate tasks: Moonshine Streaming Small is built for streaming English; Whisper small and large-v3 turbo cover many languages, Urdu included.
- **Read aloud**: Kokoro 82M runs without espeak-ng: Tern does not link espeak-ng (GPL-3.0) or download its data.

### Licence notes shown with the models

- **Parakeet TDT 0.6B v3** is CC-BY-4.0: its row carries the attribution "Parakeet TDT 0.6B v3 by NVIDIA, licensed CC-BY-4.0; ONNX conversion by istupakov".
- **Supertonic 3** is under the BigScience OpenRAIL-M licence: free to use, but not for the restricted uses it lists (for example impersonation to deceive, harassment, unlawful uses), and those restrictions pass on to anything you share made with it. The row says so, links the licence, and the licence is saved with the model.
- ONNX and GGUF conversions by third parties (ningpp, Workmind, istupakov, onnx-community, ggml-org, the whisper.cpp project) are credited in each model's licence note.

## What works today

- **The manifest** (`crates/tern-agent/src/local_models/manifest.json`, compiled in). Each entry has an `id`, a `task` (`ocr`, `doc_parse`, `vision`, `dictation`, `speech_to_text`, `text_to_speech`, `embedding`), a `role` (`default`, `alternative`, `addon`), a display `name`, the Hugging Face `hf_repo` and pinned `revision`, the `files` (path in the repo, optional `repo` and `revision` for a file from another repo, optional `dest` under the model's folder, `size` in bytes, `sha256`, optional full `url`), the `licence` (SPDX id) with an optional `licence_note` and `licence_url`, a `logo` key, and the `runtime` that runs it (`onnx`, `llama_cpp`, `whisper_cpp`, `tesseract`, `candle`, `piper`).
- **Your own entries**: a `local-models.json` in Tern's settings folder (same format) adds models, and replaces built-in ones with the same `id`. A broken file is ignored with a warning in Settings and the CLI.
- **On-demand download** to `<data>/models/<id>/`: one file at a time into `<file>.part`, resumed with an HTTP `Range` request after an interruption or Cancel (a server that ignores the range starts the file again), progress across all of the model's files, every file checked against its SHA-256 (a mismatch deletes that file and installs nothing), and `tern-model.json` written last, so a model counts as installed only when every file is whole. `TERN_HF_ENDPOINT` points downloads at a mirror.
- **State and choices**: installed models are on unless switched off; one model in use per task. Saved in `preferences.json` under `local_models` (`disabled`, `defaults`). Deleting a model removes its folder and drops it from both.
- **Settings › Local models** on desktop: a section per task with a line on what it does; a row per model with its maker's logo, name, **Default**/**Alternative**/**Optional** badge, size, licence and runtime, the licence note, **Download** (or **Resume**, or **Add** for an add-on) with a progress bar and **Cancel**, an **Enable** switch, a **Use for this task** radio, and **Delete** (asks twice). When a model fails, its row offers **Use the alternative**. Rows far off screen are culled like Settings › Providers.
- **CLI**: `tern models list [--task ocr|dictation|…] [--json]` (roles and every task, Live dictation included), `tern models download <id> [--default]`, `tern models remove <id>`, `tern models default [<task> [<id>|none]]`, `tern models enable <id> [on|off]`.
- **Pictures for chat models that cannot see**: a provider's vision or OCR model reads them first, as before; when none is set up, an installed and enabled local OCR model is the fallback, and until its runtime is linked a note under the waiting picture says why it cannot be read yet.

## Not yet

- **No runtime is linked in**, so no local model runs yet (see the note at the top). A runtime plugs in by implementing `tern_agent::local_models::LocalEngine` and calling `register_engine` at startup; `runtime_available` then turns on for it everywhere.
- The mic still uses a provider's speech-to-text model; the local dictation default takes over once its runtime lands.

## Code

- `tern-providers::download`: the byte mover (range resume, progress, cancel). Only `tern-providers` and `tern-crawl` may reach the network (`dependency_rules`).
- `tern-agent::local_models`: manifest, download orchestration and verification, state, preferences, the engine registry. Unit tests run against a local mock HTTP server.
- `tern-agent::media::plan_images_local` / `read_images_local`: the local OCR fallback.
- `tern-desktop`: `localmodels.rs` (GPUI-free page state) and `view/localmodels_view.rs`; logos in `assets/brands/` (see [assets.md](assets.md)).
- `tern-tui`: `models.rs`.
