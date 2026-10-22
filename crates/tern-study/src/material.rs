//! Study materials and their chunks.
//!
//! A material is one file, note, page, or report the user studies from.
//! Its text is split into chunks of a few paragraphs; each chunk keeps an
//! [`Anchor`]: the file, the PDF page (from Tern Crawl's `[page N]`
//! markers), and the nearest heading. Tutor replies, quiz questions, and
//! viva questions point back at these anchors.

use serde::{Deserialize, Serialize};

use crate::text::{clip, one_line};

/// About how many characters a chunk holds.
pub const CHUNK_CHARS: usize = 1400;
/// Most characters of one material kept for study.
pub const MAX_MATERIAL_CHARS: usize = 2_000_000;

/// Where a material came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    Pdf,
    /// A Markdown or plain-text file.
    #[default]
    Note,
    /// A note from the vault.
    Vault,
    /// A web page read through Tern Crawl.
    Link,
    /// A Tern research report.
    Report,
    /// Slides, documents, and anything else read as text.
    Document,
}

impl MaterialKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Note => "Note",
            Self::Vault => "Vault note",
            Self::Link => "Link",
            Self::Report => "Research report",
            Self::Document => "Document",
        }
    }

    /// The kind a file name suggests.
    pub fn for_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "pdf" => Self::Pdf,
            "md" | "markdown" | "txt" | "text" | "org" | "rst" => Self::Note,
            _ => Self::Document,
        }
    }
}

/// One material in the library (its text is kept apart, in a
/// [`MaterialText`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Material {
    /// Short stable id: `m1`, `m2`, …
    pub id: String,
    /// What the user sees: the file name, note name, or page title.
    pub name: String,
    pub kind: MaterialKind,
    /// The path, URL, vault note, or report it was read from.
    pub origin: String,
    /// When it was added, Unix seconds.
    pub added_at: i64,
    pub chars: usize,
    /// PDF pages (0 when the text has no page markers).
    pub pages: u32,
    pub chunks: usize,
    /// The text was cut to [`MAX_MATERIAL_CHARS`].
    #[serde(default)]
    pub cut: bool,
}

/// Where a passage is in the user's materials.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Anchor {
    /// The material's id.
    pub material: String,
    /// The material's name, for display.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

impl Anchor {
    /// `notes.pdf p. 4 · Methods`.
    pub fn label(&self) -> String {
        let mut s = self.file.clone();
        if let Some(p) = self.page {
            s.push_str(&format!(" p. {p}"));
        }
        if let Some(h) = &self.section {
            s.push_str(&format!(" · {}", clip(h, 48)));
        }
        s
    }
}

/// A few paragraphs of a material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// `m1#3`: the material id and the chunk's number in it.
    pub id: String,
    pub anchor: Anchor,
    pub text: String,
}

/// A material's chunks, saved beside the library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialText {
    pub material: String,
    pub chunks: Vec<Chunk>,
}

/// A passage handed to a model or shown to the user, numbered `[S1]`, `[S2]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub n: usize,
    pub chunk: String,
    pub anchor: Anchor,
    pub text: String,
}

impl Source {
    pub fn label(&self) -> String {
        self.anchor.label()
    }
}

/// The passages as a block for a prompt: `[S1] notes.pdf p. 4` then the text.
pub fn sources_block(sources: &[Source]) -> String {
    let mut out = String::new();
    for s in sources {
        out.push_str(&format!("[S{}] {}\n{}\n\n", s.n, s.label(), s.text.trim()));
    }
    out
}

/// A Markdown heading's level and text (`## Methods` → `(2, "Methods")`),
/// or a numbered heading (`2.1 Background` → `(2, "Background")`).
pub fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim();
    if t.starts_with('#') {
        let level = t.chars().take_while(|c| *c == '#').count();
        let rest = t[level..].trim();
        if (1..=6).contains(&level) && t[level..].starts_with(' ') && !rest.is_empty() {
            return Some((level, rest.trim_end_matches('#').trim().to_owned()));
        }
        return None;
    }
    // Numbered: `3`, `3.2`, `3.2.1`, followed by a capitalised title,
    // short, with no full stop at the end.
    let (num, rest) = t.split_once(' ')?;
    let num = num.trim_end_matches('.');
    if num.is_empty()
        || num.len() > 8
        || !num.chars().all(|c| c.is_ascii_digit() || c == '.')
        || !num.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return None;
    }
    let rest = rest.trim();
    let n = rest.chars().count();
    if !(3..=80).contains(&n)
        || !rest.chars().next().is_some_and(char::is_uppercase)
        || rest.ends_with(['.', ',', ';', ':'])
        || rest.split_whitespace().count() > 10
    {
        return None;
    }
    Some((
        num.split('.').filter(|p| !p.is_empty()).count(),
        rest.to_owned(),
    ))
}

/// `[page N]` on a line of its own.
fn page_marker(line: &str) -> Option<u32> {
    line.trim()
        .strip_prefix("[page ")?
        .strip_suffix(']')?
        .trim()
        .parse()
        .ok()
}

/// Splits a material's text into chunks with anchors. Pages never share a
/// chunk; headings start a new one.
pub fn chunk_text(material: &str, file: &str, text: &str) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    let mut page: Option<u32> = None;
    let mut section: Option<String> = None;
    let mut buf = String::new();
    let mut buf_section: Option<String> = None;

    let flush = |out: &mut Vec<Chunk>, buf: &mut String, page: Option<u32>, sec: Option<String>| {
        let t = buf.trim();
        if t.chars().filter(|c| c.is_alphanumeric()).count() >= 20 {
            out.push(Chunk {
                id: format!("{material}#{}", out.len() + 1),
                anchor: Anchor {
                    material: material.to_owned(),
                    file: file.to_owned(),
                    page,
                    section: sec,
                },
                text: t.to_owned(),
            });
        }
        buf.clear();
    };

    for line in text.lines() {
        if let Some(p) = page_marker(line) {
            flush(&mut out, &mut buf, page, buf_section.clone());
            page = Some(p);
            buf_section = section.clone();
            continue;
        }
        if let Some((_, h)) = heading(line) {
            flush(&mut out, &mut buf, page, buf_section.clone());
            section = Some(h);
            buf_section = section.clone();
            buf.push_str(line.trim());
            buf.push('\n');
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && buf.chars().count() >= CHUNK_CHARS {
            flush(&mut out, &mut buf, page, buf_section.clone());
            buf_section = section.clone();
            continue;
        }
        // A very long paragraph is cut at a sentence end.
        if !blank && buf.chars().count() >= CHUNK_CHARS * 2 {
            flush(&mut out, &mut buf, page, buf_section.clone());
            buf_section = section.clone();
        }
        buf.push_str(line);
        buf.push('\n');
    }
    flush(&mut out, &mut buf, page, buf_section);
    out
}

/// The text a material is made from, cut to [`MAX_MATERIAL_CHARS`].
pub fn capped(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_MATERIAL_CHARS {
        return (text.to_owned(), false);
    }
    (text.chars().take(MAX_MATERIAL_CHARS).collect(), true)
}

/// Builds a material and its chunks from text.
pub fn build(
    id: &str,
    name: &str,
    kind: MaterialKind,
    origin: &str,
    text: &str,
    now: i64,
) -> (Material, MaterialText) {
    let (text, cut) = capped(text);
    let chunks = chunk_text(id, name, &text);
    let pages = text.lines().filter_map(page_marker).max().unwrap_or(0);
    let m = Material {
        id: id.to_owned(),
        name: one_line(name),
        kind,
        origin: origin.to_owned(),
        added_at: now,
        chars: text.chars().count(),
        pages,
        chunks: chunks.len(),
        cut,
    };
    (
        m,
        MaterialText {
            material: id.to_owned(),
            chunks,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &str = "[page 1]\nIntroduction to graphs and why they matter for routing problems.\n\n[page 2]\n2 Shortest Paths\nDijkstra's algorithm finds shortest paths from one source when weights are non-negative.\n\n[page 3]\nBellman-Ford handles negative edge weights and detects negative cycles in a graph.";

    #[test]
    fn chunks_keep_pages_and_sections() {
        let c = chunk_text("m1", "graphs.pdf", PDF);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].anchor.page, Some(1));
        assert_eq!(c[1].anchor.page, Some(2));
        assert_eq!(c[1].anchor.section.as_deref(), Some("Shortest Paths"));
        assert_eq!(c[2].anchor.section.as_deref(), Some("Shortest Paths"));
        assert_eq!(c[2].id, "m1#3");
        assert_eq!(c[1].anchor.label(), "graphs.pdf p. 2 · Shortest Paths");
    }

    #[test]
    fn markdown_headings_split_chunks() {
        let md = "# Cells\nCells are the basic unit of life in all organisms.\n## Mitochondria\nMitochondria produce most of the cell's chemical energy as ATP.";
        let c = chunk_text("m2", "bio.md", md);
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].anchor.section.as_deref(), Some("Mitochondria"));
        assert_eq!(c[1].anchor.page, None);
    }

    #[test]
    fn long_text_is_split_near_the_target_size() {
        let para = "Gradient descent updates weights against the gradient of the loss. ".repeat(30);
        let text = format!("{para}\n\n{para}\n\n{para}");
        let c = chunk_text("m3", "ml.txt", &text);
        assert!(c.len() >= 2, "{}", c.len());
        assert!(c.iter().all(|x| x.text.chars().count() <= CHUNK_CHARS * 3));
    }

    #[test]
    fn headings_are_recognised_conservatively() {
        assert_eq!(heading("## Methods"), Some((2, "Methods".into())));
        assert_eq!(
            heading("3.2 Data Collection"),
            Some((2, "Data Collection".into()))
        );
        assert_eq!(heading("3 apples were eaten."), None);
        assert_eq!(heading("2024 was a year."), None);
        assert_eq!(heading("#hashtag"), None);
    }

    #[test]
    fn build_counts_pages() {
        let (m, t) = build(
            "m1",
            "graphs.pdf",
            MaterialKind::Pdf,
            "/x/graphs.pdf",
            PDF,
            7,
        );
        assert_eq!(m.pages, 3);
        assert_eq!(m.chunks, t.chunks.len());
        assert_eq!(MaterialKind::for_name("A.PDF"), MaterialKind::Pdf);
    }
}
