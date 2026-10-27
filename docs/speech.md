# Local speech: GPU and CPU (#67)

Tern transcribes attached audio files with **Whisper** on [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (through `whisper-rs`) and the mic with Moonshine Streaming on ONNX Runtime. This page is about Whisper: where it runs and how fast.

## Where Whisper runs

| Build | GPU backend | Falls back to |
| --- | --- | --- |
| Windows x64 | Vulkan (any GPU with a Vulkan 1.2 driver: NVIDIA, AMD, Intel) | CPU |
| Linux x64 | Vulkan | CPU |
| macOS arm64 | Metal | CPU |

**Settings › Speech** has two choices, saved in `preferences.json`:

- **Device**: **Auto** (default) uses the first GPU ggml finds and the CPU otherwise; **GPU** always tries the GPU first; **CPU** never uses it. `TERN_SPEECH_DEVICE=auto|gpu|cpu` sets it for the terminal (`tern models transcribe`) when the saved choice is Auto.
- **CPU threads**: **Auto** or 2, 4, 6, 8. Auto is one thread per logical core, at most twice the physical cores and **at most four**. The saved choice wins over `TERN_WHISPER_THREADS`, which wins over Auto.

The page also shows the GPU found (or why there is none) and what the last transcription ran on.

Every transcription writes one line to `tern.log` (`<data>/logs/tern.log`):

```
whisper: engine=whisper.cpp gpu=vulkan Intel(R) UHD Graphics 620
whisper: engine=whisper.cpp cpu 4 threads (no Vulkan driver (vulkan-1.dll not found))
```

and the GPU probe one line, `speech: gpu probe: …`.

## Dictation (#68)

The mic's live dictation picks its engine from **Settings › Speech › Dictation language**. The choice is saved as `dictation_language` in `preferences.json`.

- **English**: Moonshine Streaming, phrase by phrase, with at most 4 threads.
- **Any of the other 17 languages** (Urdu, Arabic, Hindi, Persian, Chinese, Spanish…): Whisper, re-run every 2–5 s over the open window. The words show grey until the window closes at a pause.
- **Auto** (default): the first window goes to Whisper, which detects the language. English then moves to Moonshine, and anything else stays on Whisper.

Moonshine knows only English. Given Urdu, it writes English that was never said, such as "You will not be able to do anything" over and over. The speech guard (`crates/tern-agent/src/speech_guard.rs`) keeps that out of the composer. It drops runs of the same words, a sentence repeated from the window before, and the phrases models write on silence ("Thank you.", "Thanks for watching", "Subtitles by the Amara.org community"). A window with no speech counts as a pause, so it shows no error. Dictated text goes in at the caret, and the caret moves with it.

### Auto, from #70

- **Live text at once**: Auto streams with the fast model straight away, so words appear while you speak.
- **The language is checked once without transcribing** (Whisper's own language detection), and again on a new phrase at most every 10 s. With the optional **Whisper tiny** add-on downloaded (Settings › Local models › Transcribe audio files › Whisper tiny (q5_1), language check › **Add**, 31 MB) the check takes about a second; without it the installed Whisper model checks.
- **Urdu (or any language but English)** restarts the open phrase on Whisper, so English never sits over Urdu speech.
- Windows with no speech are never sent; the detected language is kept for 2 minutes; sound tags like `[inaudible]` or `(music)` are dropped.

**Last transcription** in Settings › Speech names the model, the device (GPU or CPU and threads) and how long it took, for every engine.

## Read aloud (#69)

Kokoro 82M reads answers aloud on the CPU (ONNX Runtime, at most four threads), through the speakers with cpal (WASAPI, CoreAudio, ALSA). **Settings › Speech › Read aloud** picks the voice (Heart, Michael, Emma, George) and the speed (0.8× to 1.3×). Long sentences are spoken in clause-sized pieces (#70), so the first audio comes sooner, and `tern.log` says how fast synthesis ran against real time. English only: an Urdu answer gets a note. See [local-models.md](local-models.md#read-aloud-kokoro).

The same **Device** setting decides where `tern-vlm` (the local picture models) runs: Auto and GPU try the GPU first, then the CPU. Since #70 the `vlm:` line in `tern.log` says why a run went to the CPU.

## Fallback to the CPU

- **No GPU, no driver**: ggml's Vulkan backend registers no device and Whisper runs on the CPU.
- **Windows without a Vulkan driver**: `vulkan-1.dll` (the Vulkan loader that GPU drivers install) is **delay-loaded** (`/DELAYLOAD:vulkan-1.dll` in `build.rs` of `tern-desktop`, `tern-tui` and `tern-agent`'s tests), so Tern starts without it. When ggml first calls Vulkan and the DLL is missing, a delay-load failure hook (`tern_agent::local_models::gpu`) hands it a stand-in loader whose every call fails with `VK_ERROR_INITIALIZATION_FAILED`; ggml catches that and registers no Vulkan device. CI checks the delay-load (`.github/scripts/check-delayload.py`), hides the runner's own `System32\vulkan-1.dll` (the image ships the loader without a GPU), then starts `tern-desktop.exe` and runs the speech benchmark without it.
- **The GPU cannot load the model**: Whisper loads it again on the CPU.
- **The GPU takes the app down**: a marker file (`<data>/logs/speech-gpu-trial`) is written before a GPU run and removed after it. Found at the next run, Auto stays on the CPU and Settings › Speech says so; choosing **GPU** tries again.

## Building

The Vulkan backend compiles its shaders at build time and needs the **Vulkan SDK** (headers, `glslc`; on Windows also `vulkan-1.lib` and the `VULKAN_SDK` variable). CI installs it with `jakoch/install-vulkan-sdk-action` (cached); on Windows without the runtime, on purpose. Locally on Linux: `libvulkan-dev` and `glslc` (or the LunarG SDK); on Windows: the LunarG SDK. macOS builds Metal with no extra install (the shaders are embedded and compiled at run time).

## Speed

See the speech benchmark in each CI run's job summary (`.github/scripts/speech-bench.sh`): Whisper small and Moonshine on an 11 s and a 12 s clip, CPU threads 2 and 4 with flash attention on and off, and the device rows (Auto, GPU asked for, and on Linux Mesa's lavapipe standing in for a GPU). CI runners have no GPU, so GPU speed is not measured there; lavapipe proves the Vulkan path works end to end, not its speed.
