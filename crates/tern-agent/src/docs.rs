//! Documents to Markdown (#69): a PDF, Word (DOCX), PowerPoint (PPTX),
//! Excel (XLSX) or HTML file attached to a message is converted to
//! Markdown for the model, on this computer.
//!
//! - **Native parsing first**: a PDF's text layer (`tern_crawl::pdf`),
//!   DOCX/PPTX/XLSX from their XML (headings, lists, tables, slides,
//!   sheets), HTML through the crawler's reader.
//! - **Scanned PDFs** (no text layer): each page's scanned picture (JPEG,
//!   or 8-bit grey/RGB) is taken out of the PDF and read by the local
//!   document model when it runs (Granite-Docling through `tern-vlm`),
//!   else by the local OCR model (PP-OCRv6 / Tesseract), else the file is
//!   marked as not readable yet with what to download.
//! - **Cache**: conversions are kept by the file's SHA-256 in
//!   `<cache>/documents/` so the same file is not read twice.
//!
//! The Markdown starts with a `<!-- Tern: … -->` line saying how it was
//! made and how many pages it has; the chip shows it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The most pages read from a scanned PDF (each is a model run).
pub const MAX_SCANNED_PAGES: usize = 30;
/// The most rows read from one sheet, and columns from one row.
const MAX_ROWS: usize = 300;
const MAX_COLS: usize = 40;

/// A converted document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Converted {
    pub markdown: String,
    /// Pages (PDF), slides (PPTX) or sheets (XLSX), when the format has them.
    pub pages: Option<usize>,
    /// How it was read: "PDF text", "DOCX", "OCR (PP-OCRv6 small)".
    pub how: String,
}

impl Converted {
    /// The Markdown with its `<!-- Tern: … -->` header line.
    pub fn with_header(&self, name: &str) -> String {
        format!(
            "{}\n{}",
            header(name, &self.how, self.pages),
            self.markdown.trim()
        )
    }
}

/// A small PDF with one line of text per page (the UI tour, tests).
pub fn sample_pdf(pages: &[&str]) -> Vec<u8> {
    tern_crawl::pdf::sample(pages)
}

/// A small Word file: a heading, a paragraph and a list item (the UI tour).
pub fn sample_docx() -> Vec<u8> {
    use std::io::Write as _;
    let doc = r#"<w:document xmlns:w="w"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Meeting notes</w:t></w:r></w:p><w:p><w:r><w:t>The demo is on Friday in Lab 3.</w:t></w:r></w:p></w:body></w:document>"#;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        if z.start_file("word/document.xml", opts).is_ok() {
            let _ = z.write_all(doc.as_bytes());
        }
        let _ = z.finish();
    }
    out.into_inner()
}

/// `<!-- Tern: report.pdf → Markdown (PDF text, 12 pages) -->`.
pub fn header(name: &str, how: &str, pages: Option<usize>) -> String {
    let pages = match pages {
        Some(1) => ", 1 page".to_owned(),
        Some(n) => format!(", {n} pages"),
        None => String::new(),
    };
    format!("<!-- Tern: {name} → Markdown ({how}{pages}) -->")
}

/// The header's "how" and page count, for the chip: ("DOCX", Some(3)).
pub fn parse_header(text: &str) -> Option<(String, Option<usize>)> {
    let line = text.lines().next()?.trim();
    let inner = line.strip_prefix("<!-- Tern: ")?.strip_suffix(" -->")?;
    let (_, rest) = inner.split_once("→ Markdown (")?;
    let rest = rest.strip_suffix(')')?;
    let mut parts = rest.rsplitn(2, ", ");
    let last = parts.next()?;
    match (
        last.strip_suffix(" pages")
            .or_else(|| last.strip_suffix(" page")),
        parts.next(),
    ) {
        (Some(n), Some(how)) => Some((how.to_owned(), n.trim().parse().ok())),
        _ => Some((rest.to_owned(), None)),
    }
}

/// Whether Tern converts files with this name (by extension).
pub fn converts(name: &str) -> bool {
    matches!(
        ext(name).as_str(),
        "docx" | "pptx" | "xlsx" | "html" | "htm" | "pdf"
    )
}

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn cache_dir() -> Option<PathBuf> {
    crate::platform::AppPaths::discover()
        .ok()
        .map(|a| a.cache_dir.join("documents"))
}

fn cached(hash: &str) -> Option<Converted> {
    let p = cache_dir()?.join(format!("{hash}.json"));
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn keep(hash: &str, c: &Converted) {
    if let Some(dir) = cache_dir()
        && std::fs::create_dir_all(&dir).is_ok()
        && let Ok(json) = serde_json::to_vec(c)
    {
        let _ = std::fs::write(dir.join(format!("{hash}.json")), json);
    }
}

/// The SHA-256 of `bytes`, hex.
pub fn hash(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Converts `bytes` (a file named `name`) to Markdown, from the cache when
/// the same file was converted before. Scanned PDFs use the local models.
pub fn convert(name: &str, bytes: &[u8]) -> Result<Converted, String> {
    convert_with(name, bytes, true)
}

/// [`convert`]; `scanned` false leaves a PDF without a text layer
/// unread (inside a folder or archive, where a model run per page would
/// be too slow).
pub fn convert_with(name: &str, bytes: &[u8], scanned: bool) -> Result<Converted, String> {
    // A malformed file can make a parser panic; that is an error for
    // this attachment, never a crash.
    let guarded = |f: &dyn Fn() -> Result<Converted, String>| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
            crate::diag::note(&format!("docs: {name}: the parser panicked"));
            Err(format!(
                "Tern could not read {name}: the file looks damaged"
            ))
        })
    };
    if !scanned && ext(name) == "pdf" {
        return guarded(&|| pdf_text_only(bytes));
    }
    let h = hash(bytes);
    if let Some(c) = cached(&h) {
        crate::diag::note(&format!("docs: {name} from the cache ({})", c.how));
        return Ok(c);
    }
    let t0 = std::time::Instant::now();
    let c = guarded(&|| convert_fresh(name, bytes))?;
    crate::diag::note(&format!(
        "docs: {name} -> Markdown via {} ({} pages, {} chars) in {:.1}s",
        c.how,
        c.pages.map_or("-".into(), |n| n.to_string()),
        c.markdown.chars().count(),
        t0.elapsed().as_secs_f64()
    ));
    keep(&h, &c);
    Ok(c)
}

fn convert_fresh(name: &str, bytes: &[u8]) -> Result<Converted, String> {
    match ext(name).as_str() {
        "docx" => docx(bytes),
        "pptx" => pptx(bytes),
        "xlsx" => xlsx(bytes),
        "html" | "htm" => Ok(html(bytes)),
        _ if tern_crawl::pdf::is_pdf(bytes) => pdf(bytes),
        e => Err(format!("Tern does not convert .{e} files yet")),
    }
}

// ----- PDF -----

fn pdf(bytes: &[u8]) -> Result<Converted, String> {
    let text = tern_crawl::pdf::read(bytes);
    let pages = lopdf::Document::load_mem(bytes)
        .ok()
        .map(|d| d.get_pages().len());
    if let Ok(t) = &text {
        let joined = t.joined();
        let n = pages.unwrap_or(1).max(1);
        // A text layer worth reading: at least some 10 letters a page on
        // average (a scanned PDF has none, or a stray page number).
        if t.chars() >= 10 * n {
            return Ok(Converted {
                markdown: joined,
                pages,
                how: "PDF text".into(),
            });
        }
    }
    // Scanned: read the pages' pictures with a local model.
    scanned_pdf(bytes, pages).map_err(|e| match &text {
        Err(t) => format!("{t}; {e}"),
        Ok(_) => e,
    })
}

fn pdf_text_only(bytes: &[u8]) -> Result<Converted, String> {
    let t = tern_crawl::pdf::read(bytes).map_err(|e| e.to_string())?;
    let pages = lopdf::Document::load_mem(bytes)
        .ok()
        .map(|d| d.get_pages().len());
    Ok(Converted {
        markdown: t.joined(),
        pages,
        how: "PDF text".into(),
    })
}

/// The scanned picture of each page (JPEG bytes, or a PNG made from raw
/// 8-bit grey/RGB samples), in page order, at most [`MAX_SCANNED_PAGES`].
pub fn page_pictures(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    use lopdf::Object;
    let doc = lopdf::Document::load_mem(bytes)
        .map_err(|e| format!("the PDF could not be opened: {e}"))?;
    let mut out = Vec::new();
    for (_, page_id) in doc.get_pages().into_iter().take(MAX_SCANNED_PAGES) {
        let mut best: Option<(u64, Vec<u8>)> = None;
        let Ok((resources, _)) = doc.get_page_resources(page_id) else {
            continue;
        };
        let Some(res) = resources else { continue };
        let xobjects = match res.get(b"XObject") {
            Ok(Object::Dictionary(d)) => d.clone(),
            Ok(Object::Reference(r)) => match doc.get_object(*r) {
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => continue,
            },
            _ => continue,
        };
        for (_, obj) in xobjects.iter() {
            let Ok(r) = obj.as_reference() else { continue };
            let Ok(Object::Stream(s)) = doc.get_object(r) else {
                continue;
            };
            if s.dict.get(b"Subtype").and_then(|o| o.as_name()).ok() != Some(b"Image".as_slice()) {
                continue;
            }
            let w = s
                .dict
                .get(b"Width")
                .and_then(|o| o.as_i64())
                .unwrap_or(0)
                .max(0) as u32;
            let h = s
                .dict
                .get(b"Height")
                .and_then(|o| o.as_i64())
                .unwrap_or(0)
                .max(0) as u32;
            let area = u64::from(w) * u64::from(h);
            if area < 200 * 200 || best.as_ref().is_some_and(|(a, _)| *a >= area) {
                continue;
            }
            let filters: Vec<Vec<u8>> = match s.dict.get(b"Filter") {
                Ok(Object::Name(n)) => vec![n.clone()],
                Ok(Object::Array(a)) => a
                    .iter()
                    .filter_map(|o| o.as_name().ok().map(<[u8]>::to_vec))
                    .collect(),
                _ => Vec::new(),
            };
            let picture = if filters.last().is_some_and(|f| f == b"DCTDecode") {
                if filters.len() == 1 {
                    Some(s.content.clone())
                } else {
                    None
                }
            } else {
                raw_picture(s, w, h)
            };
            if let Some(p) = picture {
                best = Some((area, p));
            }
        }
        if let Some((_, p)) = best {
            out.push(p);
        }
    }
    Ok(out)
}

/// A PNG from an image stream of raw 8-bit DeviceGray or DeviceRGB samples.
fn raw_picture(s: &lopdf::Stream, w: u32, h: u32) -> Option<Vec<u8>> {
    let bpc = s
        .dict
        .get(b"BitsPerComponent")
        .and_then(|o| o.as_i64())
        .unwrap_or(8);
    if bpc != 8 {
        return None;
    }
    let cs = s.dict.get(b"ColorSpace").and_then(|o| o.as_name()).ok()?;
    let data = s.decompressed_content().ok()?;
    let img = match cs {
        b"DeviceGray" => image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, data)?),
        b"DeviceRGB" => image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, data)?),
        _ => return None,
    };
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

fn scanned_pdf(bytes: &[u8], pages: Option<usize>) -> Result<Converted, String> {
    use crate::local_models::{LocalModels, Task, can_run};
    let pictures = page_pictures(bytes)?;
    if pictures.is_empty() {
        return Err("it has no text layer and no scanned page pictures Tern can take out".into());
    }
    let (lm, _) = LocalModels::discover()?;
    let prefs = crate::prefs::Preferences::load();
    let doc_model = lm
        .usable(&prefs.local_models, Task::DocParse)
        .filter(|e| can_run(e))
        .map(|e| (e.id.clone(), e.name.clone()));
    let ocr_model = lm
        .usable(&prefs.local_models, Task::Ocr)
        .filter(|e| can_run(e))
        .map(|e| (e.id.clone(), e.name.clone()));
    if doc_model.is_none() && ocr_model.is_none() {
        return Err(
            "it is a scanned PDF and no local OCR or document model is downloaded: get PP-OCRv6 small in Settings › Local models".into(),
        );
    }
    let mut md = Vec::new();
    let mut used = String::new();
    for (i, pic) in pictures.iter().enumerate() {
        let mut page = None;
        if let Some((id, name)) = &doc_model {
            match lm.page_markdown(id, pic) {
                Ok(t) if !t.trim().is_empty() => {
                    used = name.clone();
                    page = Some(t);
                }
                Ok(_) => {}
                Err(e) => crate::diag::note(&format!("docs: {id} page {} failed: {e}", i + 1)),
            }
        }
        if page.is_none()
            && let Some((id, name)) = &ocr_model
        {
            match lm.ocr(id, pic) {
                Ok(t) => {
                    if used.is_empty() {
                        used = format!("OCR ({name})");
                    }
                    page = Some(t);
                }
                Err(e) => return Err(format!("{name} could not read page {}: {e}", i + 1)),
            }
        }
        md.push(format!(
            "[page {}]\n{}",
            i + 1,
            page.unwrap_or_default().trim()
        ));
    }
    Ok(Converted {
        markdown: md.join("\n\n"),
        pages: pages.or(Some(pictures.len())),
        how: if used.is_empty() { "OCR".into() } else { used },
    })
}

// ----- Office XML -----

fn zip_file(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Option<String> {
    use std::io::Read as _;
    let mut f = zip.by_name(name).ok()?;
    if f.size() > 64 * 1024 * 1024 {
        return None;
    }
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn open_zip(bytes: &[u8]) -> Result<zip::ZipArchive<std::io::Cursor<&[u8]>>, String> {
    zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("the file could not be opened: {e}"))
}

/// XML entities back to text.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn re(p: &str) -> regex::Regex {
    regex::Regex::new(p).expect("static regex")
}

/// The text runs (`<w:t>`, `<a:t>`, `<t>`) of an XML fragment, joined.
fn runs(xml: &str, tag: &str) -> String {
    let r = re(&format!(r"(?s)<{tag}(?:\s[^>]*)?>(.*?)</{tag}>"));
    let tab = re(r"<w:tab\s*/>");
    let xml = tab.replace_all(xml, "<w:t>\t</w:t>");
    r.captures_iter(&xml)
        .map(|c| unescape(&c[1]))
        .collect::<String>()
}

fn table_md(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0).min(MAX_COLS);
    if width == 0 {
        return String::new();
    }
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mut r: Vec<String> = r
            .iter()
            .take(width)
            .map(|c| c.replace('|', "\\|").replace('\n', " ").trim().to_owned())
            .collect();
        r.resize(width, String::new());
        out.push(format!("| {} |", r.join(" | ")));
        if i == 0 {
            out.push(format!("|{}", " --- |".repeat(width)));
        }
    }
    out.join("\n")
}

/// Word: headings (Heading1–6, Title), list items, tables, paragraphs.
pub fn docx(bytes: &[u8]) -> Result<Converted, String> {
    let mut z = open_zip(bytes)?;
    let xml = zip_file(&mut z, "word/document.xml").ok_or("it has no word/document.xml")?;
    let body = re(r"(?s)<w:tbl>.*?</w:tbl>|<w:p[ >].*?</w:p>|<w:p/>");
    let style = re(r#"<w:pStyle w:val="([^"]+)""#);
    let mut out = Vec::new();
    for m in body.find_iter(&xml) {
        let part = m.as_str();
        if part.starts_with("<w:tbl>") {
            let row = re(r"(?s)<w:tr[ >].*?</w:tr>");
            let cell = re(r"(?s)<w:tc[ >].*?</w:tc>");
            let rows: Vec<Vec<String>> = row
                .find_iter(part)
                .take(MAX_ROWS)
                .map(|r| {
                    cell.find_iter(r.as_str())
                        .map(|c| runs(c.as_str(), "w:t"))
                        .collect()
                })
                .collect();
            out.push(table_md(&rows));
            continue;
        }
        let text = runs(part, "w:t");
        if text.trim().is_empty() {
            continue;
        }
        let st = style
            .captures(part)
            .map(|c| c[1].to_ascii_lowercase())
            .unwrap_or_default();
        let line = if st == "title" {
            format!("# {text}")
        } else if let Some(n) = st
            .strip_prefix("heading")
            .and_then(|n| n.parse::<usize>().ok())
        {
            format!("{} {text}", "#".repeat((n + 1).clamp(2, 6)))
        } else if part.contains("<w:numPr>") || st.contains("list") {
            format!("- {text}")
        } else {
            text
        };
        out.push(line);
    }
    Ok(Converted {
        markdown: out.join("\n\n"),
        pages: None,
        how: "DOCX".into(),
    })
}

/// PowerPoint: one section per slide, its paragraphs as lines.
pub fn pptx(bytes: &[u8]) -> Result<Converted, String> {
    let mut z = open_zip(bytes)?;
    let slide_name = re(r"^ppt/slides/slide(\d+)\.xml$");
    let mut slides: Vec<(usize, String)> = z
        .file_names()
        .filter_map(|n| {
            slide_name
                .captures(n)
                .and_then(|c| c[1].parse().ok())
                .map(|i| (i, n.to_owned()))
        })
        .collect();
    slides.sort();
    let para = re(r"(?s)<a:p>.*?</a:p>|<a:p [^>]*>.*?</a:p>");
    let mut out = Vec::new();
    for (i, name) in &slides {
        let Some(xml) = zip_file(&mut z, name) else {
            continue;
        };
        let lines: Vec<String> = para
            .find_iter(&xml)
            .map(|p| runs(p.as_str(), "a:t"))
            .filter(|t| !t.trim().is_empty())
            .collect();
        let mut sec = format!("## Slide {i}");
        if let Some((first, rest)) = lines.split_first() {
            sec = format!("## Slide {i}: {first}");
            for l in rest {
                sec.push_str(&format!("\n- {l}"));
            }
        }
        out.push(sec);
    }
    if slides.is_empty() {
        return Err("it has no slides".into());
    }
    Ok(Converted {
        markdown: out.join("\n\n"),
        pages: Some(slides.len()),
        how: "PPTX".into(),
    })
}

/// Excel: one table per sheet (shared strings, inline strings, numbers).
pub fn xlsx(bytes: &[u8]) -> Result<Converted, String> {
    let mut z = open_zip(bytes)?;
    let shared: Vec<String> = zip_file(&mut z, "xl/sharedStrings.xml")
        .map(|x| {
            re(r"(?s)<si>(.*?)</si>")
                .captures_iter(&x)
                .map(|c| runs(&c[1], "t"))
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = zip_file(&mut z, "xl/workbook.xml")
        .map(|x| {
            re(r#"<sheet [^>]*name="([^"]*)""#)
                .captures_iter(&x)
                .map(|c| unescape(&c[1]))
                .collect()
        })
        .unwrap_or_default();
    let sheet_name = re(r"^xl/worksheets/sheet(\d+)\.xml$");
    let mut sheets: Vec<(usize, String)> = z
        .file_names()
        .filter_map(|n| {
            sheet_name
                .captures(n)
                .and_then(|c| c[1].parse().ok())
                .map(|i| (i, n.to_owned()))
        })
        .collect();
    sheets.sort();
    let row_re = re(r"(?s)<row[ >].*?</row>");
    let cell_re = re(r#"(?s)<c r="([A-Z]+)\d+"([^>]*?)(?:/>|>(.*?)</c>)"#);
    let v_re = re(r"(?s)<v>(.*?)</v>");
    let mut out = Vec::new();
    for (k, (i, name)) in sheets.iter().enumerate() {
        let Some(xml) = zip_file(&mut z, name) else {
            continue;
        };
        let mut rows = Vec::new();
        for r in row_re.find_iter(&xml).take(MAX_ROWS) {
            let mut cells: Vec<String> = Vec::new();
            for c in cell_re.captures_iter(r.as_str()) {
                let col = col_index(&c[1]);
                if col >= MAX_COLS {
                    continue;
                }
                let attrs = &c[2];
                let inner = c.get(3).map_or("", |m| m.as_str());
                let v = v_re
                    .captures(inner)
                    .map(|m| unescape(&m[1]))
                    .unwrap_or_default();
                let text = if attrs.contains(r#"t="s""#) {
                    v.parse::<usize>()
                        .ok()
                        .and_then(|n| shared.get(n).cloned())
                        .unwrap_or_default()
                } else if attrs.contains(r#"t="inlineStr""#) {
                    runs(inner, "t")
                } else {
                    v
                };
                if cells.len() <= col {
                    cells.resize(col + 1, String::new());
                }
                cells[col] = text;
            }
            if cells.iter().any(|c| !c.trim().is_empty()) {
                rows.push(cells);
            }
        }
        let title = names
            .get(k)
            .cloned()
            .unwrap_or_else(|| format!("Sheet {i}"));
        out.push(format!("## {title}\n\n{}", table_md(&rows)));
    }
    if sheets.is_empty() {
        return Err("it has no sheets".into());
    }
    Ok(Converted {
        markdown: out.join("\n\n"),
        pages: Some(sheets.len()),
        how: "XLSX".into(),
    })
}

/// `A` → 0, `Z` → 25, `AA` → 26.
fn col_index(letters: &str) -> usize {
    letters
        .bytes()
        .fold(0usize, |n, b| {
            n * 26 + usize::from(b.saturating_sub(b'A')) + 1
        })
        .saturating_sub(1)
}

/// HTML through the crawler's reader (main content, headings, links).
pub fn html(bytes: &[u8]) -> Converted {
    let text = String::from_utf8_lossy(bytes);
    let base = url::Url::parse("file:///document.html").expect("static url");
    let e = tern_crawl::extract::extract_html(&text, &base);
    let mut md = e.markdown;
    if let Some(t) = e.title.filter(|t| !md.contains(t.as_str())) {
        md = format!("# {t}\n\n{md}");
    }
    Converted {
        markdown: md,
        pages: None,
        how: "HTML".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (n, body) in files {
                z.start_file(*n, opts).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn word_headings_lists_and_tables() {
        let doc = r#"<w:document><w:body>
<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Costs &amp; totals</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Paid in </w:t></w:r><w:r><w:t>full.</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/></w:numPr></w:pPr><w:r><w:t>First</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Total</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Tea</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>4,250</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
</w:body></w:document>"#;
        let c = docx(&zip_of(&[("word/document.xml", doc)])).unwrap();
        assert_eq!(
            c.markdown,
            "# Report\n\n## Costs & totals\n\nPaid in full.\n\n- First\n\n| Item | Total |\n| --- | --- |\n| Tea | 4,250 |"
        );
    }

    #[test]
    fn slides_in_order() {
        let s = |t: &str, b: &str| {
            format!(
                "<p:sld><a:p><a:r><a:t>{t}</a:t></a:r></a:p><a:p><a:r><a:t>{b}</a:t></a:r></a:p></p:sld>"
            )
        };
        let (a, b) = (s("Intro", "Hello"), s("Plan", "Ship #69"));
        let c = pptx(&zip_of(&[
            ("ppt/slides/slide10.xml", &b),
            ("ppt/slides/slide2.xml", &a),
        ]))
        .unwrap();
        assert_eq!(c.pages, Some(2));
        assert_eq!(
            c.markdown,
            "## Slide 2: Intro\n- Hello\n\n## Slide 10: Plan\n- Ship #69"
        );
    }

    #[test]
    fn sheets_as_tables() {
        let shared = "<sst><si><t>Item</t></si><si><t>Total</t></si><si><t>Tea</t></si></sst>";
        let book = r#"<workbook><sheets><sheet name="Costs" sheetId="1"/></sheets></workbook>"#;
        let sheet = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="C2"><v>4250</v></c></row></sheetData></worksheet>"#;
        let c = xlsx(&zip_of(&[
            ("xl/sharedStrings.xml", shared),
            ("xl/workbook.xml", book),
            ("xl/worksheets/sheet1.xml", sheet),
        ]))
        .unwrap();
        assert_eq!(
            c.markdown,
            "## Costs\n\n| Item | Total |  |\n| --- | --- | --- |\n| Tea |  | 4250 |"
        );
        assert_eq!(col_index("A"), 0);
        assert_eq!(col_index("AA"), 26);
    }

    #[test]
    fn pdf_text_layer_and_header() {
        let bytes = tern_crawl::pdf::sample(&[
            "Survey of Retrieval Methods with enough words to count as a text layer",
            "Second page text here, also long enough to be read as text.",
        ]);
        let c = convert_fresh("paper.pdf", &bytes).unwrap();
        assert_eq!(c.how, "PDF text");
        assert_eq!(c.pages, Some(2));
        let h = c.with_header("paper.pdf");
        assert!(h.starts_with("<!-- Tern: paper.pdf → Markdown (PDF text, 2 pages) -->"));
        assert_eq!(parse_header(&h), Some(("PDF text".into(), Some(2))));
        assert_eq!(
            parse_header(&header("a.docx", "DOCX", None)),
            Some(("DOCX".into(), None))
        );
    }

    #[test]
    fn html_keeps_headings() {
        let c = html(b"<html><head><title>Notes</title></head><body><main><h1>Notes</h1><p>Hello <b>there</b>.</p></main></body></html>");
        assert!(c.markdown.contains("Notes"), "{}", c.markdown);
        assert!(c.markdown.contains("Hello"), "{}", c.markdown);
    }

    #[test]
    fn same_bytes_same_hash() {
        assert_eq!(
            hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
