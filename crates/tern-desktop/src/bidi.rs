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
//! does on the web); a right-to-left paragraph is drawn right-aligned, and
//! each of its lines is wrapped in a right-to-left embedding (U+202B …
//! U+202C) so punctuation and embedded English sit where a reader expects.
//!
//! GPUI-free so it is unit tested.

use unicode_bidi::Direction;

/// Right-to-left embedding and its end (invisible).
const RLE: char = '\u{202B}';
const PDF: char = '\u{202C}';

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

/// A user message's lines, each marked like [`blocks`] does, for the
/// plain-text bubble: `(right to left, text)`.
pub fn plain_lines(text: &str) -> (bool, String) {
    if !has_rtl(text) {
        return (false, text.to_owned());
    }
    let rtl = is_rtl(text);
    if !rtl {
        return (false, text.to_owned());
    }
    (
        true,
        text.lines()
            .map(|l| {
                if l.trim().is_empty() {
                    l.to_owned()
                } else {
                    format!("{RLE}{l}{PDF}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
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
        let md = format!("{URDU}\n\nIn English: Urdu is beautiful.\n\nمختصر: سلام پاکستان، Tern 2026");
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
    fn a_users_urdu_message_is_marked_line_by_line() {
        let (rtl, text) = plain_lines(&format!("{URDU}\n{URDU}"));
        assert!(rtl);
        assert_eq!(text.matches('\u{202B}').count(), 2);
        assert_eq!(plain_lines("hi"), (false, "hi".to_owned()));
    }
}
