//! The research flow's GPUI-free parts (v2 s06–s11, s15): the safe points
//! a run passed (the checkpoint strip), claim review (keep, reject or
//! comment on each claim), and the report's export as Markdown or as a
//! printable HTML page. The window draws them in `view::flow_view`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use tern_agent::Run;

/// One safe point along a run: where it saved a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafePoint {
    /// "Plan approved", "Worker 2 finished", "After Read example.com".
    pub label: String,
    /// Time since the run started.
    pub at: Duration,
    /// Pages read by then.
    pub pages: usize,
    /// Tokens in and out by then.
    pub tokens: u64,
}

/// The run's safe points, in order. They mirror where the agent saves its
/// checkpoint (`tern_agent::checkpoint`): a deep run after the plan is
/// approved and after each worker finishes; a quick turn after each tool
/// result. Only the latest one can be resumed from; the earlier ones show
/// how the run got there.
pub fn safe_points(run: &Run) -> Vec<SafePoint> {
    let deep = run.deep.is_some();
    let mut out = Vec::new();
    let mut pages = 0usize;
    let mut tokens = 0u64;
    for s in &run.steps {
        if s.page.is_some() {
            pages += 1;
        }
        if let Some((i, o)) = s.tokens {
            tokens += i + o;
        }
        let label = match s.event.as_str() {
            "plan.answered" if deep => {
                let answer = s.payload["answer"].as_str().unwrap_or_default();
                if answer == "cancel" {
                    continue;
                }
                "Plan approved".to_owned()
            }
            "worker.finished" if deep => match s.payload["worker"].as_u64() {
                Some(n) => format!("Worker {n} finished"),
                None => "A worker finished".to_owned(),
            },
            "tool.executed" if !deep => {
                let what = clip(&s.target, 48);
                if what.is_empty() {
                    format!("After {}", s.verb.to_lowercase())
                } else {
                    format!("After {} {what}", s.verb.to_lowercase())
                }
            }
            _ => continue,
        };
        out.push(SafePoint {
            label,
            at: s.at,
            pages,
            tokens,
        });
    }
    out
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut c: String = s.chars().take(n.saturating_sub(1)).collect();
        c.push('…');
        c
    }
}

/// "1m 12s", "42s".
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// The user's review of one run's claims.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaimReview {
    /// Reviewing claim by claim (keep, reject, comment).
    pub reviewing: bool,
    /// Claims the user rejected, by number.
    pub rejected: BTreeSet<usize>,
    /// The user's note on a claim, for Tern's next pass.
    pub notes: BTreeMap<usize, String>,
    /// The review was finished ("Done reviewing" or "Keep all").
    pub done: bool,
}

impl ClaimReview {
    pub fn toggle_reject(&mut self, claim: usize) {
        if !self.rejected.remove(&claim) {
            self.rejected.insert(claim);
        }
    }

    pub fn set_note(&mut self, claim: usize, note: &str) {
        let note = note.trim();
        if note.is_empty() {
            self.notes.remove(&claim);
        } else {
            self.notes.insert(claim, note.to_owned());
        }
    }

    /// "Report updated · 1 claim removed · 1 note sent to the composer".
    pub fn summary(&self) -> String {
        let mut parts = vec!["Report updated".to_owned()];
        if !self.rejected.is_empty() {
            parts.push(count(
                self.rejected.len(),
                "claim removed",
                "claims removed",
            ));
        }
        if !self.notes.is_empty() {
            parts.push(count(
                self.notes.len(),
                "note in the composer",
                "notes in the composer",
            ));
        }
        if parts.len() == 1 {
            parts.push("every claim kept".to_owned());
        }
        parts.join(" · ")
    }

    /// The message that takes the notes back to Tern for the next pass,
    /// or `None` when there are no notes.
    pub fn follow_up(&self) -> Option<String> {
        if self.notes.is_empty() {
            return None;
        }
        let mut s = String::from("Revise the last report with my review notes.\n");
        for (claim, note) in &self.notes {
            s.push_str(&format!("- Claim {claim}: {note}\n"));
        }
        if !self.rejected.is_empty() {
            let list: Vec<String> = self.rejected.iter().map(|c| c.to_string()).collect();
            s.push_str(&format!(
                "Leave out {} {} (rejected).\n",
                if list.len() == 1 { "claim" } else { "claims" },
                list.join(", ")
            ));
        }
        Some(s.trim_end().to_owned())
    }

    /// The review as JSON, saved beside the report.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rejected": self.rejected.iter().collect::<Vec<_>>(),
            "notes": self.notes.iter().map(|(c, n)| serde_json::json!({"claim": c, "note": n})).collect::<Vec<_>>(),
            "done": self.done,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Self {
        let rejected = v["rejected"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64())
                    .map(|x| x as usize)
                    .collect()
            })
            .unwrap_or_default();
        let notes = v["notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| {
                        Some((
                            n["claim"].as_u64()? as usize,
                            n["note"].as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            reviewing: false,
            rejected,
            notes,
            done: v["done"] == true,
        }
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Where a run's review is saved: beside its Markdown report.
pub fn review_path(markdown: &std::path::Path) -> std::path::PathBuf {
    markdown.with_extension("review.json")
}

/// The report as exported Markdown: the saved report, then the review
/// (rejected claims and notes) when there is one.
pub fn export_markdown(report: &str, run: &Run, review: Option<&ClaimReview>) -> String {
    let mut out = report.trim_end().to_owned();
    out.push('\n');
    let Some(r) = review.filter(|r| !r.rejected.is_empty() || !r.notes.is_empty()) else {
        return out;
    };
    out.push_str("\n## Review\n\n");
    for c in &run.checks {
        let rejected = r.rejected.contains(&c.claim);
        let note = r.notes.get(&c.claim);
        if !rejected && note.is_none() {
            continue;
        }
        let mut line = format!("- Claim {}", c.claim);
        if rejected {
            line.push_str(" (rejected)");
        }
        line.push_str(&format!(": {}", c.text.trim()));
        if let Some(n) = note {
            line.push_str(&format!(" Note: {n}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A printable HTML page of `markdown`: headings, paragraphs, lists,
/// quotes, code, pipe tables, links, bold and italic. Open it in a
/// browser and print to PDF. Everything is escaped; nothing loads from
/// the network.
pub fn to_html(title: &str, markdown: &str) -> String {
    let mut body = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, body: &mut String| {
        if !para.is_empty() {
            body.push_str(&format!("<p>{}</p>\n", inline(&para.join(" "))));
            para.clear();
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.starts_with("```") {
            flush(&mut para, &mut body);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(escape(lines[i]));
                i += 1;
            }
            body.push_str(&format!("<pre><code>{}</code></pre>\n", code.join("\n")));
            i += 1;
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut body);
            i += 1;
            continue;
        }
        if let Some(level) = heading_level(t) {
            flush(&mut para, &mut body);
            let text = t[level..].trim();
            body.push_str(&format!("<h{level}>{}</h{level}>\n", inline(text)));
            i += 1;
            continue;
        }
        if t.starts_with('|') && i + 1 < lines.len() && is_rule_row(lines[i + 1]) {
            flush(&mut para, &mut body);
            body.push_str("<table>\n<thead><tr>");
            for c in cells(t) {
                body.push_str(&format!("<th>{}</th>", inline(&c)));
            }
            body.push_str("</tr></thead>\n<tbody>\n");
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                body.push_str("<tr>");
                for c in cells(lines[i].trim()) {
                    body.push_str(&format!("<td>{}</td>", inline(&c)));
                }
                body.push_str("</tr>\n");
                i += 1;
            }
            body.push_str("</tbody>\n</table>\n");
            continue;
        }
        if let Some(item) = list_item(t) {
            flush(&mut para, &mut body);
            let ordered = item.0;
            let tag = if ordered { "ol" } else { "ul" };
            body.push_str(&format!("<{tag}>\n"));
            while i < lines.len() {
                match list_item(lines[i].trim()) {
                    Some((o, text)) if o == ordered => {
                        body.push_str(&format!("<li>{}</li>\n", inline(text)));
                        i += 1;
                    }
                    _ => break,
                }
            }
            body.push_str(&format!("</{tag}>\n"));
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            flush(&mut para, &mut body);
            let mut quote = vec![q.trim().to_owned()];
            i += 1;
            while i < lines.len() {
                match lines[i].trim().strip_prefix('>') {
                    Some(q) => quote.push(q.trim().to_owned()),
                    None => break,
                }
                i += 1;
            }
            body.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                inline(&quote.join(" "))
            ));
            continue;
        }
        para.push(t.to_owned());
        i += 1;
    }
    flush(&mut para, &mut body);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>\n{}\n</style>\n</head>\n<body>\n<main>\n{}</main>\n</body>\n</html>\n",
        escape(title),
        PRINT_CSS,
        body
    )
}

/// Plain, printable, no web fonts.
const PRINT_CSS: &str = "body{margin:0;background:#fff;color:#111;font:15px/1.6 -apple-system,'Segoe UI',system-ui,sans-serif}\nmain{max-width:760px;margin:40px auto;padding:0 24px}\nh1,h2,h3{line-height:1.25;margin:1.4em 0 .5em}\nh1{font-size:1.6em}h2{font-size:1.25em}h3{font-size:1.05em}\na{color:#111}\nblockquote{margin:1em 0;padding-left:12px;border-left:2px solid #ccc;color:#444}\ncode,pre{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:.9em}\npre{background:#f5f5f5;padding:10px;overflow:auto}\ntable{border-collapse:collapse;width:100%;margin:1em 0;font-size:.92em}\nth,td{border:1px solid #ddd;padding:5px 8px;text-align:left;vertical-align:top}\n@media print{main{margin:0;max-width:none}a{text-decoration:none}}";

fn heading_level(t: &str) -> Option<usize> {
    let n = t.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&n)
        .then_some(n)
        .filter(|&n| t[n..].starts_with(' '))
}

fn is_rule_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(row: &str) -> Vec<String> {
    let t = row.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_owned()).collect()
}

/// `(ordered, text)` for a list line.
fn list_item(t: &str) -> Option<(bool, &str)> {
    for b in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(b) {
            return Some((false, rest));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &t[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, rest));
        }
    }
    None
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Inline Markdown: `code`, [text](url), **bold**, *italic*. Only http(s),
/// file and anchor links become links.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(end) = find(&chars, i + 1, "`")
        {
            let code: String = chars[i + 1..end].iter().collect();
            out.push_str(&format!("<code>{}</code>", escape(&code)));
            i = end + 1;
            continue;
        }
        if c == '['
            && let Some(close) = find(&chars, i + 1, "](")
            && let Some(end) = find(&chars, close + 2, ")")
        {
            let text: String = chars[i + 1..close].iter().collect();
            let url: String = chars[close + 2..end].iter().collect();
            let safe = ["http://", "https://", "file://", "#"]
                .iter()
                .any(|p| url.starts_with(p));
            if safe {
                out.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    escape(&url),
                    inline(&text)
                ));
            } else {
                out.push_str(&inline(&text));
            }
            i = end + 1;
            continue;
        }
        if c == '*'
            && chars.get(i + 1) == Some(&'*')
            && let Some(end) = find(&chars, i + 2, "**")
        {
            let text: String = chars[i + 2..end].iter().collect();
            out.push_str(&format!("<strong>{}</strong>", inline(&text)));
            i = end + 2;
            continue;
        }
        if c == '*'
            && chars.get(i + 1).is_some_and(|n| !n.is_whitespace())
            && let Some(end) = find(&chars, i + 1, "*")
        {
            let text: String = chars[i + 1..end].iter().collect();
            out.push_str(&format!("<em>{}</em>", inline(&text)));
            i = end + 1;
            continue;
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out
}

/// Where `pat` next starts in `chars` at or after `from`.
fn find(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    if p.is_empty() || from + p.len() > chars.len() {
        return None;
    }
    (from..=chars.len().saturating_sub(p.len())).find(|&k| chars[k..k + p.len()] == p[..])
}

/// The export file beside the report: `turn-3.md` → `turn-3.export.md` or
/// `turn-3.html`.
pub fn export_path(markdown: &std::path::Path, html: bool) -> std::path::PathBuf {
    if html {
        markdown.with_extension("html")
    } else {
        markdown.with_extension("export.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_covers_the_report_shapes_and_escapes() {
        let md = "# Title\n\nA **bold** and *soft* line with `x<y` and [a link](https://example.com).\n\n- one\n- two\n\n1. first\n2. second\n\n> quoted\n\n| A | B |\n|---|---|\n| 1 | <2> |\n\n```\nfn main() {}\n```\n";
        let h = to_html("R & D", md);
        assert!(h.contains("<title>R &amp; D</title>"));
        assert!(h.contains("<h1>Title</h1>"));
        assert!(h.contains("<strong>bold</strong>"));
        assert!(h.contains("<em>soft</em>"));
        assert!(h.contains("<code>x&lt;y</code>"));
        assert!(h.contains("<a href=\"https://example.com\">a link</a>"));
        assert!(h.contains("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"));
        assert!(h.contains("<ol>\n<li>first</li>"));
        assert!(h.contains("<blockquote>quoted</blockquote>"));
        assert!(h.contains("<th>A</th>") && h.contains("<td>&lt;2&gt;</td>"));
        assert!(h.contains("<pre><code>fn main() {}</code></pre>"));
    }

    #[test]
    fn unsafe_links_lose_their_href() {
        let h = to_html("t", "[x](javascript:alert(1))");
        assert!(!h.contains("href=\"javascript"));
    }

    #[test]
    fn a_review_counts_and_round_trips() {
        let mut r = ClaimReview::default();
        assert_eq!(r.summary(), "Report updated · every claim kept");
        assert!(r.follow_up().is_none());
        r.toggle_reject(3);
        r.set_note(4, "  Check against the appendix ");
        assert_eq!(
            r.summary(),
            "Report updated · 1 claim removed · 1 note in the composer"
        );
        let f = r.follow_up().unwrap();
        assert!(f.contains("Claim 4: Check against the appendix"));
        assert!(f.contains("Leave out claim 3 (rejected)"));
        r.done = true;
        let back = ClaimReview::from_json(&r.to_json());
        assert_eq!(back.rejected, r.rejected);
        assert_eq!(back.notes, r.notes);
        assert!(back.done);
        r.toggle_reject(3);
        r.set_note(4, " ");
        assert!(r.rejected.is_empty() && r.notes.is_empty());
    }

    #[test]
    fn export_paths_sit_beside_the_report() {
        let md = std::path::Path::new("/r/turn-3.md");
        assert_eq!(
            export_path(md, true),
            std::path::Path::new("/r/turn-3.html")
        );
        assert_eq!(
            export_path(md, false),
            std::path::Path::new("/r/turn-3.export.md")
        );
        assert_eq!(
            review_path(md),
            std::path::Path::new("/r/turn-3.review.json")
        );
    }

    #[test]
    fn clock_reads_minutes_and_seconds() {
        assert_eq!(clock(Duration::from_secs(42)), "42s");
        assert_eq!(clock(Duration::from_secs(72)), "1m 12s");
    }

    #[test]
    fn an_empty_run_has_no_safe_points() {
        assert!(safe_points(&Run::new()).is_empty());
    }
}
