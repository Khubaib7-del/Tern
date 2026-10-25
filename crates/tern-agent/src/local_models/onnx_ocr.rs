//! The first linked runtime: PaddleOCR models (PP-OCRv6, PP-OCRv5) on
//! ONNX Runtime through the pure-Rust `oar-ocr` crate. Built with the
//! `ocr-onnx` feature; [`super::register_builtin_engines`] turns it on.
//!
//! A model folder holds `det/` (the text detector) and `rec/` (the
//! recogniser), each an `inference.onnx` with PaddleOCR's `inference.yml`
//! (thresholds, character dictionary), and optionally `rec-arabic/` (the
//! Arabic-script recogniser, for Urdu). With both recognisers each
//! detected line keeps the reading the models were surer of.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use oar_ocr::domain::tasks::TextDetectionConfig;
use oar_ocr::prelude::{OAROCR, OAROCRBuilder};

use super::{LocalEngine, ModelEntry, Runtime, Task, paddle};

/// Reads pictures with PaddleOCR ONNX models.
#[derive(Default)]
pub struct OnnxOcr {
    /// Loaded pipelines by model folder, so a model loads once.
    loaded: Mutex<HashMap<PathBuf, Arc<Pipelines>>>,
}

impl std::fmt::Debug for OnnxOcr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.loaded.lock().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("OnnxOcr").field("loaded", &n).finish()
    }
}

struct Pipelines {
    main: OAROCR,
    arabic: Option<OAROCR>,
}

/// Whether `entry` is laid out as this engine expects.
pub fn fits(entry: &ModelEntry) -> bool {
    let has = |p: &str| entry.files.iter().any(|f| f.local() == p);
    entry.task == Task::Ocr
        && entry.runtime == Runtime::Onnx
        && has("det/inference.onnx")
        && has("rec/inference.onnx")
        && has("rec/inference.yml")
}

fn pipeline(dir: &Path, rec: &str) -> Result<OAROCR, String> {
    let read = |p: PathBuf| {
        std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))
    };
    let dict = paddle::character_dict(&read(dir.join(rec).join("inference.yml"))?)?;
    let (t, b, u, m) = std::fs::read_to_string(dir.join("det/inference.yml"))
        .map(|y| paddle::det_thresholds(&y))
        .unwrap_or_default();
    let det = TextDetectionConfig {
        score_threshold: t.unwrap_or(0.2),
        box_threshold: b.unwrap_or(0.45),
        unclip_ratio: u.unwrap_or(1.4),
        max_candidates: m.unwrap_or(3000),
        ..TextDetectionConfig::default()
    };
    OAROCRBuilder::new(
        dir.join("det/inference.onnx"),
        dir.join(rec).join("inference.onnx"),
        dir.join(rec).join("dict.txt"),
    )
    .character_dict_content(dict.join("\n"))
    .text_detection_config(det)
    .build()
    .map_err(|e| format!("cannot load the OCR model: {e}"))
}

impl OnnxOcr {
    fn pipelines(&self, dir: &Path) -> Result<Arc<Pipelines>, String> {
        let mut map = self
            .loaded
            .lock()
            .map_err(|_| "the OCR engine is unusable")?;
        if let Some(p) = map.get(dir) {
            return Ok(p.clone());
        }
        let arabic = if dir.join("rec-arabic/inference.onnx").exists() {
            Some(pipeline(dir, "rec-arabic")?)
        } else {
            None
        };
        let p = Arc::new(Pipelines {
            main: pipeline(dir, "rec")?,
            arabic,
        });
        map.insert(dir.to_owned(), p.clone());
        Ok(p)
    }
}

fn lines(ocr: &OAROCR, image: image::RgbImage) -> Result<Vec<(String, f32)>, String> {
    let out = ocr
        .predict(vec![image])
        .map_err(|e| format!("OCR failed: {e}"))?;
    Ok(out
        .into_iter()
        .flat_map(|r| r.text_regions)
        .filter_map(|r| {
            r.text_with_confidence()
                .map(|(t, c)| (t.trim().to_owned(), c))
        })
        .collect())
}

impl LocalEngine for OnnxOcr {
    fn runtime(&self) -> Runtime {
        Runtime::Onnx
    }

    fn runs(&self, entry: &ModelEntry) -> bool {
        fits(entry)
    }

    fn ocr(&self, dir: &Path, entry: &ModelEntry, image: &[u8]) -> Result<String, String> {
        if !fits(entry) {
            return Err(format!("{} is not a PaddleOCR model", entry.name));
        }
        let img = image::load_from_memory(image)
            .map_err(|e| format!("the picture cannot be decoded: {e}"))?
            .to_rgb8();
        let p = self.pipelines(dir)?;
        let main = lines(&p.main, img.clone())?;
        let picked = match &p.arabic {
            Some(ar) => {
                let urdu: Vec<(String, f32)> = lines(ar, img)?
                    .into_iter()
                    .map(|(t, c)| (paddle::rtl_reading_order(&t), c))
                    .collect();
                paddle::pick_lines(&main, &urdu)
            }
            None => main.into_iter().map(|(t, _)| t).collect(),
        };
        Ok(picked
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Manifest;

    #[test]
    fn runs_only_the_paddle_ocr_models() {
        let m = Manifest::builtin();
        let runs: Vec<&str> = m
            .models
            .iter()
            .filter(|e| fits(e))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(runs, ["pp-ocrv6-small"]);
    }

    /// Reads a real picture with the downloaded PP-OCRv6 model. Needs
    /// `TERN_OCR_MODEL_DIR` (an installed `pp-ocrv6-small` folder) and
    /// `TERN_OCR_IMAGE`; run with `--ignored`.
    #[test]
    #[ignore]
    fn reads_a_picture_with_the_installed_model() {
        let dir = PathBuf::from(std::env::var("TERN_OCR_MODEL_DIR").unwrap());
        let image = std::fs::read(std::env::var("TERN_OCR_IMAGE").unwrap()).unwrap();
        let m = Manifest::builtin();
        let entry = m.find("pp-ocrv6-small").unwrap();
        let text = OnnxOcr::default().ocr(&dir, entry, &image).unwrap();
        println!("{text}");
        if let Ok(want) = std::env::var("TERN_OCR_EXPECT") {
            for w in want.split('|') {
                assert!(text.contains(w), "{w:?} not in {text:?}");
            }
        }
    }
}
