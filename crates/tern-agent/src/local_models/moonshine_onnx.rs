//! Moonshine Streaming on ONNX Runtime (the `asr-onnx` feature): fast
//! English speech to text for live dictation. ONNX Runtime is the one
//! already linked for OCR, so this adds no library.
//!
//! The model folder is a transformers.js-style export: `onnx/encoder_model*.onnx`
//! (raw 16 kHz samples in, hidden states out), `onnx/decoder_model_merged*.onnx`
//! (one token at a time with a key/value cache, `use_cache_branch` off on
//! the first step), `config.json` (layer and head counts), and
//! `tokenizer.json` (a SentencePiece-style BPE vocabulary). Decoding is
//! greedy, capped at 6.5 tokens a second of audio as the model card asks.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};

use super::{LocalEngine, ModelEntry, Runtime, Task};

/// Runs Moonshine models.
#[derive(Default)]
pub struct MoonshineOnnx {
    loaded: Mutex<Option<(PathBuf, Arc<Mutex<Model>>)>>,
}

impl std::fmt::Debug for MoonshineOnnx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoonshineOnnx").finish_non_exhaustive()
    }
}

struct Model {
    encoder: Session,
    decoder: Session,
    vocab: Vocab,
    layers: usize,
    heads: usize,
    head_dim: usize,
    bos: i64,
    eos: i64,
}

fn file<'a>(entry: &'a ModelEntry, prefix: &str) -> Option<&'a str> {
    entry
        .files
        .iter()
        .map(|f| f.local())
        .find(|n| n.starts_with(prefix) && n.ends_with(".onnx"))
}

/// Whether `entry` is a Moonshine export this engine runs.
pub fn fits(entry: &ModelEntry) -> bool {
    let has = |p: &str| entry.files.iter().any(|f| f.local() == p);
    entry.runtime == Runtime::Onnx
        && matches!(entry.task, Task::Dictation | Task::SpeechToText)
        && file(entry, "onnx/encoder_model").is_some()
        && file(entry, "onnx/decoder_model_merged").is_some()
        && has("tokenizer.json")
        && has("config.json")
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 8))
        .unwrap_or(4)
}

fn err(e: impl std::fmt::Display) -> String {
    format!("Moonshine failed: {e}")
}

fn session(path: &Path) -> Result<Session, String> {
    Session::builder()
        .map_err(err)?
        .with_intra_threads(threads())
        .map_err(err)?
        .commit_from_file(path)
        .map_err(|e| format!("cannot load {}: {e}", path.display()))
}

impl Model {
    fn load(dir: &Path, entry: &ModelEntry) -> Result<Self, String> {
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).map_err(err)?)
                .map_err(err)?;
        let num = |keys: &[&str], d: u64| {
            keys.iter()
                .find_map(|k| cfg.get(*k).and_then(|v| v.as_u64()))
                .unwrap_or(d) as usize
        };
        let vocab =
            Vocab::parse(&std::fs::read_to_string(dir.join("tokenizer.json")).map_err(err)?)?;
        let enc = file(entry, "onnx/encoder_model").ok_or("no encoder")?;
        let dec = file(entry, "onnx/decoder_model_merged").ok_or("no decoder")?;
        Ok(Self {
            encoder: session(&dir.join(enc))?,
            decoder: session(&dir.join(dec))?,
            vocab,
            layers: num(&["decoder_num_hidden_layers", "num_hidden_layers"], 10),
            heads: num(&["decoder_num_key_value_heads", "num_key_value_heads"], 8),
            head_dim: num(&["head_dim"], 64),
            bos: num(&["decoder_start_token_id", "bos_token_id"], 1) as i64,
            eos: num(&["eos_token_id"], 2) as i64,
        })
    }

    fn empty_cache(&self) -> Result<DynValue, String> {
        Tensor::<f32>::from_array(([1usize, self.heads, 0, self.head_dim], Vec::new()))
            .map(|t| t.into_dyn())
            .map_err(err)
    }

    /// Token ids for one stretch of speech.
    fn tokens(&mut self, pcm: &[f32]) -> Result<Vec<i64>, String> {
        let mut x = pcm.to_vec();
        x.resize(x.len().div_ceil(80).max(1) * 80, 0.0);
        let n = x.len();
        let input = Tensor::from_array(([1usize, n], x)).map_err(err)?;
        let mut out = self
            .encoder
            .run(ort::inputs!["input_values" => input])
            .map_err(err)?;
        let hidden = out
            .remove("last_hidden_state")
            .ok_or("the encoder gave no hidden states")?;
        drop(out);
        let names: Vec<(String, String)> = (0..self.layers)
            .flat_map(|l| {
                [
                    "decoder.key",
                    "decoder.value",
                    "encoder.key",
                    "encoder.value",
                ]
                .map(|k| {
                    (
                        format!("past_key_values.{l}.{k}"),
                        format!("present.{l}.{k}"),
                    )
                })
            })
            .collect();
        let mut past: Vec<DynValue> = names
            .iter()
            .map(|_| self.empty_cache())
            .collect::<Result<_, _>>()?;
        let max = ((pcm.len() as f64 * 6.5 / 16_000.0).ceil() as usize).max(8);
        let mut ids = Vec::new();
        let mut tok = self.bos;
        for step in 0..max {
            let mut inputs: Vec<(Cow<str>, SessionInputValue)> = vec![
                (
                    "input_ids".into(),
                    Tensor::from_array(([1usize, 1], vec![tok]))
                        .map_err(err)?
                        .into(),
                ),
                ("encoder_hidden_states".into(), (&hidden).into()),
                (
                    "use_cache_branch".into(),
                    Tensor::from_array(([1usize], vec![step > 0]))
                        .map_err(err)?
                        .into(),
                ),
            ];
            for ((name, _), v) in names.iter().zip(&past) {
                inputs.push((name.as_str().into(), v.into()));
            }
            let mut out = self.decoder.run(inputs).map_err(err)?;
            let (shape, logits) = out["logits"].try_extract_tensor::<f32>().map_err(err)?;
            let v = *shape.last().ok_or("logits have no shape")? as usize;
            let last = &logits[logits.len().saturating_sub(v)..];
            let next = last
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i as i64)
                .ok_or("empty logits")?;
            for (i, (name, present)) in names.iter().enumerate() {
                // The cross-attention cache is made once, on the first step.
                if step > 0 && name.contains(".encoder.") {
                    continue;
                }
                past[i] = out
                    .remove(present.as_str())
                    .ok_or_else(|| format!("the decoder gave no {present}"))?;
            }
            if next == self.eos {
                break;
            }
            ids.push(next);
            tok = next;
        }
        Ok(ids)
    }
}

/// A SentencePiece-style BPE vocabulary (`▁` for spaces, `<0xNN>` byte
/// pieces), enough to turn ids back into text.
#[derive(Debug, Default)]
pub struct Vocab {
    pieces: Vec<String>,
}

impl Vocab {
    /// Reads `tokenizer.json`'s `model.vocab` and added tokens.
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(err)?;
        let mut pieces: Vec<String> = Vec::new();
        let mut put = |id: usize, s: &str| {
            if pieces.len() <= id {
                pieces.resize(id + 1, String::new());
            }
            pieces[id] = s.to_owned();
        };
        let vocab = v
            .pointer("/model/vocab")
            .and_then(|m| m.as_object())
            .ok_or("tokenizer.json has no vocabulary")?;
        for (piece, id) in vocab {
            if let Some(id) = id.as_u64() {
                put(id as usize, piece);
            }
        }
        for t in v
            .get("added_tokens")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            if let (Some(id), Some(s)) = (
                t.get("id").and_then(|i| i.as_u64()),
                t.get("content").and_then(|c| c.as_str()),
            ) {
                // Special tokens (`<s>`, `<<ST_0>>`) never reach the text.
                put(id as usize, &format!("\u{0}{s}"));
            }
        }
        Ok(Self { pieces })
    }

    /// The text for `ids`.
    pub fn decode(&self, ids: &[i64]) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            let Some(p) = usize::try_from(id).ok().and_then(|i| self.pieces.get(i)) else {
                continue;
            };
            if p.starts_with('\u{0}') {
                continue;
            }
            if let Some(hex) = p.strip_prefix("<0x").and_then(|h| h.strip_suffix('>'))
                && let Ok(b) = u8::from_str_radix(hex, 16)
            {
                bytes.push(b);
                continue;
            }
            bytes.extend(p.replace('\u{2581}', " ").as_bytes());
        }
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }
}

impl MoonshineOnnx {
    fn model(&self, dir: &Path, entry: &ModelEntry) -> Result<Arc<Mutex<Model>>, String> {
        let mut slot = self
            .loaded
            .lock()
            .map_err(|_| "the Moonshine engine is unusable")?;
        if let Some((p, m)) = slot.as_ref()
            && p == dir
        {
            return Ok(m.clone());
        }
        let m = Arc::new(Mutex::new(Model::load(dir, entry)?));
        *slot = Some((dir.to_owned(), m.clone()));
        Ok(m)
    }
}

/// Longest stretch given to the model at once; longer audio is cut.
const CHUNK: usize = 16_000 * 24;

impl LocalEngine for MoonshineOnnx {
    fn runtime(&self) -> Runtime {
        Runtime::Onnx
    }

    fn name(&self) -> &'static str {
        "moonshine"
    }

    fn runs(&self, entry: &ModelEntry) -> bool {
        fits(entry)
    }

    fn transcribe(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        audio: &[f32],
        _language: Option<&str>,
    ) -> Result<String, String> {
        if !fits(entry) {
            return Err(format!("{} is not a Moonshine model", entry.name));
        }
        let m = self.model(dir, entry)?;
        let mut m = m.lock().map_err(|_| "the Moonshine model is unusable")?;
        let mut parts = Vec::new();
        for chunk in audio.chunks(CHUNK) {
            if chunk.len() < 1_600 {
                continue;
            }
            let ids = m.tokens(chunk)?;
            let text = m.vocab.decode(&ids);
            if !text.is_empty() {
                parts.push(text);
            }
        }
        Ok(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Manifest;

    #[test]
    fn runs_the_moonshine_model_only() {
        let m = Manifest::builtin();
        let runs: Vec<&str> = m
            .models
            .iter()
            .filter(|e| fits(e))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(runs, ["moonshine-streaming-small"]);
    }

    #[test]
    fn vocab_decodes_spaces_bytes_and_skips_specials() {
        let json = r#"{"model":{"type":"BPE","vocab":{"<unk>":0,"<s>":1,"</s>":2,"<0xC3>":3,"<0xA9>":4,"▁caf":5,"▁ask":6,"▁not":7}},
            "added_tokens":[{"id":1,"content":"<s>"},{"id":8,"content":"<<ST_0>>"}]}"#;
        let v = Vocab::parse(json).unwrap();
        assert_eq!(v.decode(&[1, 6, 7, 8]), "ask not");
        assert_eq!(v.decode(&[5, 3, 4]), "café");
    }

    /// Transcribes a real file with a downloaded Moonshine model. Needs
    /// `TERN_MOONSHINE_DIR` and `TERN_AUDIO`; run with `--ignored`.
    #[test]
    #[ignore]
    fn transcribes_with_the_installed_model() {
        let dir = PathBuf::from(std::env::var("TERN_MOONSHINE_DIR").unwrap());
        let path = std::env::var("TERN_AUDIO").unwrap();
        let pcm =
            crate::local_models::audio::decode_16k_mono(&std::fs::read(&path).unwrap(), &path)
                .unwrap();
        let m = Manifest::builtin();
        let entry = m.find("moonshine-streaming-small").unwrap();
        let t = std::time::Instant::now();
        let text = MoonshineOnnx::default()
            .transcribe(&dir, entry, &pcm, None)
            .unwrap();
        println!("{text} ({:.2} s)", t.elapsed().as_secs_f32());
        if let Ok(want) = std::env::var("TERN_EXPECT") {
            for w in want.split('|') {
                assert!(
                    text.to_lowercase().contains(&w.to_lowercase()),
                    "{w:?} not in {text:?}"
                );
            }
        }
    }
}
