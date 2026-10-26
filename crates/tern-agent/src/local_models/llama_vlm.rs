//! Local picture understanding (#69): the vision models (SmolVLM,
//! Qwen3-VL) and Granite-Docling on llama.cpp's multimodal runtime.
//!
//! The runtime is a helper program, `tern-vlm`, built in CI from
//! llama.cpp's `llama-mtmd-cli` (pinned in `.github/scripts/build-vlm.sh`)
//! and shipped beside Tern (`vlm/tern-vlm[.exe]`, with its ggml backends:
//! Vulkan on Windows and Linux, Metal on macOS, and the CPU). Tern runs it
//! once per picture, reads the answer from its output, and stops it after
//! [`TIME_LIMIT`]. A helper process keeps a crash in the C++ runtime or a
//! GPU driver away from the app: a failed run is an error the chat shows
//! with "Use the alternative", never a crash.
//!
//! Device: the Settings › Speech device choice (Auto, GPU, CPU) applies
//! here too. Auto and GPU offload every layer to the GPU the helper finds
//! (`-ngl 99`); when that run fails, Tern runs it again on the CPU
//! (`--device none`). CPU threads: the same rule as Whisper (Auto, at
//! most four). Each run logs model, device, threads and time in tern.log.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{LocalEngine, ModelEntry, Runtime, Task};

/// The longest one picture may take (model load included).
pub const TIME_LIMIT: Duration = Duration::from_secs(240);

/// The longest answer, in tokens.
const MAX_TOKENS: &str = "512";

/// What Granite-Docling is asked (its own instruction).
pub const DOCLING_PROMPT: &str = "Convert this page to docling.";

/// The llama.cpp vision engine (the `tern-vlm` helper).
#[derive(Debug, Default)]
pub struct LlamaVlm;

/// Whether `entry` is a llama.cpp model with a vision projector: a
/// language model `.gguf` and an `mmproj-*.gguf`.
pub fn fits(entry: &ModelEntry) -> bool {
    entry.runtime == Runtime::LlamaCpp
        && matches!(entry.task, Task::Vision | Task::DocParse)
        && model_files_of(entry).is_some()
}

/// The language model and projector file names of `entry`.
fn model_files_of(entry: &ModelEntry) -> Option<(String, String)> {
    let mut model = None;
    let mut proj = None;
    for f in &entry.files {
        let name = f.local();
        let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
        if !base.ends_with(".gguf") {
            continue;
        }
        if base.starts_with("mmproj") {
            proj = Some(name.to_owned());
        } else {
            model = Some(name.to_owned());
        }
    }
    Some((model?, proj?))
}

/// The helper's file name on this system.
pub fn helper_name() -> &'static str {
    if cfg!(windows) {
        "tern-vlm.exe"
    } else {
        "tern-vlm"
    }
}

/// Where the helper is: `TERN_VLM` (a path), else `vlm/tern-vlm` or
/// `tern-vlm` beside the running program (on macOS also in the app's
/// `Resources/vlm`). `None` when it is not there.
pub fn helper() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TERN_VLM") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut places = vec![dir.join("vlm"), dir.to_path_buf()];
    if let Some(contents) = dir.parent() {
        places.push(contents.join("Resources").join("vlm"));
    }
    places
        .into_iter()
        .map(|d| d.join(helper_name()))
        .find(|p| p.is_file())
}

/// Why the helper is missing, for notices.
pub fn helper_missing() -> String {
    format!(
        "this build of Tern does not include the local vision helper ({}); vision models need a build that ships it",
        helper_name()
    )
}

/// One run of the helper: what it printed and on what it ran.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    /// "GPU · Vulkan0 (Intel(R) UHD Graphics)" or "CPU".
    pub device: String,
    pub secs: f64,
}

/// The device a run used, read from the helper's log lines.
pub fn device_from_log(log: &str) -> String {
    for line in log.lines() {
        if let Some(i) = line.find("using device ") {
            let rest = line[i + "using device ".len()..].trim();
            let name = rest.split(" - ").next().unwrap_or(rest).trim();
            if !name.is_empty() && !name.to_ascii_lowercase().starts_with("cpu") {
                return format!("GPU · {name}");
            }
        }
    }
    for line in log.lines() {
        let l = line.to_ascii_lowercase();
        if l.contains("offloaded") && l.contains("layers to gpu") && !l.contains("offloaded 0/") {
            return "GPU".into();
        }
    }
    "CPU".into()
}

/// The last meaningful error line of the helper's log.
fn error_from_log(log: &str) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("error") || l.contains("failed") || l.contains("unable")
        })
        .or(lines.last())
        .map(|l| l.chars().take(240).collect())
        .unwrap_or_else(|| "no output".into())
}

/// A picture as a file the helper reads: PNG and JPEG as they are, other
/// formats (GIF, WebP) re-encoded as PNG.
fn picture_file(image: &[u8]) -> Result<PathBuf, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let is_png = image.starts_with(b"\x89PNG");
    let is_jpeg = image.starts_with(&[0xFF, 0xD8]);
    let ext = if is_jpeg { "jpg" } else { "png" };
    let path = std::env::temp_dir().join(format!("tern-vlm-{}-{n}.{ext}", std::process::id()));
    if is_png || is_jpeg {
        std::fs::write(&path, image).map_err(|e| e.to_string())?;
    } else {
        let img = image::load_from_memory(image)
            .map_err(|e| format!("the picture could not be decoded: {e}"))?;
        img.save_with_format(&path, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Runs `cmd` to the end or [`TIME_LIMIT`]; returns (ok, stdout, stderr).
fn run_limited(mut cmd: Command, limit: Duration) -> Result<(bool, String, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window flashes over Tern.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("the vision helper could not start: {e}"))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut s);
        }
        String::from_utf8_lossy(&s).into_owned()
    });
    let err_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut s);
        }
        String::from_utf8_lossy(&s).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    match status {
        None => Err(format!(
            "the vision helper did not finish in {} s",
            limit.as_secs()
        )),
        Some(s) => Ok((s.success(), stdout, stderr)),
    }
}

/// The helper's answer without the echo and log noise some builds print
/// on standard output.
pub fn clean_answer(stdout: &str) -> String {
    let t = stdout.trim();
    // Older builds echo the prompt marker; drop anything up to it.
    let t = t.rsplit("<__media__>").next().unwrap_or(t);
    t.trim().to_owned()
}

impl LlamaVlm {
    /// Runs the helper on one picture with the model in `dir`.
    pub fn run(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        image: &[u8],
        prompt: &str,
    ) -> Result<Run, String> {
        let helper = helper().ok_or_else(helper_missing)?;
        let (model, proj) = model_files_of(entry)
            .ok_or_else(|| format!("{} has no vision projector file", entry.name))?;
        let (model, proj) = (dir.join(model), dir.join(proj));
        for f in [&model, &proj] {
            if !f.is_file() {
                return Err(format!("{} is missing {}", entry.name, f.display()));
            }
        }
        let pic = picture_file(image)?;
        let threads = super::speech::threads();
        let device = super::speech::choice().device;
        let tries: &[bool] = match device {
            super::speech::Device::Cpu => &[false],
            _ => &[true, false],
        };
        let mut last_err = String::new();
        for &gpu in tries {
            let mut cmd = Command::new(&helper);
            cmd.arg("-m")
                .arg(&model)
                .arg("--mmproj")
                .arg(&proj)
                .arg("--image")
                .arg(&pic)
                .arg("-p")
                .arg(prompt)
                .args(["-n", MAX_TOKENS, "--temp", "0", "-c", "8192"])
                .args(["-t", &threads.to_string()]);
            if gpu {
                cmd.args(["-ngl", "99"]);
            } else {
                cmd.args(["-ngl", "0", "--device", "none", "--no-mmproj-offload"]);
            }
            let t0 = Instant::now();
            let got = run_limited(cmd, TIME_LIMIT);
            let secs = t0.elapsed().as_secs_f64();
            match got {
                Ok((true, out, log)) => {
                    let text = clean_answer(&out);
                    let device = if gpu {
                        device_from_log(&log)
                    } else {
                        "CPU".into()
                    };
                    crate::diag::note(&format!(
                        "vlm: model={} device={device} threads={threads} took={secs:.1}s chars={}",
                        entry.id,
                        text.chars().count()
                    ));
                    let _ = std::fs::remove_file(&pic);
                    if text.is_empty() {
                        return Err(format!("{} gave no answer", entry.name));
                    }
                    return Ok(Run { text, device, secs });
                }
                Ok((false, _, log)) => {
                    last_err = error_from_log(&log);
                    crate::diag::note(&format!(
                        "vlm: model={} gpu={gpu} failed after {secs:.1}s: {last_err}",
                        entry.id
                    ));
                }
                Err(e) => {
                    crate::diag::note(&format!(
                        "vlm: model={} gpu={gpu} failed after {secs:.1}s: {e}",
                        entry.id
                    ));
                    last_err = e;
                    // A run that hit the time limit on the GPU is not
                    // tried again on the slower CPU.
                    if last_err.contains("did not finish") {
                        break;
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&pic);
        Err(format!("{} could not run: {last_err}", entry.name))
    }
}

impl LocalEngine for LlamaVlm {
    fn runtime(&self) -> Runtime {
        Runtime::LlamaCpp
    }

    fn name(&self) -> &'static str {
        "llama.cpp vision"
    }

    fn runs(&self, entry: &ModelEntry) -> bool {
        fits(entry) && helper().is_some()
    }

    fn describe(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        image: &[u8],
        prompt: &str,
    ) -> Result<String, String> {
        self.run(dir, entry, image, prompt).map(|r| r.text)
    }

    fn page_markdown(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        image: &[u8],
    ) -> Result<String, String> {
        let prompt = if entry.id.contains("docling") {
            DOCLING_PROMPT
        } else {
            "Convert this page to Markdown. Keep headings, lists and tables; transcribe the text exactly."
        };
        let r = self.run(dir, entry, image, prompt)?;
        Ok(if r.text.contains("<loc_") || r.text.contains("<doctag>") {
            doctags_to_markdown(&r.text)
        } else {
            r.text
        })
    }
}

/// Granite-Docling's DocTags as Markdown: headings, paragraphs, list
/// items, captions, code, formulas, and OTSL tables (`<fcel>`/`<ecel>`/
/// `<nl>`); location tags, page headers and footers are dropped.
pub fn doctags_to_markdown(doctags: &str) -> String {
    let loc = regex::Regex::new(r"<loc_\d+>").expect("static regex");
    let s = loc.replace_all(doctags, "");
    let block = regex::Regex::new(
        r"(?s)<(title|section_header_level_\d|text|paragraph|list_item|caption|footnote|code|formula|otsl|page_header|page_footer)>(.*?)</(?:title|section_header_level_\d|text|paragraph|list_item|caption|footnote|code|formula|otsl|page_header|page_footer)>",
    )
    .expect("static regex");
    let mut out: Vec<String> = Vec::new();
    for c in block.captures_iter(&s) {
        let (tag, body) = (&c[1], c[2].trim());
        if body.is_empty() {
            continue;
        }
        let line = match tag {
            "title" => format!("# {}", strip_tags(body)),
            t if t.starts_with("section_header") => {
                let level = t
                    .rsplit('_')
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 5);
                format!("{} {}", "#".repeat(level + 1), strip_tags(body))
            }
            "list_item" => format!("- {}", strip_tags(body)),
            "code" => format!("```\n{}\n```", strip_tags(body)),
            "formula" => format!("$$ {} $$", strip_tags(body)),
            "otsl" => otsl_table(body),
            "caption" | "footnote" => format!("_{}_", strip_tags(body)),
            "page_header" | "page_footer" => continue,
            _ => strip_tags(body),
        };
        if !line.trim().is_empty() {
            out.push(line);
        }
    }
    if out.is_empty() {
        return strip_tags(&s).trim().to_owned();
    }
    out.join("\n\n")
}

fn strip_tags(s: &str) -> String {
    let tag = regex::Regex::new(r"<[^>]+>").expect("static regex");
    tag.replace_all(s, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// An OTSL table body as a Markdown table.
fn otsl_table(body: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in body.split("<nl>") {
        let mut cells = Vec::new();
        let cell = regex::Regex::new(r"<(fcel|ecel|ched|rhed|srow|lcel|ucel|xcel)>([^<]*)")
            .expect("static regex");
        for c in cell.captures_iter(row) {
            match &c[1] {
                "lcel" | "ucel" | "xcel" => cells.push(String::new()),
                _ => cells.push(c[2].trim().replace('|', "\\|")),
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mut r = r.clone();
        r.resize(width, String::new());
        out.push(format!("| {} |", r.join(" | ")));
        if i == 0 {
            out.push(format!("|{}", " --- |".repeat(width)));
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Manifest;

    #[test]
    fn runs_the_vision_and_docling_models() {
        let m = Manifest::builtin();
        let ids: Vec<&str> = m
            .models
            .iter()
            .filter(|e| fits(e))
            .map(|e| e.id.as_str())
            .collect();
        for want in [
            "smolvlm-500m",
            "smolvlm-256m",
            "qwen3-vl-2b",
            "granite-docling-258m",
        ] {
            assert!(ids.contains(&want), "{want} missing from {ids:?}");
        }
        assert!(!ids.contains(&"embeddinggemma-2-q8"));
    }

    #[test]
    fn the_device_is_read_from_the_log() {
        let log = "llama_model_load_from_file_impl: using device Vulkan0 (Intel(R) UHD Graphics) - 7000 MiB free\n";
        assert_eq!(
            device_from_log(log),
            "GPU · Vulkan0 (Intel(R) UHD Graphics)"
        );
        assert_eq!(
            device_from_log("load_tensors: offloaded 0/33 layers to GPU"),
            "CPU"
        );
        assert_eq!(device_from_log("nothing here"), "CPU");
    }

    #[test]
    fn doctags_become_markdown() {
        let d = "<doctag><title><loc_1><loc_2>Invoice</title><section_header_level_1>Items</section_header_level_1><text>Paid in full.</text><otsl><ched>Item<ched>Total<nl><fcel>Tea<fcel>4,250<nl></otsl><list_item>Thanks</list_item></doctag>";
        let md = doctags_to_markdown(d);
        assert!(md.contains("# Invoice"), "{md}");
        assert!(md.contains("## Items"), "{md}");
        assert!(md.contains("Paid in full."), "{md}");
        assert!(md.contains("| Item | Total |"), "{md}");
        assert!(md.contains("| Tea | 4,250 |"), "{md}");
        assert!(md.contains("- Thanks"), "{md}");
    }

    #[test]
    fn answers_drop_the_prompt_echo() {
        assert_eq!(
            clean_answer("<__media__>Describe\n A cat. \n"),
            "Describe\n A cat."
        );
        assert_eq!(clean_answer("  A red square.\n"), "A red square.");
    }

    #[test]
    fn a_missing_helper_is_an_error_not_a_panic() {
        let m = Manifest::builtin();
        let e = m.find("smolvlm-500m").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let r = LlamaVlm.run(dir.path(), e, b"\x89PNG", "hi");
        assert!(r.is_err());
    }
}
