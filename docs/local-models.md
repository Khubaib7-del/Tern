# Local models

Tern can run OCR, document parsing, vision, speech, and embedding models on your own computer. **Nothing is bundled**: the app stays small and a model is downloaded only when you ask for it, from Hugging Face, into Tern's data folder.

This page describes the foundation that ships in PR #62 (images, OCR and voice, part 1). The specific models are being chosen separately; they plug in through the manifest below without code changes to the download, state, or settings.

## What works today

- **A manifest of models** (`crates/tern-agent/src/local_models/manifest.json`, compiled in). Each entry has an `id`, a `task` (`ocr`, `doc_parse`, `vision`, `speech_to_text`, `text_to_speech`, `embedding`), a display `name`, the Hugging Face `hf_repo` and a pinned `revision`, the `files` (path in the repo, `size` in bytes, `sha256`, optional full `url`), the `licence`, and the `runtime` that runs it (`whisper_cpp`, `onnx`, `llama_cpp`, `candle`, `piper`). The built-in entries are **placeholders** (marked `"placeholder": true` and "(placeholder)" in their names): Whisper base.en and tiny.en (ggml, MIT) and RapidOCR PP-OCRv4 mobile (ONNX, Apache-2.0).
- **Your own entries**: a `local-models.json` in Tern's settings folder (same format) adds models, and replaces built-in ones with the same `id`. A broken file is ignored with a warning in Settings and the CLI.
- **On-demand download** to `<data>/models/<id>/`: one file at a time into `<file>.part`, resumed with an HTTP `Range` request after an interruption or Cancel (a server that ignores the range starts the file again), progress across all of the model's files, every file checked against its SHA-256 (a mismatch deletes that file and installs nothing), and `tern-model.json` written last, so a model counts as installed only when every file is whole. `TERN_HF_ENDPOINT` points downloads at a mirror.
- **State and choices**: installed models are on unless switched off; one default per task. Saved in `preferences.json` under `local_models` (`disabled`, `defaults`). Deleting a model removes its folder and drops it from both.
- **Settings › Local models** on desktop: a section per task, a row per model with its logo, name, size, licence and runtime, **Download** (or **Resume**) with a progress bar and **Cancel**, an **Enable** switch, a **Default** radio per task, and **Delete** (asks twice). Rows far off screen are culled like Settings › Providers.
- **CLI**: `tern models list [--task ocr] [--json]`, `tern models download <id> [--default]`, `tern models remove <id>`, `tern models default [<task> [<id>|none]]`, `tern models enable <id> [on|off]`.
- **Pictures for chat models that cannot see**: a provider's vision or OCR model reads them first, as before; when none is set up, an installed and enabled local OCR model is the fallback. Its text is passed on as untrusted data with a notice.

## Not yet

- **No runtime is linked in.** A local model's files download and verify, but this build cannot run them: rows say "This build cannot run ONNX Runtime models yet", and a picture that would fall back to a local OCR model gets a note saying so instead of being read. A runtime plugs in by implementing `tern_agent::local_models::LocalEngine` and calling `register_engine` at startup; `runtime_available` then turns on for it everywhere.
- The mic still uses a provider's speech-to-text model; a local Whisper default is the next step once its runtime lands.
- The placeholder entries will be replaced by the models the research picks.

## Code

- `tern-providers::download`: the byte mover (range resume, progress, cancel). Only `tern-providers` and `tern-crawl` may reach the network (`dependency_rules`).
- `tern-agent::local_models`: manifest, download orchestration and verification, state, preferences, the engine registry. Unit tests run against a local mock HTTP server.
- `tern-agent::media::plan_images_local` / `read_images_local`: the local OCR fallback.
- `tern-desktop`: `localmodels.rs` (GPUI-free page state) and `view/localmodels_view.rs`.
- `tern-tui`: `models.rs`.
