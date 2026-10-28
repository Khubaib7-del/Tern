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
    if !text.contains("\\(")
        && !text.contains("\\[")
        && !text.contains("\\begin{")
        && !has_math_fence(text)
    {
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
            // A ```latex fence that holds formulas, not a TeX document.
            if fence.is_none() {
                if let Some(next) = math_fence(&lines, i, &marker, &mut out) {
                    i = next;
                    continue;
                }
            }
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

/// Fence languages a model wraps formulas in.
const MATH_FENCES: &[&str] = &["latex", "tex", "math", "katex"];

/// Whether `text` has a fence opened with one of [`MATH_FENCES`].
fn has_math_fence(text: &str) -> bool {
    text.lines().any(|l| fence_lang(l.trim()).is_some())
}

/// The math language a fence line opens (` ```latex `), lower-cased.
fn fence_lang(trimmed: &str) -> Option<String> {
    let marker = trimmed
        .chars()
        .take_while(|c| *c == '`' || *c == '~')
        .count();
    if marker < 3 {
        return None;
    }
    let lang = trimmed[marker..]
        .split_whitespace()
        .next()?
        .trim_matches(|c| c == '{' || c == '}' || c == '.')
        .to_ascii_lowercase();
    MATH_FENCES.contains(&lang.as_str()).then_some(lang)
}

/// What a math-language fence's body is.
#[derive(Debug, PartialEq, Eq)]
enum FenceBody {
    /// A TeX document or macro code: stays a code block.
    Code,
    /// Prose and formulas with their own delimiters: read as Markdown.
    Delimited,
    /// One bare formula: a display math block.
    Formula,
}

/// Sorts a ```latex fence's body (see [`FenceBody`]).
fn fence_body(body: &str) -> FenceBody {
    const DOCUMENT: &[&str] = &[
        "\\documentclass",
        "\\usepackage",
        "\\begin{document}",
        "\\newcommand",
        "\\renewcommand",
        "\\def\\",
        "\\section",
        "\\chapter",
        "\\maketitle",
        "\\input{",
        "\\include{",
    ];
    let t = body.trim();
    if t.is_empty() || DOCUMENT.iter().any(|d| t.contains(d)) {
        return FenceBody::Code;
    }
    if t.contains("\\(") || t.contains("\\[") || t.contains('$') {
        return FenceBody::Delimited;
    }
    if t.contains("\\begin{") {
        // `\begin{align*}…` alone is a formula; with prose around it the
        // Markdown pass lifts the environment out.
        return if prose_outside(t) {
            FenceBody::Delimited
        } else {
            FenceBody::Formula
        };
    }
    if looks_mathy(t) && !prose_outside(t) {
        FenceBody::Formula
    } else {
        FenceBody::Code
    }
}

/// Whether `t` has a run of plain words outside `\text{…}` and other
/// braces: "Where the score is" rather than `\frac{a}{b} = x`.
fn prose_outside(t: &str) -> bool {
    let mut flat = String::with_capacity(t.len());
    let mut depth = 0i32;
    let mut cmd = false;
    for c in t.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '\\' => {
                cmd = true;
                flat.push(' ');
                continue;
            }
            _ if depth > 0 => {}
            c if cmd && c.is_ascii_alphabetic() => continue,
            c => flat.push(c),
        }
        cmd = false;
    }
    // Three words of two or more letters in a row.
    let mut run = 0;
    for w in flat.split(|c: char| c.is_whitespace()) {
        if w.is_empty() {
            continue;
        }
        let word = w.trim_end_matches([',', '.', ':', ';']);
        if word.len() >= 2 && word.chars().all(|c| c.is_alphabetic()) {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// A ```latex / ```tex / ```math / ```katex fence opened on line `start`
/// whose body is math: writes it as Markdown math into `out` and returns
/// the line after the fence. `None` (nothing written) leaves the fence a
/// code block: another language, a TeX document, or one still streaming
/// without delimiters yet.
fn math_fence(lines: &[&str], start: usize, marker: &str, out: &mut String) -> Option<usize> {
    let open = lines[start];
    fence_lang(open.trim())?;
    let indent: String = open
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let close = lines[start + 1..].iter().position(|l| {
        let t = l.trim();
        t.starts_with(marker) && t.chars().all(|c| c == marker.chars().next().unwrap_or('`'))
    });
    let end = close.map_or(lines.len(), |c| start + 1 + c);
    let body_lines = &lines[start + 1..end];
    let common = body_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let body: String = body_lines
        .iter()
        .map(|l| {
            let cut = common.min(l.len() - l.trim_start_matches([' ', '\t']).len());
            &l[cut..]
        })
        .collect();
    let kind = fence_body(&body);
    let next = if close.is_some() {
        end + 1
    } else {
        lines.len()
    };
    match kind {
        FenceBody::Code => None,
        // A bare formula is only known whole once its fence closes.
        FenceBody::Formula if close.is_none() => None,
        FenceBody::Formula => {
            push_block(out, "", &indent, &body, true);
            ensure_gap(lines, next, out);
            Some(next)
        }
        FenceBody::Delimited => {
            let inner = delimiters(&body);
            for l in inner.split_inclusive('\n') {
                if !l.trim().is_empty() {
                    out.push_str(&indent);
                }
                out.push_str(l);
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            ensure_gap(lines, next, out);
            Some(next)
        }
    }
}

/// A blank line after an unwrapped fence, so the text after it starts a
/// paragraph of its own.
fn ensure_gap(lines: &[&str], next: usize, out: &mut String) {
    if lines.get(next).is_some_and(|l| !l.trim().is_empty()) {
        out.push('\n');
    }
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

/// Operators models write as if they were defined (`\\softmax`), and what
/// each becomes when RaTeX does not know it. Anything else unknown becomes
/// `\\operatorname{name}` (see [`lenient`]).
const OPERATORS: &[(&str, &str)] = &[
    ("softmax", "\\operatorname*{softmax}"),
    ("argmax", "\\operatorname*{arg\\,max}"),
    ("argmin", "\\operatorname*{arg\\,min}"),
    ("sign", "\\operatorname{sign}"),
    ("sgn", "\\operatorname{sgn}"),
    ("ReLU", "\\operatorname{ReLU}"),
    ("relu", "\\operatorname{ReLU}"),
    ("GELU", "\\operatorname{GELU}"),
    ("sigmoid", "\\operatorname{sigmoid}"),
    ("diag", "\\operatorname{diag}"),
    ("tr", "\\operatorname{tr}"),
    ("Tr", "\\operatorname{Tr}"),
    ("trace", "\\operatorname{trace}"),
    ("rank", "\\operatorname{rank}"),
    ("Var", "\\operatorname{Var}"),
    ("Cov", "\\operatorname{Cov}"),
    ("E", "\\mathbb{E}"),
    ("Prob", "\\operatorname{P}"),
    ("KL", "\\operatorname{KL}"),
    ("LayerNorm", "\\operatorname{LayerNorm}"),
    ("Attention", "\\operatorname{Attention}"),
    ("MultiHead", "\\operatorname{MultiHead}"),
    ("Concat", "\\operatorname{Concat}"),
];

/// The command a RaTeX "Undefined control sequence: \\name" error names.
pub fn undefined_macro(error: &str) -> Option<&str> {
    let at = error.find("Undefined control sequence: \\")?;
    let rest = &error[at + "Undefined control sequence: \\".len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// `s` with the command `\\name` (not `\\namefoo`) replaced by `with`.
pub fn replace_command(s: &str, name: &str, with: &str) -> String {
    let pat = format!("\\{name}");
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s;
    while let Some(p) = rest.find(&pat) {
        let after = &rest[p + pat.len()..];
        out.push_str(&rest[..p]);
        if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
            out.push_str(&pat);
        } else {
            out.push_str(with);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Parses `latex` with `parse`; when it stops at an unknown command, that
/// command is defined as an operator ([`OPERATORS`], else
/// `\\operatorname{name}`) and it tries again, a few times at most. Each
/// retry is written to `tern.log`.
pub fn lenient<T>(latex: &str, parse: impl Fn(&str) -> Result<T, String>) -> Option<T> {
    let mut src = latex.to_owned();
    for _ in 0..8 {
        match parse(&src) {
            Ok(t) => return Some(t),
            Err(e) => {
                let name = undefined_macro(&e)?.to_owned();
                let with = OPERATORS
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, w)| (*w).to_owned())
                    .unwrap_or_else(|| format!("\\operatorname{{{name}}}"));
                let next = replace_command(&src, &name, &with);
                if next == src {
                    log_retry(&format!(
                        "math: unknown \\{name} could not be replaced; shown as text"
                    ));
                    return None;
                }
                log_retry(&format!("math: unknown \\{name} read as {with}, retrying"));
                src = next;
            }
        }
    }
    None
}

/// Writes a retry of [`lenient`] to `tern.log` (typesetting is cached,
/// so each formula logs once).
fn log_retry(line: &str) {
    #[cfg(not(test))]
    crate::crashlog::log_line(line);
    #[cfg(test)]
    let _ = line;
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

/// The two answers ministral-8b wrote with formulas inside ```latex
/// fences, which showed as code before (#74; tests and the UI tour).
pub const FENCED_SAMPLE: &str = "Here is the attention formula in LaTeX:\n\n**Inline:**\n\n```latex\nThis is the attention score: \\( \\text{Attention}(q, k, v) = \\frac{\\exp(\\frac{q \\cdot k^T}{\\sqrt{d_k}})}{\\sum_{j=1}^D \\exp(\\frac{q \\cdot k_j^T}{\\sqrt{d_k}})} \\cdot v \\)\n```\n\n**Display Equation:**\n\n```latex\n\\[\n\\text{Attention}(q, k, v) = \\softmax_{\\text{scale and shift}} \\left( \\frac{q K^T}{\\sqrt{d_k}} \\right) V\n\\]\nWhere:\n\\[\n\\softmax(x)_i = \\frac{\\exp(x_i)}{\\sum_{j=1}^D \\exp(x_j)}\n\\]\n```\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn ratex(s: &str) -> Result<(), String> {
        ratex_parser::parser::parse(s)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn latex_fences_with_formulas_become_math() {
        let out = delimiters(FENCED_SAMPLE);
        assert!(!out.contains("```"), "{out}");
        // Inline: the prose line keeps its formula inline.
        assert!(
            out.contains("This is the attention score: $$\\text{Attention}(q, k, v) = "),
            "{out}"
        );
        // Display: two math blocks with "Where:" between them.
        assert_eq!(out.lines().filter(|l| *l == "$$").count(), 4, "{out}");
        assert!(
            out.contains("$$\n\\text{Attention}(q, k, v) = \\softmax_{\\text{scale and shift}} \\left( \\frac{q K^T}{\\sqrt{d_k}} \\right) V\n$$\nWhere:\n$$\n\\softmax(x)_i = \\frac{\\exp(x_i)}{\\sum_{j=1}^D \\exp(x_j)}\n$$\n"),
            "{out}"
        );
        assert!(out.contains("**Display Equation:**"), "{out}");
    }

    #[test]
    fn both_fenced_samples_parse_with_operators_defined() {
        let inline = "\\text{Attention}(q, k, v) = \\frac{\\exp(\\frac{q \\cdot k^T}{\\sqrt{d_k}})}{\\sum_{j=1}^D \\exp(\\frac{q \\cdot k_j^T}{\\sqrt{d_k}})} \\cdot v";
        assert!(lenient(inline, ratex).is_some());
        let d1 = "\\text{Attention}(q, k, v) = \\softmax_{\\text{scale and shift}} \\left( \\frac{q K^T}{\\sqrt{d_k}} \\right) V";
        assert!(ratex(d1).is_err(), "\\softmax is not a RaTeX command");
        assert!(lenient(d1, ratex).is_some());
        let d2 = "\\softmax(x)_i = \\frac{\\exp(x_i)}{\\sum_{j=1}^D \\exp(x_j)}";
        assert!(lenient(d2, ratex).is_some());
        // Anything else unknown is an operator too.
        assert!(lenient("\\Foo(x) + \\ReLU(y) + \\E[x]", ratex).is_some());
        assert_eq!(
            replace_command("\\softmax(x) \\softmaxed", "softmax", "S"),
            "S(x) \\softmaxed"
        );
        assert_eq!(
            undefined_macro("ParseError at position 3: Undefined control sequence: \\softmax"),
            Some("softmax")
        );
    }

    #[test]
    fn latex_fences_that_are_code_stay_code() {
        let doc = "```latex\n\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n$x$\n\\end{document}\n```\n";
        assert_eq!(delimiters(doc), doc);
        let rust = "```rust\nlet x = \"\\\\(\";\n```\n";
        assert_eq!(delimiters(rust), rust);
        // Prose with commands but no formula delimiters is code too.
        let prose = "```tex\nUse the \\emph command to stress a word in the text\n```\n";
        assert_eq!(delimiters(prose), prose);
    }

    #[test]
    fn a_bare_formula_fence_is_a_display_block() {
        assert_eq!(
            delimiters("See\n\n```math\nE = mc^2\n```\nnext\n"),
            "See\n\n$$\nE = mc^2\n$$\n\nnext\n"
        );
        assert_eq!(
            delimiters("```latex\n\\begin{align*}\na &= b \\\\\nc &= d\n\\end{align*}\n```\n"),
            "$$\n\\begin{align*}\na &= b \\\\\nc &= d\n\\end{align*}\n$$\n"
        );
        // Streaming, no fence close yet: still code until it closes.
        let open = "```latex\nE = mc^2\n";
        assert_eq!(delimiters(open), open);
    }

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
