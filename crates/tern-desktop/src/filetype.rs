//! File kinds for attachment preview cards (#71): whether the card shows
//! a picture, a first page of text, or only its kind (the kind's logo is
//! `crate::fileicon`, #72).

/// What kind of file an attachment is, from its name (and whether it
/// carries a picture).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Pdf,
    Image,
    Word,
    Sheet,
    Slides,
    Markdown,
    Text,
    Csv,
    Python,
    Notebook,
    Code,
    Audio,
    Folder,
    Archive,
    Link,
    Other,
}

impl FileKind {
    /// The kind of a file named `name`.
    pub fn of(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "pdf" => Self::Pdf,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "svg" => {
                Self::Image
            }
            "doc" | "docx" | "odt" | "rtf" => Self::Word,
            "xls" | "xlsx" | "ods" => Self::Sheet,
            "ppt" | "pptx" | "odp" | "key" => Self::Slides,
            "md" | "markdown" => Self::Markdown,
            "txt" | "log" | "text" => Self::Text,
            "csv" | "tsv" => Self::Csv,
            "py" | "pyw" | "pyi" => Self::Python,
            "ipynb" => Self::Notebook,
            "rs" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cc"
            | "java" | "kt" | "go" | "rb" | "php" | "cs" | "swift" | "sh" | "ps1" | "bat"
            | "html" | "htm" | "css" | "json" | "toml" | "yaml" | "yml" | "xml" | "sql" | "lua"
            | "r" | "m" | "scala" | "dart" | "zig" | "asm" | "s" => Self::Code,
            "mp3" | "wav" | "m4a" | "ogg" | "flac" | "opus" | "aac" | "wma" => Self::Audio,
            "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "xz" | "bz2" => Self::Archive,
            _ => Self::Other,
        }
    }

    /// Whether the card shows the file's first lines as a little page
    /// (text it was read as), when there is no picture.
    pub fn shows_page(self) -> bool {
        !matches!(
            self,
            Self::Image | Self::Audio | Self::Folder | Self::Archive | Self::Other
        )
    }
}

/// The first lines of an attachment's text for its card's page preview:
/// up to `lines` non-empty lines, each at most 60 characters, without the
/// conversion header (`[Converted … ]`) and page markers.
pub fn first_lines(text: &str, lines: usize) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !(l.starts_with('[') && l.ends_with(']')))
        .filter(|l| !l.starts_with("<!--"))
        .filter(|l| !l.starts_with("```") && !l.starts_with("---"))
        .take(lines)
        .map(|l| {
            let l = l.trim_start_matches('#').trim();
            if l.chars().count() > 60 {
                let cut: String = l.chars().take(59).collect();
                format!("{cut}…")
            } else {
                l.to_owned()
            }
        })
        .collect()
}

/// The lines a card previews for `text` of kind `kind`: a notebook's
/// cell sources (its raw JSON says nothing at a glance), else
/// [`first_lines`].
pub fn preview_lines(kind: FileKind, text: &str, lines: usize) -> Vec<String> {
    if kind == FileKind::Notebook
        && let Some(cells) = notebook_cells(text)
    {
        return first_lines(&cells.join("\n"), lines);
    }
    first_lines(text, lines)
}

/// The sources of a Jupyter notebook's cells, in order; `None` when
/// `text` is not a notebook's JSON.
pub fn notebook_cells(text: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let cells = v.get("cells")?.as_array()?;
    Some(
        cells
            .iter()
            .filter_map(|c| match c.get("source")? {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Array(parts) => {
                    Some(parts.iter().filter_map(|x| x.as_str()).collect::<String>())
                }
                _ => None,
            })
            .filter(|s| !s.trim().is_empty())
            .collect(),
    )
}

/// A file size as a card shows it: "820 B", "48 KB", "3.4 MB".
pub fn size_words(bytes: u64) -> String {
    const KB: u64 = 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < KB * KB {
        format!("{} KB", bytes.div_ceil(KB))
    } else {
        format!("{:.1} MB", bytes as f64 / (KB * KB) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notebook_card_previews_its_cells_not_its_json() {
        let nb = r##"{"cells": [{"cell_type": "markdown", "source": ["# Lab 3\n", "Train the model"]},
            {"cell_type": "code", "source": "import torch"}], "nbformat": 4}"##;
        assert_eq!(notebook_cells(nb).unwrap().len(), 2);
        assert_eq!(
            preview_lines(FileKind::Notebook, nb, 6),
            ["Lab 3", "Train the model", "import torch"]
        );
        // Not a notebook's JSON: plain first lines.
        assert_eq!(preview_lines(FileKind::Notebook, "x = 1", 6), ["x = 1"]);
        assert!(notebook_cells("{}").is_none());
    }

    #[test]
    fn kinds_come_from_the_name() {
        assert_eq!(FileKind::of("CA-3 Report.PDF"), FileKind::Pdf);
        assert_eq!(FileKind::of("photo.jpeg"), FileKind::Image);
        assert_eq!(FileKind::of("notes.docx"), FileKind::Word);
        assert_eq!(FileKind::of("old.doc"), FileKind::Word);
        assert_eq!(FileKind::of("marks.xlsx"), FileKind::Sheet);
        assert_eq!(FileKind::of("deck.ppt"), FileKind::Slides);
        assert_eq!(FileKind::of("README.md"), FileKind::Markdown);
        assert_eq!(FileKind::of("log.txt"), FileKind::Text);
        assert_eq!(FileKind::of("data.csv"), FileKind::Csv);
        assert_eq!(FileKind::of("train.py"), FileKind::Python);
        assert_eq!(FileKind::of("lab.ipynb"), FileKind::Notebook);
        assert_eq!(FileKind::of("main.rs"), FileKind::Code);
        assert_eq!(FileKind::of("memo.m4a"), FileKind::Audio);
        assert_eq!(FileKind::of("Pasted text"), FileKind::Other);
        assert!(FileKind::Pdf.shows_page() && !FileKind::Image.shows_page());
    }

    #[test]
    fn a_card_reads_the_first_lines_and_the_size() {
        let text = "<!-- Tern: ca3.pdf → Markdown (PDF text, 3 pages) -->\n\n[page 1]\n# Computer Architecture\nAssignment 3\n\nQ1. Pipelining hazards and how forwarding resolves them in a five stage pipeline";
        let l = first_lines(text, 3);
        assert_eq!(l[0], "Computer Architecture");
        assert_eq!(l[1], "Assignment 3");
        assert!(l[2].ends_with('…') && l[2].chars().count() == 60);
        assert_eq!(size_words(820), "820 B");
        assert_eq!(size_words(48 * 1024), "48 KB");
        assert_eq!(size_words(3_565_158), "3.4 MB");
    }
}
