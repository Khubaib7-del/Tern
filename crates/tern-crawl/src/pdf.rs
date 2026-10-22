//! PDF text. Papers are the heart of research, so the reader opens PDFs
//! from the web and from attachments alike. Text is kept page by page with
//! `[page N]` markers, so an answer can cite the page a claim is on.
//! Scanned PDFs (pictures of pages) have no text layer and are reported as
//! such; reading them needs a vision model.

/// The largest PDF Tern opens.
pub const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;

/// A PDF's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfText {
    /// Each page's text, tidied.
    pub pages: Vec<String>,
    /// A title guessed from the first page's first substantial line.
    pub title: Option<String>,
}

impl PdfText {
    /// All pages joined, each opened by a `[page N]` marker.
    pub fn joined(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.pages.iter().enumerate() {
            if p.trim().is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("[page {}]\n{}", i + 1, p.trim()));
        }
        out
    }

    /// Characters of real text.
    pub fn chars(&self) -> usize {
        self.pages.iter().map(|p| p.trim().chars().count()).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error("this is not a PDF")]
    NotPdf,
    #[error("the PDF is over {} MB", MAX_PDF_BYTES / 1_048_576)]
    TooBig,
    #[error("the PDF is password-protected")]
    Encrypted,
    #[error("the PDF has no text layer (scanned pages need a vision model)")]
    NoText,
    #[error("the PDF could not be read: {0}")]
    Broken(String),
}

/// Whether bytes start like a PDF.
pub fn is_pdf(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    head.windows(5).any(|w| w == b"%PDF-")
}

/// Reads a PDF's text page by page.
pub fn read(bytes: &[u8]) -> Result<PdfText, PdfError> {
    if !is_pdf(bytes) {
        return Err(PdfError::NotPdf);
    }
    if bytes.len() > MAX_PDF_BYTES {
        return Err(PdfError::TooBig);
    }
    // The parser can panic on malformed files; a bad PDF must never take
    // the app down with it.
    let owned = bytes.to_vec();
    let result =
        std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem_by_pages(&owned));
    let pages = match result {
        Ok(Ok(pages)) => pages,
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.to_ascii_lowercase().contains("encrypt")
                || msg.to_ascii_lowercase().contains("password")
            {
                return Err(PdfError::Encrypted);
            }
            return Err(PdfError::Broken(msg));
        }
        Err(_) => return Err(PdfError::Broken("the parser gave up on it".into())),
    };
    let pages: Vec<String> = pages.iter().map(|p| tidy_page(p)).collect();
    let text = PdfText {
        title: guess_title(pages.first().map(String::as_str).unwrap_or_default()),
        pages,
    };
    if text.chars() < 20 {
        return Err(PdfError::NoText);
    }
    Ok(text)
}

/// Joins words hyphenated across lines, collapses runs of spaces, and
/// keeps paragraph breaks.
fn tidy_page(raw: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in raw.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            if blank > 0 {
                out.push_str("\n\n");
            } else if out.ends_with('-')
                && out[..out.len() - 1].ends_with(|c: char| c.is_alphabetic())
                && line.starts_with(|c: char| c.is_lowercase())
            {
                out.pop();
            } else {
                out.push('\n');
            }
        }
        blank = 0;
        out.push_str(&line);
    }
    out
}

fn guess_title(first_page: &str) -> Option<String> {
    first_page
        .lines()
        .map(str::trim)
        .find(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 8)
        .map(|l| l.chars().take(160).collect())
}

/// A small, valid PDF with one line of Helvetica per page. For tests in
/// this crate and the ones that use it.
pub fn sample(pages: &[&str]) -> Vec<u8> {
    let mut objs: Vec<String> = Vec::new();
    let n = pages.len();
    // 1 catalog, 2 pages, 3 font, then (page, content) pairs.
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + i * 2)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {n} >>",
        kids.join(" ")
    ));
    objs.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    );
    for (i, text) in pages.iter().enumerate() {
        let content_id = 5 + i * 2;
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
        ));
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 12 Tf 72 720 Td ({escaped}) Tj ET");
        objs.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objs.len() + 1
    ));
    for off in offsets {
        out.push_str(&format!("{off:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    ));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pages_with_markers_and_a_title() {
        let pdf = sample(&[
            "Attention Is All You Need",
            "The Transformer uses self-attention throughout.",
        ]);
        let t = read(&pdf).unwrap();
        assert_eq!(t.pages.len(), 2);
        assert_eq!(t.title.as_deref(), Some("Attention Is All You Need"));
        let j = t.joined();
        assert!(j.starts_with("[page 1]\nAttention Is All You Need"), "{j}");
        assert!(
            j.contains("[page 2]\nThe Transformer uses self-attention throughout."),
            "{j}"
        );
    }

    #[test]
    fn plain_errors() {
        assert_eq!(read(b"hello"), Err(PdfError::NotPdf));
        assert!(matches!(
            read(b"%PDF-1.4\ngarbage"),
            Err(PdfError::Broken(_))
        ));
        assert!(is_pdf(&sample(&["x"])));
    }

    #[test]
    fn tidies_hyphenation_and_spacing() {
        assert_eq!(
            tidy_page("retrieval aug-\nmented   generation\n\nNext para"),
            "retrieval augmented generation\n\nNext para"
        );
        assert_eq!(tidy_page("Self-\nAttention"), "Self-\nAttention");
    }
}
