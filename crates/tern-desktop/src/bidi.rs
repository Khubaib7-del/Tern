//! Right-to-left paragraphs (Urdu, Arabic, Persian, Hebrew) in chat
//! messages.
//!
//! The text shapers lay out a run of right-to-left letters in visual order
//! and join Arabic letters (cosmic-text on Linux, Core Text on macOS, and
//! DirectWrite on Windows with Tern's patch to GPUI's glyph-run callback,
//! `third_party/gpui-pre-windows`). What they cannot know is that a
//! paragraph as a whole reads right to left: GPUI lays every line out
//! left to right, so a sentence's final `۔` or `?` lands at the wrong end
//! and the paragraph hugs the left edge. Here a message is split into
//! paragraphs by direction (the first strong letter decides, as `dir=auto`
//! does on the web); each line of a right-to-left paragraph is wrapped in
//! a right-to-left embedding (U+202B … U+202C) so punctuation and embedded
//! English sit where a reader expects, and drawn as a row of words that
//! flows from the right edge and wraps there ([`RtlLine`], [`rtl_lines`]),
//! because GPUI's own line wrapping cannot wrap a right-to-left line.
//!
//! GPUI-free so it is unit tested.

use unicode_bidi::Direction;

/// Right-to-left embedding and its end (invisible).
const RLE: char = '\u{202B}';
const PDF: char = '\u{202C}';
/// Right-to-left mark.
const RLM: char = '\u{200F}';

/// Whether `text` has any right-to-left letter at all.
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
    })
}

/// Whether a paragraph reads right to left: its first strong letter is.
pub fn is_rtl(paragraph: &str) -> bool {
    unicode_bidi::get_base_direction(paragraph) == Direction::Rtl
}

/// A run of paragraphs that share a direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub rtl: bool,
    /// Markdown; a right-to-left block's lines carry the embedding.
    pub text: String,
}

/// The message split into blocks by direction, in order. A message with
/// no right-to-left letter is one left-to-right block, unchanged. Code
/// fences are never split or marked.
pub fn blocks(markdown: &str) -> Vec<Block> {
    if !has_rtl(markdown) {
        return vec![Block {
            rtl: false,
            text: markdown.to_owned(),
        }];
    }
    let mut paragraphs: Vec<(bool, String)> = Vec::new();
    let mut cur = String::new();
    let mut fence = false;
    let flush = |cur: &mut String, out: &mut Vec<(bool, String)>| {
        if !cur.trim().is_empty() {
            let text = std::mem::take(cur);
            let rtl = !text.trim_start().starts_with("```") && is_rtl(&text);
            out.push((rtl, text.trim_end_matches('\n').to_owned()));
        }
        cur.clear();
    };
    for line in markdown.split('\n') {
        if line.trim_start().starts_with("```") {
            if !fence {
                flush(&mut cur, &mut paragraphs);
            }
            fence = !fence;
            cur.push_str(line);
            cur.push('\n');
            if !fence {
                flush(&mut cur, &mut paragraphs);
            }
            continue;
        }
        if !fence && line.trim().is_empty() {
            flush(&mut cur, &mut paragraphs);
            continue;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    flush(&mut cur, &mut paragraphs);
    let mut out: Vec<Block> = Vec::new();
    for (rtl, p) in paragraphs {
        let p = if rtl { embed(&p) } else { p };
        match out.last_mut() {
            Some(b) if b.rtl == rtl => {
                b.text.push_str("\n\n");
                b.text.push_str(&p);
            }
            _ => out.push(Block { rtl, text: p }),
        }
    }
    if out.is_empty() {
        out.push(Block {
            rtl: false,
            text: markdown.to_owned(),
        });
    }
    out
}

/// Each line of a right-to-left paragraph inside an embedding, after any
/// Markdown marker that starts it (`- `, `1. `, `> `, `# `), so the
/// marker still parses.
fn embed(paragraph: &str) -> String {
    paragraph
        .lines()
        .map(|line| {
            let (marker, rest) = split_marker(line);
            if rest.trim().is_empty() {
                line.to_owned()
            } else {
                format!("{marker}{RLE}{rest}{PDF}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_marker(line: &str) -> (&str, &str) {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let mut n = 0;
    for p in ["- ", "* ", "+ ", "> "] {
        if body.starts_with(p) {
            n = p.len();
        }
    }
    if n == 0 {
        let hashes = body.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && body[hashes..].starts_with(' ') {
            n = hashes + 1;
        }
    }
    if n == 0 {
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && (body[digits..].starts_with(". ") || body[digits..].starts_with(") ")) {
            n = digits + 2;
        }
    }
    line.split_at(indent + n)
}

/// One line of a right-to-left paragraph, laid out by Tern rather than
/// by GPUI's line wrapper.
///
/// GPUI shapes a paragraph as one line and then wraps it by the glyphs'
/// x positions, which only works when they grow left to right. A line
/// whose base direction is right to left is laid out from its right end
/// leftwards (cosmic-text on Linux), so it never wraps and runs off the
/// column's left edge; where the glyphs come in screen order (Windows,
/// macOS) a long paragraph wraps with its last words on the first row.
/// So a right-to-left line is drawn as a row of words, flowing from the
/// right and wrapping by the layout engine: each word is short enough to
/// shape as one run, which every platform joins and orders correctly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlLine {
    /// A list marker drawn at the line's right end (`•`, `1.`).
    pub marker: Option<String>,
    /// `#` count for a heading line, else 0.
    pub heading: u8,
    /// The first line of a paragraph (after a blank line).
    pub starts_paragraph: bool,
    /// Words in reading order. A run of left-to-right words (an English
    /// name, a number with its unit) stays one item, in its own order.
    pub words: Vec<String>,
}

/// A right-to-left block's lines (see [`RtlLine`]). With `markdown`, list
/// and heading markers are read and inline marks (`**`, `__`, backticks,
/// `[text](link)`) are dropped, keeping their text.
pub fn rtl_lines(text: &str, markdown: bool) -> Vec<RtlLine> {
    let mut out = Vec::new();
    let mut gap = false;
    for raw in text.lines() {
        let line: String = raw.chars().filter(|c| *c != RLE && *c != PDF).collect();
        if line.trim().is_empty() {
            gap = !out.is_empty();
            continue;
        }
        let (mut marker, mut heading, mut body) = (None, 0u8, line.trim().to_owned());
        if markdown {
            let (m, rest) = split_marker(line.trim());
            let m = m.trim();
            if m.starts_with('#') {
                heading = m.len().min(6) as u8;
            } else if matches!(m, "-" | "*" | "+") {
                marker = Some("•".to_owned());
            } else if m == ">" {
                marker = Some("▏".to_owned());
            } else if !m.is_empty() {
                marker = Some(m.to_owned());
            }
            body = strip_inline(rest.trim());
        }
        out.push(RtlLine {
            marker,
            heading,
            starts_paragraph: std::mem::take(&mut gap),
            words: group_words(&body),
        });
    }
    out
}

/// Inline Markdown marks dropped, their text kept.
fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        match after.find("](") {
            Some(j) if after[j + 2..].contains(')') => {
                let close = after[j + 2..].find(')').unwrap_or(0);
                out.push_str(&rest[..i]);
                out.push_str(&after[..j]);
                rest = &after[j + 2 + close + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

/// A word of a right-to-left line as drawn: a word with a right-to-left
/// letter is put between right-to-left marks (U+200F), so punctuation at
/// its ends (`،`, `:`, `(`) takes the word's direction and sits on its
/// left or right as an Urdu reader expects. Without them a lone word is
/// shaped in a left-to-right line on Windows, and `ہے،` showed its comma
/// on the right of the word, between it and the word before (#66 tour).
pub fn rtl_word(word: &str) -> String {
    if has_rtl(word) {
        format!("{RLM}{word}{RLM}")
    } else {
        word.to_owned()
    }
}

/// Words split at spaces; consecutive words without a right-to-left
/// letter are kept together as one left-to-right item.
fn group_words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut ltr_open = false;
    for w in s.split_whitespace() {
        let ltr = !has_rtl(w) && w.chars().any(char::is_alphanumeric);
        match out.last_mut() {
            Some(last) if ltr && ltr_open => {
                last.push(' ');
                last.push_str(w);
            }
            _ => out.push(w.to_owned()),
        }
        ltr_open = ltr;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const URDU: &str = "اردو ایک خوبصورت زبان ہے۔";

    #[test]
    fn english_is_left_alone() {
        let md = "Hello **world**\n\n- one\n- two";
        assert_eq!(
            blocks(md),
            vec![Block {
                rtl: false,
                text: md.to_owned()
            }]
        );
        assert!(!has_rtl(md));
    }

    #[test]
    fn urdu_paragraphs_read_right_to_left_and_english_ones_do_not() {
        let md =
            format!("{URDU}\n\nIn English: Urdu is beautiful.\n\nمختصر: سلام پاکستان، Tern 2026");
        let b = blocks(&md);
        assert_eq!(b.len(), 3);
        assert!(b[0].rtl && !b[1].rtl && b[2].rtl);
        assert_eq!(b[0].text, format!("\u{202B}{URDU}\u{202C}"));
        assert_eq!(b[1].text, "In English: Urdu is beautiful.");
        // The first strong letter decides: Latin first reads left to right.
        assert!(!is_rtl("Tern says سلام"));
        assert!(is_rtl("«سلام» Tern"));
    }

    #[test]
    fn markers_stay_in_front_and_code_is_untouched() {
        let md = format!("- {URDU}\n1. {URDU}\n## {URDU}\n\n```\n{URDU}\n```");
        let b = blocks(&md);
        assert!(b[0].rtl);
        assert!(b[0].text.starts_with("- \u{202B}اردو"), "{:?}", b[0].text);
        assert!(b[0].text.contains("\n1. \u{202B}"), "{:?}", b[0].text);
        assert!(b[0].text.contains("\n## \u{202B}"), "{:?}", b[0].text);
        assert!(!b[1].rtl);
        assert_eq!(b[1].text, format!("```\n{URDU}\n```"));
    }

    #[test]
    fn a_right_to_left_paragraph_is_laid_out_as_words_in_reading_order() {
        let md = format!(
            "\u{202B}{URDU}\u{202C}\n\n- \u{202B}**سلام** پاکستان\u{202C}\n## \u{202B}عنوان\u{202C}\n\u{202B}مختصر: سلام، Tern 2026 [ویب](https://x.org)\u{202C}"
        );
        let l = rtl_lines(&md, true);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].words, vec!["اردو", "ایک", "خوبصورت", "زبان", "ہے۔"]);
        assert!(!l[0].starts_paragraph && l[0].marker.is_none());
        assert!(l[1].starts_paragraph);
        assert_eq!(l[1].marker.as_deref(), Some("•"));
        assert_eq!(l[1].words, vec!["سلام", "پاکستان"]);
        assert_eq!(
            (l[2].heading, l[2].words.clone()),
            (2, vec!["عنوان".to_owned()])
        );
        // English words stay one item in their own order; links keep text.
        assert_eq!(l[3].words, vec!["مختصر:", "سلام،", "Tern 2026", "ویب"]);
        // A user's message is plain text: marks are kept.
        let plain = rtl_lines("**سلام** - ok", false);
        assert_eq!(plain[0].words, vec!["**سلام**", "-", "ok"]);
        // Drawn words: right-to-left ones between marks, others as they are.
        assert_eq!(rtl_word("ہے،"), "\u{200F}ہے،\u{200F}");
        assert_eq!(rtl_word("Tern 2026"), "Tern 2026");
    }
}
