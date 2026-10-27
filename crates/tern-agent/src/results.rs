//! Long tool results (#72): the model sees the head and the tail, and the
//! whole text is kept on disk for the chat so the model can read any part
//! of it with `read_result`.
//!
//! Before #72 a result was cut after its first 12,000 characters, so the
//! end of a long page or PDF (its conclusion, its references) was lost
//! for good. Now the middle is cut instead, with a line that says how
//! much is missing and how to read it, and the full text is saved as
//! `<data>/runs/<session>/results/<id>.txt`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use tern_core::EffectClass;

use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// Longest tool result the model is shown, in bytes. Errors are never cut.
pub const RESULT_LIMIT: usize = 12_000;
/// The share of [`RESULT_LIMIT`] given to the start of the text; the rest
/// goes to its end.
const HEAD_SHARE: f64 = 0.7;
/// The most `read_result` returns in one call, in characters.
pub const READ_MAX: usize = 12_000;

/// The text as the model sees it: all of it when it fits in `limit`
/// bytes, else its start and end with a note in between naming how many
/// characters were left out and, when the full text was saved, the
/// `read_result` call that reads them. Cuts fall on line ends when one is
/// near, and never inside a character.
pub fn head_tail(text: &str, limit: usize, saved_as: Option<&str>) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let head_bytes = ((limit as f64) * HEAD_SHARE) as usize;
    let tail_bytes = limit.saturating_sub(head_bytes);
    let head_end = line_cut_back(text, floor_boundary(text, head_bytes));
    let tail_start = line_cut_forward(text, ceil_boundary(text, text.len() - tail_bytes));
    let tail_start = tail_start.max(head_end);
    let head = &text[..head_end];
    let tail = &text[tail_start..];
    let total = text.chars().count();
    let skipped_from = head.chars().count();
    let skipped = total - skipped_from - tail.chars().count();
    let how = match saved_as {
        Some(id) => format!(
            " Read them with read_result {{\"id\": \"{id}\", \"offset\": {skipped_from}, \"length\": {}}}.",
            skipped.min(READ_MAX)
        ),
        None => String::new(),
    };
    format!(
        "{head}\n\n[… {skipped} of {total} characters cut from the middle (characters {skipped_from}–{}).{how} …]\n\n{tail}",
        skipped_from + skipped
    )
}

fn floor_boundary(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Moves a head cut back to just after a newline within the last 400
/// bytes, so a line is not split; stays put when there is none.
fn line_cut_back(s: &str, at: usize) -> usize {
    let from = floor_boundary(s, at.saturating_sub(400));
    match s[from..at].rfind('\n') {
        Some(i) => from + i + 1,
        None => at,
    }
}

/// Moves a tail cut forward to just after a newline within 400 bytes.
fn line_cut_forward(s: &str, at: usize) -> usize {
    let to = ceil_boundary(s, at.saturating_add(400));
    match s[at..to].find('\n') {
        Some(i) => at + i + 1,
        None => at,
    }
}

/// Where one chat's full tool results are kept.
#[derive(Clone, Debug)]
pub struct ResultStore {
    dir: PathBuf,
}

impl ResultStore {
    /// The store in `dir` (created on the first save).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The store for `session` under the app's data folder:
    /// `<data>/runs/<session>/results`.
    pub fn for_session(data_dir: &Path, session: tern_core::SessionId) -> Self {
        Self::at(
            data_dir
                .join("runs")
                .join(session.to_string())
                .join("results"),
        )
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Saves `text` as result `id`. Ids are made of letters, digits, `-`
    /// and `_` only.
    pub fn save(&self, id: &str, text: &str) -> std::io::Result<PathBuf> {
        let path = self.path(id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad result id")
        })?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(&path, text)?;
        Ok(path)
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        let ok = !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        ok.then(|| self.dir.join(format!("{id}.txt")))
    }

    /// Characters `offset..offset + length` of result `id` (at most
    /// [`READ_MAX`]), with a header naming the range and the total.
    pub fn read(&self, id: &str, offset: usize, length: usize) -> Result<String, String> {
        let path = self
            .path(id)
            .ok_or_else(|| format!("`{id}` is not a result id."))?;
        let text = std::fs::read_to_string(&path).map_err(|_| {
            format!("No saved result `{id}` in this chat. Use the id from the cut-off note.")
        })?;
        let total = text.chars().count();
        if offset >= total {
            return Err(format!(
                "Result `{id}` has {total} characters; offset {offset} is past its end."
            ));
        }
        let length = length.clamp(1, READ_MAX);
        let part: String = text.chars().skip(offset).take(length).collect();
        let end = offset + part.chars().count();
        let more = if end < total {
            format!(" Next: offset {end}.")
        } else {
            " That is the end.".to_owned()
        };
        Ok(format!(
            "Result `{id}`, characters {offset}–{end} of {total}.{more}\n{part}"
        ))
    }
}

/// The tool that reads a saved result: `read_result {id, offset, length}`.
#[derive(Clone, Debug)]
pub struct ReadResultTool(pub Arc<ResultStore>);

impl Tool for ReadResultTool {
    fn name(&self) -> &'static str {
        "read_result"
    }
    fn summary(&self) -> &'static str {
        "Read part of a long tool result that was cut (its id, a character offset, and a length, as the cut-off note says)."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Analysis
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["id"], "additionalProperties": false,
        "properties": {
            "id": {"type": "string"},
            "offset": {"type": "integer", "minimum": 0},
            "length": {"type": "integer", "minimum": 1, "maximum": READ_MAX}
        }})
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let id = args["id"].as_str().unwrap_or_default().to_owned();
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let length = args["length"].as_u64().unwrap_or(READ_MAX as u64) as usize;
            let store = self.0.clone();
            tokio::task::spawn_blocking(move || store.read(&id, offset, length))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?
                .map(ToolOutput::text)
                .map_err(ToolError::Failed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_results_are_left_whole() {
        assert_eq!(head_tail("abc", 10, Some("r1")), "abc");
    }

    #[test]
    fn long_results_keep_the_head_and_the_tail() {
        let mut text = String::new();
        for i in 0..2_000 {
            text.push_str(&format!("line {i}\n"));
        }
        text.push_str("CONCLUSION: the tail survives.\n");
        let shown = head_tail(&text, 2_000, Some("t1-3"));
        assert!(shown.len() < 2_400, "{}", shown.len());
        assert!(shown.starts_with("line 0\nline 1\n"));
        assert!(shown.ends_with("CONCLUSION: the tail survives.\n"));
        assert!(shown.contains("characters cut from the middle"));
        assert!(shown.contains(r#"read_result {"id": "t1-3", "offset": "#));
        // The cuts fall on line ends.
        let note = shown.find("\n\n[…").unwrap();
        assert!(shown[..note].ends_with('\n'));
        let after = shown.find("…]\n\n").unwrap() + "…]\n\n".len();
        assert!(shown[after..].starts_with("line "));
        // Without a saved copy there is no read_result hint.
        assert!(!head_tail(&text, 2_000, None).contains("read_result"));
    }

    #[test]
    fn cuts_never_split_a_character() {
        let text = "ہ".repeat(5_000) + &"é".repeat(5_000);
        let shown = head_tail(&text, 1_001, None);
        assert!(shown.starts_with('ہ') && shown.ends_with('é'));
    }

    #[test]
    fn the_numbers_in_the_note_match_what_was_cut() {
        let text: String = "x".repeat(10_000);
        let shown = head_tail(&text, 1_000, Some("a"));
        // 700 head + 300 tail of 10,000.
        assert!(
            shown
                .contains("[… 9000 of 10000 characters cut from the middle (characters 700–9700)."),
            "{shown}"
        );
        assert!(
            shown.contains(r#""offset": 700, "length": 9000"#),
            "{shown}"
        );
    }

    #[test]
    fn saved_results_can_be_read_in_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let store = ResultStore::at(dir.path().join("results"));
        let text = "abcdefghij".repeat(3);
        store.save("t1-1", &text).unwrap();
        let part = store.read("t1-1", 5, 10).unwrap();
        assert_eq!(
            part,
            "Result `t1-1`, characters 5–15 of 30. Next: offset 15.\nfghijabcde"
        );
        assert!(
            store
                .read("t1-1", 25, 100)
                .unwrap()
                .ends_with("That is the end.\nfghij")
        );
        assert!(store.read("t1-1", 30, 5).is_err());
        assert!(store.read("nope", 0, 5).is_err());
        assert!(store.read("../etc/passwd", 0, 5).is_err());
        assert!(store.save("../x", "no").is_err());
    }

    #[tokio::test]
    async fn the_tool_reads_a_range() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ResultStore::at(dir.path()));
        store.save("r", "hello world").unwrap();
        let tool = ReadResultTool(store);
        let out = tool
            .call(json!({"id": "r", "offset": 6, "length": 5}))
            .await
            .unwrap();
        assert!(out.content.ends_with("\nworld"), "{}", out.content);
        assert!(tool.call(json!({"id": "missing"})).await.is_err());
    }
}
