//! LaTeX delimiters in model answers (#72 round 2).
//!
//! Models write formulas as `\( … \)` and `\[ … \]` (and bare
//! `\begin{align*} … \end{align*}`) as often as `$…$`. Markdown reads
//! `\(` and `\[` as escaped punctuation and drops the backslash, so the
//! reader saw `[ \alpha = \text{softmax}…` instead of a formula.
//! [`delimiters`] rewrites those spans to the dollar forms the Markdown
//! parser keeps verbatim, before the answer is parsed:
//!
//! - `\( x \)` becomes `$$x$$` inside the line: inline math that the
//!   inline plugin always typesets (a single `$` would go through the
//!   dollar-amount guess, and `\(5\)` is still math).
//! - `\[ x \]` and a bare `\begin{env} … \end{env}` become a `$$` block on
//!   lines of their own, indented like the line they were on (a formula in
//!   a list item stays in it).
//!
//! Code spans, fenced code blocks and `$$` blocks are left alone. A block
//! still streaming (no closing `\]` or `\end` yet) opens its `$$` block,
//! which the text view shows as source until it closes.
//!
//! [`ratex_candidates`] turns a formula into what RaTeX can lay out:
//! numbered environments become their unnumbered inner forms (`align` →
//! `aligned`, `gather` → `gathered`, `equation` dropped), `\label` and
//! `\nonumber` go, and rows written without an environment are wrapped
//! in `aligned`.

use std::borrow::Cow;

/// Environments a model writes on their own, outside any math delimiters.
const DISPLAY_ENVS: &[&str] = &[
    "equation",
    "equation*",
    "align",
    "align*",
    "aligned",
    "alignat",
    "alignat*",
    "gather",
    "gather*",
    "gathered",
    "multline",
    "multline*",
    "eqnarray",
    "eqnarray*",
    "split",
    "cases",
    "matrix",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "array",
];

/// `text` with `\( \)`, `\[ \]` and bare display environments rewritten to
/// dollar math. Borrowed when there is nothing to rewrite.
pub fn delimiters(text: &str) -> Cow<'_, str> {
    if !text.contains("\\(") && !text.contains("\\[") && !text.contains("\\begin{") {
        return Cow::Borrowed(text);
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len() + 32);
    let mut fence: Option<String> = None;
    let mut dollar_block = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        // Fenced code: verbatim, fence to fence.
        let marker: String = trimmed
            .chars()
            .take_while(|c| *c == '`' || *c == '~')
            .collect();
        if marker.len() >= 3 && !dollar_block {
            match &fence {
                Some(f) if marker.starts_with(f.as_str()) && trimmed.len() == marker.len() => {
                    fence = None
                }
                None => fence = Some(marker),
                _ => {}
            }
            out.push_str(line);
            i += 1;
            continue;
        }
        if fence.is_some() {
            out.push_str(line);
            i += 1;
            continue;
        }
        // `$$` blocks: verbatim too.
        if trimmed == "$$" || (trimmed.starts_with("$$") && !trimmed[2..].contains('$')) {
            dollar_block = !dollar_block;
            out.push_str(line);
            i += 1;
            continue;
        }
        if dollar_block {
            out.push_str(line);
            i += 1;
            continue;
        }
        i = rewrite_from(&lines, i, &mut out);
    }
    Cow::Owned(out)
}

/// Rewrites line `start` (and, for a display formula spanning lines, the
/// lines it takes) into `out`; returns the next line to read.
fn rewrite_from(lines: &[&str], start: usize, out: &mut String) -> usize {
    let line = lines[start];
    let indent = block_indent(line);
    let bytes = line.as_bytes();
    let mut k = 0;
    let mut seg_start = 0;
    while k < line.len() {
        let c = bytes[k];
        if c == b'`' {
            // A code span: through its closing run of the same length.
            let run = line[k..].bytes().take_while(|b| *b == b'`').count();
            let ticks = &line[k..k + run];
            match line[k + run..].find(ticks) {
                Some(end) => k = k + run + end + run,
                None => k += run,
            }
            continue;
        }
        if c != b'\\' {
            k += line[k..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let rest = &line[k..];
        if rest.starts_with("\\\\") {
            k += 2;
            continue;
        }
        if let Some(after_open) = rest.strip_prefix("\\(") {
            if let Some(end) = after_open.find("\\)") {
                let inner = after_open[..end].trim();
                if !inner.is_empty() && !inner.contains('$') {
                    out.push_str(&line[seg_start..k]);
                    out.push_str("$$");
                    out.push_str(inner);
                    out.push_str("$$");
                    k += 2 + end + 2;
                    seg_start = k;
                    continue;
                }
            }
            k += 2;
            continue;
        }
        let display = if rest.starts_with("\\[") {
            Some(("\\]", 2usize, false))
        } else {
            // The `\begin` stays part of the formula.
            begun_env(rest).map(|e| (leak_end(e), 0usize, true))
        };
        if let Some((close, open_len, keep)) = display {
            if let Some((body, next_line, after)) = find_close(lines, start, k + open_len, close) {
                // `find_close` keeps the `\end{…}` of an environment but not
                // its opening; put the opening back.
                let formula = if keep {
                    format!("{}{body}", begin_text(rest))
                } else {
                    body
                };
                let before = &line[seg_start..k];
                let take = keep
                    || (looks_mathy(&formula) && (after.is_some() || before.trim().is_empty()));
                if take {
                    push_block(out, before, &indent, &formula, after.is_some());
                    let Some(tail) = after else {
                        return lines.len();
                    };
                    let tail_line = lines.get(next_line).copied().unwrap_or("");
                    let tail = &tail_line[tail.min(tail_line.len())..];
                    if !tail.trim().is_empty() {
                        // The rest of the line, as a line of its own.
                        let mut rest_out = String::new();
                        rewrite_from(&[tail], 0, &mut rest_out);
                        out.push_str(&indent);
                        out.push_str(rest_out.trim_start());
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                    return next_line + 1;
                }
            }
            k += 2;
            continue;
        }
        k += 1;
    }
    out.push_str(&line[seg_start..]);
    start + 1
}

/// The environment a `\begin{env}` at the start of `rest` opens, when it
/// is one written as a display formula on its own.
fn begun_env(rest: &str) -> Option<&str> {
    let name = rest.strip_prefix("\\begin{")?;
    let end = name.find('}')?;
    let env = &name[..end];
    DISPLAY_ENVS.contains(&env).then_some(env)
}

/// `\begin{env}` as written at the start of `rest`.
fn begin_text(rest: &str) -> &str {
    let end = rest.find('}').map_or(rest.len(), |e| e + 1);
    &rest[..end]
}

/// `\end{env}` for a known environment name (static, from the table).
fn leak_end(env: &str) -> &'static str {
    const ENDS: &[&str] = &[
        "\\end{equation}",
        "\\end{equation*}",
        "\\end{align}",
        "\\end{align*}",
        "\\end{aligned}",
        "\\end{alignat}",
        "\\end{alignat*}",
        "\\end{gather}",
        "\\end{gather*}",
        "\\end{gathered}",
        "\\end{multline}",
        "\\end{multline*}",
        "\\end{eqnarray}",
        "\\end{eqnarray*}",
        "\\end{split}",
        "\\end{cases}",
        "\\end{matrix}",
        "\\end{pmatrix}",
        "\\end{bmatrix}",
        "\\end{Bmatrix}",
        "\\end{vmatrix}",
        "\\end{Vmatrix}",
        "\\end{array}",
    ];
    ENDS.iter()
        .find(|e| &e[5..e.len() - 1] == env)
        .copied()
        .unwrap_or("\\end{}")
}

/// Finds `close` from byte `from` of line `start` on. Returns the text in
/// between (lines joined with `\n`), the line the close is on, and the byte
/// after it on that line; `None` for that byte when the text ends first (a
/// formula still streaming), with everything to the end as its body.
fn find_close(
    lines: &[&str],
    start: usize,
    from: usize,
    close: &str,
) -> Option<(String, usize, Option<usize>)> {
    let mut body = String::new();
    let mut l = start;
    let mut at = from;
    // For `\begin…\end`, the opening `\begin{env}` is skipped and nested
    // environments of the same name are counted.
    let begin = close.replacen("\\end{", "\\begin{", 1);
    let mut depth = 0usize;
    if close.starts_with("\\end{") {
        at += begin.len();
    }
    loop {
        let line = lines.get(l)?;
        let seg = line.get(at..).unwrap_or("");
        let mut search = 0;
        loop {
            let found = seg[search..].find(close).map(|p| p + search);
            let nested = if close.starts_with("\\end{") {
                seg[search..].find(begin.as_str()).map(|p| p + search)
            } else {
                None
            };
            match (found, nested) {
                (Some(f), Some(n)) if n < f => {
                    depth += 1;
                    search = n + begin.len();
                }
                (Some(f), _) if depth > 0 => {
                    depth -= 1;
                    search = f + close.len();
                }
                (Some(f), _) => {
                    body.push_str(&seg[..f]);
                    let body = if close.starts_with("\\end{") {
                        format!("{}{close}", body)
                    } else {
                        body
                    };
                    return Some((body, l, Some(at + f + close.len())));
                }
                (None, _) => break,
            }
        }
        body.push_str(seg);
        l += 1;
        at = 0;
        if l >= lines.len() {
            // Streaming: no close yet.
            return Some((body, lines.len(), None));
        }
    }
}

/// Whether `\[ … \]` holds a formula rather than an escaped bracket
/// (`\[1\]`, `\[sic\]`).
fn looks_mathy(s: &str) -> bool {
    s.contains('\\')
        || s.contains('^')
        || s.contains('_')
        || s.contains('=')
        || s.contains('{')
        || s.contains('+')
}

/// The indentation a block on this line takes: its leading spaces and,
/// on a list item's first line, the marker's width too.
fn block_indent(line: &str) -> String {
    let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[lead..];
    let marker = if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
        2
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && digits < 10
            && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
            && rest.as_bytes().get(digits + 1) == Some(&b' ')
        {
            digits + 2
        } else {
            0
        }
    };
    " ".repeat(lead + marker)
}

/// Writes `before` (the line up to the formula), then the formula as a
/// `$$` block on lines of its own at `indent`.
fn push_block(out: &mut String, before: &str, indent: &str, formula: &str, closed: bool) {
    let before_trim = before.trim_end();
    if !before_trim.trim().is_empty() {
        out.push_str(before_trim);
        out.push('\n');
    }
    out.push_str(indent);
    out.push_str("$$\n");
    for l in formula.trim().lines() {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        out.push_str(indent);
        out.push_str(l);
        out.push('\n');
    }
    // Still streaming: the block stays open, so the text view knows not to
    // call it unreadable yet.
    if closed {
        out.push_str(indent);
        out.push_str("$$\n");
    }
}

/// Whether a `$$` block's source (as written) has its closing `$$`: one
/// still streaming has none yet.
pub fn math_closed(source: &str) -> bool {
    let t = source.trim();
    let mut lines = t.lines();
    let first = lines.next().unwrap_or_default().trim();
    let last = t.lines().last().unwrap_or_default().trim();
    first.starts_with("$$") && t.lines().count() >= 2 && last.ends_with("$$")
}

/// What to try laying out, in order, for `latex`: the formula with
/// numbered environments made unnumbered and labels dropped, then the
/// same wrapped in `aligned` when it has rows (`\\`) or columns (`&`) at
/// the top level.
pub fn ratex_candidates(latex: &str) -> Vec<String> {
    let mut s = latex.trim().to_owned();
    for (from, to) in [
        ("\\begin{align*}", "\\begin{aligned}"),
        ("\\end{align*}", "\\end{aligned}"),
        ("\\begin{align}", "\\begin{aligned}"),
        ("\\end{align}", "\\end{aligned}"),
        ("\\begin{eqnarray*}", "\\begin{aligned}"),
        ("\\end{eqnarray*}", "\\end{aligned}"),
        ("\\begin{eqnarray}", "\\begin{aligned}"),
        ("\\end{eqnarray}", "\\end{aligned}"),
        ("\\begin{alignat*}", "\\begin{alignedat}"),
        ("\\end{alignat*}", "\\end{alignedat}"),
        ("\\begin{alignat}", "\\begin{alignedat}"),
        ("\\end{alignat}", "\\end{alignedat}"),
        ("\\begin{gather*}", "\\begin{gathered}"),
        ("\\end{gather*}", "\\end{gathered}"),
        ("\\begin{gather}", "\\begin{gathered}"),
        ("\\end{gather}", "\\end{gathered}"),
        ("\\begin{multline*}", "\\begin{gathered}"),
        ("\\end{multline*}", "\\end{gathered}"),
        ("\\begin{multline}", "\\begin{gathered}"),
        ("\\end{multline}", "\\end{gathered}"),
        ("\\begin{equation*}", ""),
        ("\\end{equation*}", ""),
        ("\\begin{equation}", ""),
        ("\\end{equation}", ""),
        ("\\nonumber", ""),
        ("\\notag", ""),
    ] {
        s = s.replace(from, to);
    }
    s = strip_command_arg(&s, "\\label");
    s = strip_command_arg(&s, "\\tag");
    let s = s.trim().to_owned();
    let mut out = vec![s.clone()];
    if !s.starts_with("\\begin{") && (s.contains("\\\\") || top_level_amp(&s)) {
        out.push(format!("\\begin{{aligned}}{s}\\end{{aligned}}"));
    }
    out
}

/// `s` without `cmd{…}` (its braced argument too).
fn strip_command_arg(s: &str, cmd: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find(cmd) {
        let after = &rest[p + cmd.len()..];
        // `\labelx` is another command.
        if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
            out.push_str(&rest[..p + cmd.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..p]);
        let a = after.trim_start();
        if let Some(inner) = a.strip_prefix('{') {
            let mut depth = 1;
            let mut end = inner.len();
            for (i, c) in inner.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &inner[end.min(inner.len())..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Whether `s` has an `&` outside braces and environments.
fn top_level_amp(s: &str) -> bool {
    let mut depth = 0i32;
    let b = s.as_bytes();
    for (i, c) in b.iter().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'&' if depth == 0 && (i == 0 || b[i - 1] != b'\\') => return true,
            _ => {}
        }
    }
    false
}

/// An answer as openai/gpt-oss-120b wrote it, which showed raw LaTeX
/// before (the tests and the UI tour use it).
pub const SAMPLE: &str = "The attention is\n\n\\[ \\alpha = \\text{softmax}\\left(\\frac{Q K^T}{\\sqrt{d_k}}\\right) V \\]\n\nwhere:\n\n- \\(Q\\): Query matrix (\\(n \\times d_k\\))\n- \\(K\\): Key matrix\n\nThe projections:\n\n\\[\n\\begin{align*}\nQ &= X W^Q \\\\\nK &= X W^K \\\\\nV &= X W^V\n\\end{align*}\n\\]\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paren_and_bracket_delimiters_become_dollar_math() {
        let out = delimiters(SAMPLE);
        assert!(
            out.contains(
                "$$\n\\alpha = \\text{softmax}\\left(\\frac{Q K^T}{\\sqrt{d_k}}\\right) V\n$$\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("- $$Q$$: Query matrix ($$n \\times d_k$$)"),
            "{out}"
        );
        assert!(out.contains("- $$K$$: Key matrix"), "{out}");
        assert!(
            out.contains("$$\n\\begin{align*}\nQ &= X W^Q \\\\\nK &= X W^K \\\\\nV &= X W^V\n\\end{align*}\n$$\n"),
            "{out}"
        );
        assert!(!out.contains("\\["), "{out}");
        assert!(!out.contains("\\("), "{out}");
    }

    #[test]
    fn align_inside_brackets_and_bare_environments() {
        let text = "Steps:\n\\[\n\\begin{align*}\nQ &= X W^Q \\\\\nK &= X W^K\n\\end{align*}\n\\]\nDone.\n";
        let out = delimiters(text);
        assert_eq!(
            out,
            "Steps:\n$$\n\\begin{align*}\nQ &= X W^Q \\\\\nK &= X W^K\n\\end{align*}\n$$\nDone.\n"
        );
        let bare = "Then\n\\begin{align}\na &= b \\\\\nc &= d\n\\end{align}\nafter\n";
        let out = delimiters(bare);
        assert_eq!(
            out,
            "Then\n$$\n\\begin{align}\na &= b \\\\\nc &= d\n\\end{align}\n$$\nafter\n"
        );
        // One line, mid-paragraph.
        let inline = "so \\[ x^2 + y^2 = r^2 \\] holds.\n";
        assert_eq!(delimiters(inline), "so\n$$\nx^2 + y^2 = r^2\n$$\nholds.\n");
    }

    #[test]
    fn code_escaped_brackets_and_dollar_blocks_are_left_alone() {
        let text = "`\\(x\\)` and\n```\n\\[ y = 1 \\]\n```\n$$\n\\(z\\)\n$$\nsee \\[1\\] here\n";
        assert_eq!(delimiters(text), text);
        assert!(matches!(delimiters("plain $x$ text"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_formula_in_a_list_item_stays_in_it() {
        let out = delimiters("1. Energy: \\[ E = mc^2 \\]\n2. Next\n");
        assert_eq!(out, "1. Energy:\n   $$\n   E = mc^2\n   $$\n2. Next\n");
    }

    #[test]
    fn a_streaming_display_formula_opens_its_block() {
        let out = delimiters("Look:\n\\[\n\\frac{a}{b");
        assert_eq!(out, "Look:\n$$\n\\frac{a}{b\n");
        assert!(!math_closed("$$\n\\frac{a}{b\n"));
        assert!(math_closed("$$\nx\n$$"));
        assert!(math_closed("   $$\n   x\n   $$\n"));
    }

    #[test]
    fn numbered_environments_become_ones_ratex_lays_out() {
        let c = ratex_candidates(
            "\\begin{align*}\nQ &= X W^Q \\label{eq:q} \\\\\nK &= X W^K \\nonumber\n\\end{align*}",
        );
        assert_eq!(
            c[0],
            "\\begin{aligned}\nQ &= X W^Q  \\\\\nK &= X W^K \n\\end{aligned}"
        );
        let rows = ratex_candidates("a &= b \\\\ c &= d");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], "\\begin{aligned}a &= b \\\\ c &= d\\end{aligned}");
        assert_eq!(
            ratex_candidates("\\begin{equation}x=1\\end{equation}")[0],
            "x=1"
        );
    }
}
