//! Mermaid as models write it (#72 round 2).
//!
//! Mermaid's flowchart grammar refuses a node label with parentheses,
//! brackets, braces, `|`, `"` or a bare `@` unless the label is quoted,
//! and models write `D1[Linear (d_model → 4*d_model)]` all the time; the
//! diagram then stayed a code block. When merman cannot read a diagram,
//! [`tolerant`] rewrites it the way Mermaid's own docs say to and the
//! diagram is drawn again:
//!
//! - node labels (`[…]`, `(…)`, `{…}`, `([…])`, `[[…]]`, `((…))`, `{{…}}`,
//!   `[/…/]`, `>…]`) and edge labels (`|…|`) with those characters are
//!   quoted, a `"` inside becoming `#quot;`;
//! - a subgraph title with them is quoted;
//! - `%%` comments are dropped (whole lines and the end of a line), but
//!   not `%%{init}%%` directives.

/// `source` rewritten so Mermaid reads it, or `None` when nothing needed
/// changing.
pub fn tolerant(source: &str) -> Option<String> {
    let flowchart = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"))
        .is_some_and(|l| {
            let w = l.split_whitespace().next().unwrap_or("");
            w.eq_ignore_ascii_case("flowchart") || w.eq_ignore_ascii_case("graph")
        });
    let mut out = String::with_capacity(source.len() + 32);
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("%%") && !trimmed.starts_with("%%{") {
            continue;
        }
        let line = strip_trailing_comment(line);
        if line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if flowchart {
            out.push_str(&flow_line(line));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    let out = out.trim_end().to_owned();
    (out != source.trim_end()).then_some(out)
}

/// `line` without a `%% …` comment after its statement (outside quotes).
fn strip_trailing_comment(line: &str) -> &str {
    if line.trim_start().starts_with("%%{") {
        return line;
    }
    let mut quoted = false;
    let b = line.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'"' => quoted = !quoted,
            b'%' if !quoted && b.get(i + 1) == Some(&b'%') && i > 0 => {
                if line[i..].starts_with("%%{") {
                    return line;
                }
                return line[..i].trim_end();
            }
            _ => {}
        }
    }
    line
}

/// Characters a label can only hold when quoted.
fn needs_quotes(label: &str) -> bool {
    label.contains(['(', ')', '[', ']', '{', '}', '|', '"', '@', ';'])
}

/// `label` in quotes, its own quotes as `#quot;`. (A `*` stays: the bold
/// italic `4*d_model)` was the SVG renderer's font fallback for `→`, not
/// emphasis; see third_party/gpui-pre/TERN_PATCH.md.)
fn quote(label: &str) -> String {
    format!("\"{}\"", label.trim().replace('"', "#quot;"))
}

fn is_quoted(label: &str) -> bool {
    let t = label.trim();
    (t.len() >= 2 && t.starts_with('"') && t.ends_with('"')) || t.starts_with('`')
}

/// Node shapes: opening, closing. Longest openings first.
const SHAPES: &[(&str, &str)] = &[
    ("(((", ")))"),
    ("((", "))"),
    ("([", "])"),
    ("[[", "]]"),
    ("[(", ")]"),
    ("[/", "/]"),
    ("[\\", "\\]"),
    ("{{", "}}"),
    ("(", ")"),
    ("[", "]"),
    ("{", "}"),
    (">", "]"),
];

const KEYWORDS: &[&str] = &[
    "classDef",
    "class",
    "style",
    "linkStyle",
    "click",
    "direction",
    "accTitle",
    "accDescr",
];

/// One flowchart statement line with its labels quoted where needed.
fn flow_line(line: &str) -> String {
    let lead = &line[..line.len() - line.trim_start().len()];
    let body = line.trim_start();
    let first = body.split_whitespace().next().unwrap_or("");
    if KEYWORDS.contains(&first)
        || first.eq_ignore_ascii_case("flowchart")
        || first.eq_ignore_ascii_case("graph")
        || first == "end"
    {
        return line.to_owned();
    }
    if first == "subgraph" {
        let rest = body["subgraph".len()..].trim();
        if !rest.is_empty() && !is_quoted(rest) && !rest.contains('[') && needs_quotes(rest) {
            return format!("{lead}subgraph {}", quote(rest));
        }
        if !rest.contains('[') {
            return line.to_owned();
        }
    }
    let mut out = String::with_capacity(line.len() + 8);
    out.push_str(lead);
    let b = body.as_bytes();
    let mut i = 0;
    let mut id_len = 0usize;
    while i < body.len() {
        let c = b[i];
        if c == b'"' {
            // A quoted stretch is copied whole.
            let end = body[i + 1..]
                .find('"')
                .map_or(body.len(), |e| i + 1 + e + 1);
            out.push_str(&body[i..end]);
            i = end;
            id_len = 0;
            continue;
        }
        if c == b'|' {
            if let Some(e) = body[i + 1..].find('|') {
                let label = &body[i + 1..i + 1 + e];
                out.push('|');
                if !is_quoted(label) && needs_quotes(label) {
                    out.push_str(&quote(label));
                } else {
                    out.push_str(label);
                }
                out.push('|');
                i += e + 2;
                id_len = 0;
                continue;
            }
        }
        if id_len > 0
            && let Some((open, close)) = SHAPES.iter().find(|(o, _)| body[i..].starts_with(o))
            && let Some(end) = closing(body, i + open.len(), open, close)
        {
            let label = &body[i + open.len()..end];
            out.push_str(open);
            if !is_quoted(label) && needs_quotes(label) {
                out.push_str(&quote(label));
            } else {
                out.push_str(label);
            }
            out.push_str(close);
            i = end + close.len();
            id_len = 0;
            continue;
        }
        let ch = body[i..].chars().next().unwrap_or(' ');
        if ch.is_alphanumeric() || ch == '_' {
            id_len += 1;
        } else if !(ch == '-' && id_len > 0 && body[i + 1..].starts_with(char::is_alphanumeric)) {
            id_len = 0;
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Where the label opened by `open` at byte `from` closes: for one-
/// character brackets, at the matching bracket (nested pairs counted);
/// for the longer shapes, at the last `close` on the line before the next
/// arrow.
fn closing(body: &str, from: usize, open: &str, close: &str) -> Option<usize> {
    let rest = &body[from..];
    if open.len() == 1 && close.len() == 1 && open != ">" {
        let (o, c) = (open.as_bytes()[0], close.as_bytes()[0]);
        let mut depth = 0i32;
        let mut quoted = false;
        for (k, ch) in rest.bytes().enumerate() {
            if ch == b'"' {
                quoted = !quoted;
            }
            if quoted {
                continue;
            }
            if ch == o {
                depth += 1;
            } else if ch == c {
                if depth == 0 {
                    return Some(from + k);
                }
                depth -= 1;
            }
        }
        return None;
    }
    let stop = [
        "-->", "---", "-.-", "==>", "-.->", "~~~", "--o", "--x", " & ",
    ]
    .iter()
    .filter_map(|a| rest.find(a))
    .min()
    .unwrap_or(rest.len());
    rest[..stop].rfind(close).map(|k| from + k)
}

/// What openai/gpt-oss-120b wrote for a Transformer diagram (the tests
/// and the UI tour draw it).
pub const SAMPLE: &str = r#"flowchart TD
    %% Encoder stack
    A[Input Embeddings + Positional Encoding] --> B[Multi-Head Attention]
    subgraph "Multi-Head Attention"
        B --> C[Add & Norm]
    end
    C --> D1[Linear (d_model → 4*d_model)] %% feed-forward
    D1 --> D2[ReLU]
    D2 --> D3[Linear (4*d_model → d_model)]
    D3 --> E{Output (logits)}
    E -->|softmax (probabilities)| F([Next token])"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_with_parentheses_are_quoted() {
        let out = tolerant(SAMPLE).expect("changed");
        assert!(
            out.contains("D1[\"Linear (d_model → 4*d_model)\"] --> ")
                || out.contains("C --> D1[\"Linear (d_model → 4*d_model)\"]"),
            "{out}"
        );
        assert!(
            out.contains("D3[\"Linear (4*d_model → d_model)\"]"),
            "{out}"
        );
        assert!(out.contains(r#"E{"Output (logits)"}"#), "{out}");
        assert!(
            out.contains(r#"-->|"softmax (probabilities)"| F([Next token])"#),
            "{out}"
        );
        // Untouched: plain labels, the quoted subgraph, the header.
        assert!(
            out.contains("A[Input Embeddings + Positional Encoding] --> B[Multi-Head Attention]"),
            "{out}"
        );
        assert!(out.contains(r#"subgraph "Multi-Head Attention""#), "{out}");
        assert!(out.contains("D1 --> D2[ReLU]"), "{out}");
        assert!(out.starts_with("flowchart TD"), "{out}");
        assert!(!out.contains("%%"), "{out}");
    }

    #[test]
    fn quoted_and_clean_diagrams_are_left_alone() {
        assert_eq!(tolerant("flowchart LR\n  A[\"x (y)\"] --> B[Plain]"), None);
        assert_eq!(tolerant("sequenceDiagram\n  A->>B: hi (there)"), None);
        assert_eq!(
            tolerant("%%{init: {'theme':'dark'}}%%\ngraph TD\n  A-->B"),
            None
        );
        let sub = tolerant("graph TD\n  subgraph Encoder (6 layers)\n    A-->B\n  end").unwrap();
        assert!(sub.contains(r#"subgraph "Encoder (6 layers)""#), "{sub}");
        let quotes = tolerant("graph TD\n  A[say \"hi\" (now)] --> B").unwrap();
        assert!(
            quotes.contains(r#"A["say #quot;hi#quot; (now)"]"#),
            "{quotes}"
        );
    }
}
