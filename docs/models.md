# Models: listing, capabilities, and routing

How Tern lists the models a key can use, decides what each one can do, and sends work to the model that can do it. Code: `crates/tern-providers/src/model.rs`, `crates/tern-providers/src/client.rs` (`list_models`, `transcribe`, `ocr`), `crates/tern-agent/src/media.rs`, `crates/tern-platform/src/mic.rs`.

## Listing

Tern lists every model the provider returns for the key. Nothing is filtered for rate limits or plans; a model the key cannot call fails when called, with the provider's own error message ("The provider said: ..."). Deprecated and retired models are kept and marked; the picker hides them behind **Show retired**.

| Provider | Endpoint | Pages | What the listing says |
| --- | --- | --- | --- |
| OpenAI, Groq, DeepSeek, Cerebras, Fireworks | `GET /models` | `has_more` + `last_id` followed with `?after=` when a server pages | Ids only (Groq adds `context_window` and `active`) |
| Mistral | `GET /v1/models` | none | `capabilities` flags (chat, vision, function calling, reasoning, OCR, transcription, speech, moderation), `max_context_length`, `name`, `aliases`, `deprecation` |
| Anthropic | `GET /v1/models?limit=1000` | `has_more` + `last_id` followed with `after_id` (the default page is 20) | `capabilities` (`image_input`, `thinking`), `max_input_tokens`, `display_name`, `lifecycle` |
| Google AI Studio | `GET /v1beta/models?pageSize=1000` | `nextPageToken` followed | `supportedGenerationMethods`, `inputTokenLimit`, `thinking`, `displayName` |
| OpenRouter | `GET /api/v1/models?output_modalities=all` (the default lists text-output models only) | full list | `architecture.input_modalities` / `output_modalities`, `supported_parameters`, `context_length`, `expiration_date` |
| xAI | `GET /v1/models`, then `/v1/language-models` and `/v1/image-generation-models` | none | modalities and aliases from the second two |
| Together | `GET /v1/models` (a bare array) | none | `type` (chat, image, embedding, moderation, rerank, audio, transcribe) |
| Ollama | `GET /v1/models`, then `GET /api/tags` | none | `capabilities` (completion, vision, tools, thinking, embedding) |
| LM Studio | `GET /v1/models`, then `GET /api/v0/models` | none | `type` (llm, vlm, embeddings) |

The second call for xAI, Ollama, and LM Studio is best effort: if it fails, the first listing stands.

**Aliases.** Mistral lists `mistral-large-latest` and `mistral-large-2512` as two cards that name each other. Tern shows one row, the id with a version or date (it names the exact model), with the other names as its aliases ("Also: mistral-large-latest"). Search matches the id, the name, and every alias.

**Names.** Rows show the provider's name for a model when it has one. Mistral's ids carry a date rather than the version people know ("Mistral Small 4" is `mistral-small-2603`), so Tern keeps a small table of Mistral names from Mistral's model page for listings that leave the name out (`known_display_name`).

## Capabilities

| Capability | Meaning |
| --- | --- |
| Chat | Answers chat messages |
| Vision | Reads pictures sent with a chat message |
| Tools | Calls tools |
| Reasoning | Thinks before answering |
| SpeechToText | Transcribes speech (`/audio/transcriptions`) |
| TextToSpeech | Reads text aloud (`/audio/speech`) |
| Ocr | Reads text out of documents and pictures (Mistral `/ocr`) |
| ImageGen | Makes pictures |
| Embedding | Turns text into vectors |
| Moderation | Screens text for safety |
| Rerank | Ranks passages against a query |

Order of evidence:

1. **The listing, when it names every capability** (Mistral, Anthropic, OpenRouter, xAI, Ollama): taken as given.
2. **The listing, when it names only the role** (Gemini's generation methods, Together's and LM Studio's `type`): the role is kept, the pattern table adds vision and tools, and a model whose name clearly says its job keeps that job (Gemini's TTS models accept `generateContent` but only speak).
3. **The pattern table** (`table_capabilities`), for listings that say nothing (OpenAI, Groq, DeepSeek, Cerebras, Fireworks). The first role that matches wins: `whisper`, `transcribe`, `-stt` → speech to text; `-tts`, `tts-`, `orpheus`, `playai` → text to speech; `-ocr`, `ocr-` → OCR; `embed`, `bge-`, `e5-`, `nomic-embed` → embeddings; `imagen`, `dall-e`, `gpt-image`, `flux`, `stable-diffusion` → image; `moderation`, `guard`, `shieldstral` → safety; `rerank` → rerank; `realtime`, `sora`, `veo-`, `video`, `timestral` → listed, not used. Anything else is chat; vision and tools come from family lists (GPT-4o and later, Claude, Gemini, Gemma 3, Llama 4, Pixtral, Mistral Large 3 and 4, Medium 3 and later, Small 3.1 and later, Ministral 3, Qwen VL and Qwen 3.8, Grok 4, and others).

Reasoning is also set from Tern's thinking table (`thinking_support`), the one the effort chip uses.

## Categories

Each model is in exactly one picker section, in this order: **Chat** (chat, text only), **Vision** (chat that also reads pictures), **Speech** (speech to text, text to speech), **OCR**, **Image**, **Embeddings**, **Safety**, **Other**. Rows carry small badges for the rest (vision, tools, thinks, speech to text, OCR).

## The not-a-chat-model notice

Only a model with the Chat capability becomes the chat's model. Picking any other keeps the current chat model and says why, for example:

> whisper-large-v3 only transcribes speech, so it cannot chat. Pick a chat model; Tern uses whisper-large-v3 for the mic.

| Picked | What happens |
| --- | --- |
| A speech-to-text model | Saved as `default_stt_model`; the mic uses it |
| A Mistral OCR model | Saved as `default_ocr_model`; it reads pictures the chat model cannot see when no vision model is chosen |
| Anything else that cannot chat | The notice only |

## Routing

Preferences (`preferences.json`, each `provider/model`; `None` picks automatically):

| Key | Used for | Automatic pick |
| --- | --- | --- |
| `default_stt_model` | The mic and attached audio files | A listed speech-to-text model on an OpenAI-style provider, the chat's provider first, fast ones first (`*-turbo`, `gpt-transcribe`, `*-mini-transcribe`); realtime-only models are skipped |
| `default_vision_model` | Describing pictures for a chat model that cannot see | A listed vision chat model, the chat's provider first, small fast ones first |
| `default_ocr_model` | Reading text out of pictures when no vision model is chosen | The newest listed Mistral OCR model |

The routing reads every usable provider's listing at once (`tern_agent::media::catalog`), so a Groq Whisper model can serve the mic while the chat runs on Mistral.

### The mic

The mic button is on when a speech-to-text model is listed on any configured provider; otherwise it is off with a tooltip saying what is missing. Click to record (cpal: WASAPI on Windows, CoreAudio on macOS, ALSA on Linux), click again or press Esc to stop (Esc throws the audio away). The recording is made into 16 kHz mono 16-bit WAV, sent to the speech-to-text model with a multipart `POST /audio/transcriptions` (Groq, OpenAI, Mistral Voxtral, compatible servers), and the transcript lands in the composer for you to read and send. Nothing is recorded before the click and nothing is kept. Recordings stop at ten minutes.

An attached audio file (WAV, MP3, M4A, OGG, WebM, FLAC, MP4) goes the same way: its transcript joins the composer.

### Pictures

When a picture is attached and the chat model cannot see:

1. A vision chat model (the saved one, else the automatic pick) describes it: every piece of visible text transcribed, tables as Markdown, the numbers in charts.
2. Without one, an OCR model (Mistral `POST /v1/ocr` with the picture as a data URL) reads its text as Markdown.
3. The text joins the message as untrusted data (it cannot give instructions), and a notice says which model read which file.
4. When nothing can read it, the picture is not sent and the notice says what is missing.

PDFs are read page by page by Tern Crawl as before.

## Not yet

- Per-role defaults in Settings (the keys exist; the picker sets them).
- Text to speech (listed, not used).
- Realtime transcription over WebSocket.
- OCR for scanned PDFs (pictures only today).
