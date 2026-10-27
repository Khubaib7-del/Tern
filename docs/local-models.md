# Local models

Tern can run OCR, document parsing, vision, speech, and embedding models on your own computer. **Nothing is bundled**: the app stays small and a model is downloaded only when you ask for it, from Hugging Face, into Tern's data folder.

> **Five runtimes are linked: OCR, transcription, live dictation, pictures and read aloud.** PP-OCRv6 small with the Arabic-script recogniser runs on ONNX Runtime through the pure-Rust [`oar-ocr`](https://crates.io/crates/oar-ocr) crate and reads pasted or attached pictures (English and Urdu) for chat models that cannot see. **Whisper** (small and large-v3 turbo) runs on [whisper.cpp](https://github.com/ggml-org/whisper.cpp) through `whisper-rs`, on the GPU (Vulkan on Windows and Linux, Metal on macOS) with the CPU as fallback ([speech.md](speech.md)), and transcribes attached audio files. **Moonshine Streaming Small** runs on ONNX Runtime (`ort`) and transcribes the mic as you speak. **SmolVLM 500M / 256M, Qwen3-VL 2B and Granite-Docling** run on llama.cpp through `tern-vlm`, a helper program shipped beside Tern (#69), to describe pictures and read scanned pages. **Kokoro 82M** runs on ONNX Runtime through `loqui-kokoro` and reads answers aloud (#69). The remaining models download and verify but do not run yet: their row says "This build cannot run it yet".

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
| Describe pictures | Alternative | SmolVLM 256M Instruct (Q8) (`smolvlm-256m`) | 279 MB | Apache-2.0 | llama.cpp |
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
- **CLI**: `tern models list [--task ocr|dictation|…] [--json]` (roles and every task, Live dictation included), `tern models download <id> [--default]`, `tern models remove <id>`, `tern models default [<task> [<id>|none]]`, `tern models enable <id> [on|off]`, `tern models read <picture> [--model <id>]`.
- **Local OCR** (`tern_agent::local_models::onnx_ocr`, the `ocr-onnx` feature, on in the desktop app and the CLI): PP-OCRv6 small finds the lines and reads them; with the Arabic-script recogniser installed beside it, each line is read by both and keeps the surer reading, and Urdu lines are put in reading order (PaddleOCR's `pred_reverse`). The detector thresholds and character dictionaries come from the models' own `inference.yml`. ONNX Runtime is linked statically (the `ort` crate's prebuilt CPU library). A model loads once and stays loaded.
- **Pictures for chat models that cannot see** (`tern_agent::media::route_images`, `read_with`): readers are tried in this order, each failure falling through to the next: a **local vision model**, the **local OCR model** (installed, on, and runnable), a **provider's vision model**, then a **provider's OCR model** (a saved OCR default goes before an automatic vision pick). Any failure counts: a refused key, no credits, a rate limit, a timeout, a server error, a model that refuses pictures, a local model that will not load or reads nothing. The text that was read joins the message as untrusted data, and an info toast names the reader ("codestral-2508 cannot see pictures, so Tern read the text in img.jpg with local PP-OCRv6 small."). When every reader fails, the message still goes, with "[Tern couldn't read img.jpg: <reason>]" in place of the picture; the turn never fails because of a picture. A local model's failure is shown on its Settings row.
- **Pictures are normalised first** (`tern_agent::pictures`): PNG or JPEG, at most 2048 px on the long edge and 5 MB. WebP, GIF (first frame), oversized PNGs and big JPEGs are converted; HEIC/AVIF and files that are not pictures are refused with a plain reason. This applies to pictures sent to a chat model that sees, to a provider reader, and to a local model.
- **A model that turns out not to see**: when a provider refuses pictures for a model (Mistral's code 3051 "Image input is not enabled for this model", OpenAI's "does not support image", and the like), Tern remembers it in `<data>/not-vision.json` and never sends it pictures again. If it was the chat model, the pictures leave that chat's earlier messages (each leaves a short note), and the turn runs again through the readers with no failed run. A model that cannot see never gets a picture from anywhere: earlier messages in every chat lose theirs ("[image: name]") before the request, a picture attached anyway goes as a note, a refusal caused by an earlier picture reruns the turn without it, and run checkpoints never keep pictures, so Resume and Retry never replay one. A provider vision model that refuses is skipped next time.
- **Plain provider errors**: a provider's JSON error body is shown as its message and code, for example "Mistral: Image input is not enabled for this model (code 3051) (HTTP 400)", in toasts and the run banner.
- **Local transcription** (`whisper_cpp.rs`, the `whisper` feature, on in the desktop app and the CLI): Whisper small (q5_1) or large-v3 turbo (q5_0) on whisper.cpp, built from source and linked statically, on the GPU when one works (Vulkan or Metal, `whisper-vulkan` / `whisper-metal` features) and the CPU otherwise; Settings › Speech picks the device and CPU threads ([speech.md](speech.md)). Audio files (WAV, MP3, M4A/AAC, Ogg Vorbis, FLAC) are decoded with `symphonia` (pure Rust) and resampled to 16 kHz mono. The x86-64 build targets AVX2/FMA/F16C (`.cargo/config.toml`, never `-march=native`), which every x86-64 PC from about 2013 on has; ARM64 uses NEON. On desktop an attached audio file is transcribed by the installed Whisper model before any provider. `tern models transcribe <file> [--model <id>] [--language ur]`.
- **Local live dictation** (`moonshine_onnx.rs`, the `asr-onnx` feature): Moonshine Streaming Small on ONNX Runtime. The mic uses it when it is installed and on, before a provider's speech-to-text model; with neither, the installed Whisper model takes the mic.
- **`tern models read <picture> [--model <id>]`** prints the text the local OCR model reads.

## Pictures, documents and read aloud (#69)

### Describing pictures: `tern-vlm`

- **The helper.** llama.cpp's multimodal CLI (`llama-mtmd-cli`, MIT) at a pinned release (`b11900`), built by CI with `.github/scripts/build-vlm.sh` and renamed `tern-vlm`. It ships in a `vlm/` folder beside the app: `vlm/tern-vlm.exe` with `ggml-cpu.dll`, `ggml-vulkan.dll` and friends on Windows, `vlm/tern-vlm` with its `.so` files on Linux, `Tern.app/Contents/Resources/vlm/tern-vlm` on macOS (one static program with Metal). `TERN_VLM` points Tern at another copy. Without it, the vision rows say the build does not include the helper and pictures go to the next reader.
- **Why a separate program**: a crash in the C++ runtime or a GPU driver ends the helper, never Tern. A failed run is a banner over the composer with **Use the alternative** or **Download**.
- **Device**: Settings › Speech › Device applies. Auto and GPU offload every layer (`-ngl 99`; Vulkan on Windows and Linux, Metal on macOS) and run again on the CPU (`--device none`) when that fails; a PC without a Vulkan driver loads no Vulkan backend and runs on the CPU. CPU threads follow Whisper's rule (Auto, at most four). A run stops after 240 s. Each run logs the model, device, threads and time in `tern.log` (`vlm: model=… device=… threads=… took=…`).
- **GPU reason and short answers** (#70): Tern asks `tern-vlm` for its GPUs once, and the `vlm:` line says why a run is on the CPU (no GPU listed, Device set to CPU, the GPU run failed). SmolVLM is capped at 200 tokens with a repeat penalty and a brief prompt, repeated lines are dropped, and pictures are scaled to 1024 px on the long edge first.
- **In the chat**: a picture for a chat model that cannot see is described by the installed vision model first (the status line says "Looking at image…"); the local OCR text goes with the description. SmolVLM 256M is the first alternative (half the size of 500M, faster on an older laptop), Qwen3-VL 2B the second.
- `tern models describe <picture> [--model <id>] [--prompt "…"]` prints the description and the device.

### Documents to Markdown

Attached documents become Markdown before the model sees them (`tern_agent::docs`):

- **PDF**: the text layer, page by page. A page counts as scanned when it has a picture and under 100 letters of text, or only a scanning app's watermark ("Scanned with CamScanner" and the like) (#70); scanned pages are read by Granite-Docling (DocTags → Markdown: headings, lists, tables, captions, code, formulas) when it is installed and runnable, else by the local OCR model; at most 30 scanned pages. A PDF with text pages and scanned pages keeps both, in page order.
- **Read as scanned** (#70): a PDF chip offers it to read every page as a scan. Typing "try local model granite docling" (or "read it again as scanned") reads the chat's last PDF again as scanned, then sends.
- **Old Office files** (#70): `.xls` (Excel 97–2003, BIFF8 cells, a table per sheet), `.ppt` (slide text) and `.doc` (the document's text) through `office_legacy.rs`, Tern's own compound-file reader (no extra crates). A file it cannot read says to Save As `.xlsx`, `.pptx` or `.docx`.
- **DOCX** (headings, lists, tables), **PPTX** (a section per slide), **XLSX** (a table per sheet, at most 300 rows and 40 columns), and an **HTML** file attached on its own (its main text).
- The result is cached by the file's SHA-256, so attaching the same file again is instant.
- The chip says how it was read: "PDF text → Markdown · 12 pages", "DOCX → Markdown", "Granite-Docling 258M (Q8) → Markdown · 3 pages", "OCR (PP-OCRv6 small) → Markdown · 2 pages". While a single document converts, the composer says "Converting report.pdf to Markdown…".
- `tern models convert <file>` prints the Markdown.

### Read aloud: Kokoro

- **Kokoro 82M (int8)** on ONNX Runtime through [`loqui-kokoro`](https://crates.io/crates/loqui-kokoro) (MIT) with [`loqui-g2p`](https://crates.io/crates/loqui-g2p) (MIT) for the phonemes: misaki's English lexicons and a small neural model for unknown words. No espeak-ng (GPL-3.0) is linked or downloaded.
- **The speaker button** under an answer reads it sentence by sentence (long sentences in clause-sized pieces since #70, so the first audio comes sooner; `tern.log` says how fast synthesis ran against real time): the first sentence plays as soon as it is made, the next ones are made behind it. While it reads the button becomes **Pause**/**Play** and **Stop** with "Reading 2 of 5". Leaving the chat or starting another reading stops it. Code blocks are not read out (they are announced as "code block"), and link addresses are left out.
- **English only**: an Urdu answer gets the note "Read aloud supports English for now." instead of garbled phonemes; English sentences in a mixed answer are read.
- **Settings › Speech › Read aloud**: Voice (Heart, Michael, Emma, George), Speed (0.8× to 1.3×), and the voice model in use. Saved as `read_aloud_voice` and `read_aloud_speed` in `preferences.json`.
- With no voice model downloaded, the button shows the banner with **Download** (Kokoro 82M).
- `tern models speak "text" --out hello.wav [--voice af_heart] [--speed 1.0]`.

### CI

Every pull request builds `tern-vlm` on Windows, Linux and macOS (cached by the hash of `build-vlm.sh`) and puts `vlm/` in the test builds. On Windows, CI checks that `tern-vlm.exe --list-devices` runs with the runner's Vulkan loader hidden. On Linux, `.github/scripts/local-runtimes-smoke.sh` downloads SmolVLM 256M and Kokoro (cached) and runs `tern models describe` on a test picture (checking SmolVLM's capped output), `tern models speak` to a WAV and `tern models convert` on a DOCX, a scanned PDF with a watermark (the OCR/Granite path), an `.xls` and a `.ppt`.

## Not yet

- Document layout (PaddleOCR layout + tables), embeddings, Parakeet (the dictation alternative), Supertonic (the read-aloud alternative) and the Tesseract OCR alternative download and verify but do not run. A runtime plugs in by implementing `tern_agent::local_models::LocalEngine` (with `runs` saying which models it takes) and registering it in `register_builtin_engines`; `can_run` then turns on for those models everywhere.
- Read aloud is English only, and on the CPU.
- Whisper's and `tern-vlm`'s GPU speed is not measured in CI (the runners have no GPU); see [speech.md](speech.md).

## Code

- `tern-providers::download`: the byte mover (range resume, progress, cancel). Only `tern-providers` and `tern-crawl` may reach the network (`dependency_rules`).
- `tern-agent::local_models`: manifest, download orchestration and verification, state, preferences, the engine registry. Unit tests run against a local mock HTTP server. `paddle.rs` reads PaddleOCR's `inference.yml` (dictionary, thresholds) and orders Urdu lines; `onnx_ocr.rs` is the OCR engine (an `#[ignore]`d test reads a real picture given `TERN_OCR_MODEL_DIR` and `TERN_OCR_IMAGE`).
- `tern-agent::media`: `route_images` / `readers_for` (who reads pictures, best first), `read_chain` / `read_with` (the fallback chain), `mark_not_vision` / `is_not_vision` / `rejects_images`, `plain_error`. `tern-agent::pictures`: normalising pictures. `local_models/whisper_cpp.rs`, `moonshine_onnx.rs`, `audio.rs` (decode), `resample.rs` (16 kHz), `llama_vlm.rs` (the `tern-vlm` helper, DocTags), `kokoro.rs` and `kokoro_text.rs` (read aloud). `tern-agent::docs`: documents to Markdown. `tern-agent::read_aloud` and `tern-platform::speaker`: playing an answer.
- `tern-desktop`: `localmodels.rs` (GPUI-free page state) and `view/localmodels_view.rs`; logos in `assets/brands/` (see [assets.md](assets.md)).
- `tern-tui`: `models.rs`.
