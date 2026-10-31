//! Flashcards as CSV and TSV.
//!
//! * **CSV** is Tern's own plain format, one card per row with a header:
//!   `front,back,kind,topic,tags,source`. It round-trips through
//!   [`import_delimited`].
//! * **TSV** is ready for Anki's File › Import: Anki's header lines
//!   (`#separator:tab`, `#html:true`, `#notetype column:3`,
//!   `#tags column:4`, `#deck:…`), then Front, Back, the note type
//!   (`Basic` or `Cloze`), and the tags. The source anchor rides at the
//!   end of the back, as Anki's Basic note type has only two fields.
//!
//! Import reads either, and any other CSV or TSV whose first two columns
//! are front and back.

use crate::cards::{Card, CardKind, cloze_numbers};
use crate::material::Anchor;
use crate::text::one_line;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delim {
    Comma,
    Tab,
}

impl Delim {
    fn char(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Tab => '\t',
        }
    }
}

/// The note type Anki uses for a card.
pub fn anki_notetype(kind: CardKind) -> &'static str {
    match kind {
        CardKind::Cloze => "Cloze",
        CardKind::Basic | CardKind::Why => "Basic",
    }
}

/// Tags safe for Anki: no spaces, `::` kept for hierarchy.
pub fn anki_tags(card: &Card) -> Vec<String> {
    let mut tags: Vec<String> = card
        .tags
        .iter()
        .chain(card.topic.iter())
        .map(|t| one_line(t).replace(' ', "_"))
        .filter(|t| !t.is_empty())
        .collect();
    if card.kind == CardKind::Why {
        tags.push("explain-why".into());
    }
    tags.sort();
    tags.dedup();
    tags
}

/// Text as an HTML field: escaped, line breaks as `<br>`.
pub fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("\r\n", "\n")
        .replace('\n', "<br>")
}

/// An HTML field back to text.
pub fn unhtml(s: &str) -> String {
    let s = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<div>", "\n")
        .replace("</div>", "");
    // Drop any other tags.
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

/// What goes after the back in Anki: the source in small text.
pub const SOURCE_MARK: &str = "<br><br><small>Source: ";

/// The back as Anki shows it: the answer, then the source.
pub fn anki_back(card: &Card) -> String {
    let mut b = html(&card.back);
    if let Some(a) = &card.anchor {
        b.push_str(SOURCE_MARK);
        b.push_str(&html(&a.label()));
        b.push_str("</small>");
    }
    if b.starts_with("<br>") {
        b = b.trim_start_matches("<br>").to_owned();
    }
    b
}

fn quote(field: &str, d: Delim) -> String {
    let needs = field.contains(d.char())
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r');
    if needs {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

fn row(fields: &[String], d: Delim) -> String {
    fields
        .iter()
        .map(|f| quote(f, d))
        .collect::<Vec<_>>()
        .join(&d.char().to_string())
}

/// The deck as CSV (Tern's format) or TSV (Anki's import format).
/// Suspended cards are left out.
pub fn export_delimited(cards: &[Card], d: Delim, deck: &str) -> String {
    let mut out = String::new();
    let live = cards.iter().filter(|c| !c.suspended);
    match d {
        Delim::Comma => {
            out.push_str("front,back,kind,topic,tags,source\n");
            for c in live {
                out.push_str(&row(
                    &[
                        c.front.clone(),
                        c.back.clone(),
                        c.kind.key().into(),
                        c.topic.clone().unwrap_or_default(),
                        c.tags.join(" "),
                        c.anchor.as_ref().map(Anchor::label).unwrap_or_default(),
                    ],
                    d,
                ));
                out.push('\n');
            }
        }
        Delim::Tab => {
            out.push_str("#separator:tab\n#html:true\n#notetype column:3\n#tags column:4\n");
            out.push_str(&format!("#deck:{}\n", one_line(deck)));
            for c in live {
                out.push_str(&row(
                    &[
                        html(&c.front),
                        anki_back(c),
                        anki_notetype(c.kind).into(),
                        anki_tags(c).join(" "),
                    ],
                    d,
                ));
                out.push('\n');
            }
        }
    }
    out
}

/// Splits delimited text into rows of fields: quoted fields may hold the
/// delimiter, `""`, and line breaks.
pub fn parse_rows(text: &str, d: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            c if c == d => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            _ => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

/// Reads cards from CSV or TSV: Tern's CSV, Anki's TSV (with or without
/// its `#` header lines), or any file whose first two columns are front
/// and back. Rows without a front are skipped.
pub fn import_delimited(text: &str, now: i64) -> Result<Vec<Card>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut sep: Option<char> = None;
    let mut html_on = false;
    let mut kind_col: Option<usize> = None;
    let mut tags_col: Option<usize> = None;
    let mut body = String::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix('#')
            && let Some((k, v)) = h.split_once(':')
        {
            let v = v.trim();
            match k.trim().to_lowercase().as_str() {
                "separator" => {
                    sep = Some(match v.to_lowercase().as_str() {
                        "tab" | "\t" => '\t',
                        "comma" | "," => ',',
                        "semicolon" | ";" => ';',
                        "pipe" | "|" => '|',
                        "space" => ' ',
                        _ => v.chars().next().unwrap_or(','),
                    })
                }
                "html" => html_on = v.eq_ignore_ascii_case("true"),
                "notetype column" => kind_col = v.parse::<usize>().ok().map(|n| n - 1),
                "tags column" => tags_col = v.parse::<usize>().ok().map(|n| n - 1),
                _ => {}
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    let sep = sep.unwrap_or_else(|| {
        let first = body.lines().next().unwrap_or("");
        if first.contains('\t') {
            '\t'
        } else if first.matches(';').count() > first.matches(',').count() {
            ';'
        } else {
            ','
        }
    });
    let mut rows = parse_rows(&body, sep);
    if rows.is_empty() {
        return Err("The file has no cards in it.".into());
    }
    // Tern's header row names the columns.
    let col = |name: &str, default: Option<usize>| default.map(|d| (name.to_owned(), d));
    let mut cols: Vec<(String, usize)> = Vec::new();
    let head: Vec<String> = rows[0].iter().map(|h| h.trim().to_lowercase()).collect();
    if head.iter().any(|h| h == "front") && head.iter().any(|h| h == "back") {
        for (i, h) in head.iter().enumerate() {
            cols.push((h.clone(), i));
        }
        rows.remove(0);
    } else {
        cols.extend(col("front", Some(0)));
        cols.extend(col("back", Some(1)));
        cols.extend(col("kind", kind_col));
        cols.extend(col("tags", tags_col));
    }
    let at = |r: &[String], name: &str| -> String {
        cols.iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, i)| r.get(*i))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for r in &rows {
        let clean = |s: String| if html_on { unhtml(&s) } else { s };
        let front = clean(at(r, "front"));
        if front.is_empty() {
            continue;
        }
        let mut back_raw = at(r, "back");
        let mut source = at(r, "source");
        // Tern's own TSV: the source sits at the end of the back.
        if let Some(i) = back_raw.find(SOURCE_MARK) {
            let tail = back_raw[i + SOURCE_MARK.len()..].trim_end_matches("</small>");
            if source.is_empty() {
                source = unhtml(tail);
            }
            back_raw.truncate(i);
        }
        let back = clean(back_raw);
        let kind_s = at(r, "kind");
        let kind = CardKind::parse(&kind_s).unwrap_or(if !cloze_numbers(&front).is_empty() {
            CardKind::Cloze
        } else {
            CardKind::Basic
        });
        let mut tags: Vec<String> = at(r, "tags")
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let kind = if kind == CardKind::Basic && tags.iter().any(|t| t == "explain-why") {
            CardKind::Why
        } else {
            kind
        };
        tags.retain(|t| t != "explain-why");
        if kind == CardKind::Cloze && cloze_numbers(&front).is_empty() {
            continue;
        }
        if kind != CardKind::Cloze && back.is_empty() {
            continue;
        }
        let mut c = Card::new(kind, &front, &back, "import", now);
        let topic = at(r, "topic");
        c.topic = (!topic.is_empty()).then_some(topic);
        c.tags = tags;
        if !source.is_empty() {
            c.anchor = Some(Anchor {
                material: String::new(),
                file: source,
                page: None,
                section: None,
            });
        }
        out.push(c);
    }
    if out.is_empty() {
        return Err(
            "No usable cards: each row needs a front and a back (or a {{c1::…}} gap).".into(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards() -> Vec<Card> {
        let mut a = Card::new(
            CardKind::Basic,
            "What is a \"graph\", briefly?",
            "Vertices, joined\nby edges.",
            "you",
            0,
        );
        a.topic = Some("graphs".into());
        a.tags = vec!["graphs".into()];
        a.anchor = Some(Anchor {
            material: "m1".into(),
            file: "algo.pdf".into(),
            page: Some(3),
            section: None,
        });
        let b = Card::new(
            CardKind::Cloze,
            "Dijkstra uses a {{c1::priority queue}} & <heap>.",
            "",
            "you",
            0,
        );
        let mut w = Card::new(
            CardKind::Why,
            "Explain why ice floats.",
            "It is less dense.",
            "you",
            0,
        );
        w.topic = Some("physics".into());
        let mut s = Card::new(CardKind::Basic, "Hidden?", "yes", "you", 0);
        s.suspended = true;
        vec![a, b, w, s]
    }

    #[test]
    fn csv_round_trips() {
        let csv = export_delimited(&cards(), Delim::Comma, "Tern");
        assert!(csv.starts_with("front,back,kind,topic,tags,source\n"));
        assert!(csv.contains("\"What is a \"\"graph\"\", briefly?\""));
        assert!(!csv.contains("Hidden?"), "suspended cards stay home");
        let back = import_delimited(&csv, 7).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].front, "What is a \"graph\", briefly?");
        assert_eq!(back[0].back, "Vertices, joined\nby edges.");
        assert_eq!(back[0].topic.as_deref(), Some("graphs"));
        assert_eq!(back[0].anchor.as_ref().unwrap().file, "algo.pdf p. 3");
        assert_eq!(back[1].kind, CardKind::Cloze);
        assert_eq!(back[2].kind, CardKind::Why);
        assert!(back.iter().all(|c| c.written_by == "import"));
    }

    #[test]
    fn tsv_is_anki_ready_and_reads_back() {
        let tsv = export_delimited(&cards(), Delim::Tab, "My project");
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines[0], "#separator:tab");
        assert_eq!(lines[4], "#deck:My project");
        let first: Vec<&str> = lines[5].split('\t').collect();
        assert_eq!(first.len(), 4);
        assert_eq!(
            first[1],
            "Vertices, joined<br>by edges.<br><br><small>Source: algo.pdf p. 3</small>"
        );
        assert_eq!(first[2], "Basic");
        assert_eq!(first[3], "graphs");
        assert!(lines[6].contains("{{c1::priority queue}} &amp; &lt;heap&gt;."));
        assert!(lines[6].ends_with("\tCloze\t"));
        assert!(lines[7].ends_with("Basic\texplain-why physics"));
        let back = import_delimited(&tsv, 0).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].back, "Vertices, joined\nby edges.");
        assert_eq!(back[0].anchor.as_ref().unwrap().file, "algo.pdf p. 3");
        assert_eq!(
            back[1].front,
            "Dijkstra uses a {{c1::priority queue}} & <heap>."
        );
        assert_eq!(back[2].kind, CardKind::Why);
        assert_eq!(back[2].tags, vec!["physics".to_string()]);
    }

    #[test]
    fn plain_files_without_headers_import() {
        let c = import_delimited("Capital of France;Paris\nH2O;Water\n;skipped\n", 0).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].back, "Water");
        let t = import_delimited("A {{c1::gap}} here\t\nQ\tA\n", 0).unwrap();
        assert_eq!(t[0].kind, CardKind::Cloze);
        assert!(import_delimited("", 0).is_err());
        assert!(import_delimited("only fronts\nno backs\n", 0).is_err());
        let rows = parse_rows("a,\"b,\nc\",d\n", ',');
        assert_eq!(rows, vec![vec!["a", "b,\nc", "d"]]);
    }
}
