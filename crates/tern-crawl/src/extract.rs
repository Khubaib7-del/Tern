//! HTML to readable text, plus the page's title and icon.

use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

/// What the reader keeps from an HTML page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extracted {
    pub title: Option<String>,
    pub description: Option<String>,
    /// The page's declared icon, resolved against its URL.
    pub icon: Option<String>,
    pub text: String,
    /// The same text as Markdown, with links kept as `[text](url)` and
    /// heading levels kept, for the built-in browser.
    pub markdown: String,
    /// Every link in the main text, absolute, in page order, deduplicated.
    pub links: Vec<Anchor>,
}

/// A link in the page's main text.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    pub text: String,
    pub url: String,
}

struct Walk<'a> {
    base: &'a Url,
    md: bool,
    links: Vec<Anchor>,
}

const SKIP: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "nav", "header",
    "footer", "aside", "form", "button", "select", "input", "textarea", "dialog", "menu",
];
const BLOCK: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "li",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "table",
    "tr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "figure",
    "figcaption",
    "br",
    "hr",
    "td",
    "th",
];

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn first_attr(doc: &Html, selector: &str, attr: &str) -> Option<String> {
    doc.select(&sel(selector))
        .find_map(|e| e.value().attr(attr))
        .map(collapse)
        .filter(|s| !s.is_empty())
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn extract_html(html: &str, base: &Url) -> Extracted {
    let doc = Html::parse_document(html);
    let title = first_attr(&doc, r#"meta[property="og:title"]"#, "content").or_else(|| {
        doc.select(&sel("title"))
            .next()
            .map(|t| collapse(&t.text().collect::<String>()))
            .filter(|s| !s.is_empty())
    });
    let description = first_attr(&doc, r#"meta[name="description"]"#, "content")
        .or_else(|| first_attr(&doc, r#"meta[property="og:description"]"#, "content"));
    let icon = doc
        .select(&sel("link[rel][href]"))
        .find(|e| {
            e.value()
                .attr("rel")
                .is_some_and(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("icon")))
        })
        .and_then(|e| e.value().attr("href"))
        .and_then(|h| base.join(h).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.to_string());

    let root = main_content(&doc);
    let mut out = String::new();
    let mut plain = Walk {
        base,
        md: false,
        links: Vec::new(),
    };
    walk(root, &mut out, &mut plain);
    let mut md_out = String::new();
    let mut md = Walk {
        base,
        md: true,
        links: Vec::new(),
    };
    walk(root, &mut md_out, &mut md);
    let mut links: Vec<Anchor> = Vec::new();
    for a in plain.links {
        if !links.iter().any(|l| l.url == a.url) {
            links.push(a);
        }
    }
    Extracted {
        title,
        description,
        icon,
        text: tidy(&out),
        markdown: tidy(&md_out),
        links,
    }
}

/// Tags never dropped for their class name (the text itself).
const BLOCK_KEEP: &[&str] = &[
    "p", "pre", "code", "h1", "h2", "h3", "h4", "h5", "h6", "table",
];

const POSITIVE: &[&str] = &[
    "article", "content", "post", "entry", "main", "body", "text", "story", "paper", "abstract",
    "prose", "markdown",
];
const NEGATIVE: &[&str] = &[
    "comment",
    "sidebar",
    "footer",
    "nav",
    "menu",
    "advert",
    "ad-",
    "promo",
    "related",
    "share",
    "social",
    "cookie",
    "banner",
    "newsletter",
    "subscribe",
    "breadcrumb",
    "popup",
    "modal",
    "widget",
    "sponsor",
    "masthead",
    "toolbar",
];

/// Readability's hint from an element's class and id: +25 for content
/// words, -25 for page furniture.
fn class_weight(e: &scraper::node::Element) -> i32 {
    let hints = format!(
        "{} {}",
        e.attr("class").unwrap_or_default(),
        e.attr("id").unwrap_or_default()
    )
    .to_ascii_lowercase();
    if hints.trim().is_empty() {
        return 0;
    }
    let mut w = 0;
    if NEGATIVE.iter().any(|n| hints.contains(n)) {
        w -= 25;
    }
    if POSITIVE.iter().any(|n| hints.contains(n)) {
        w += 25;
    }
    w
}

fn text_len(el: ElementRef<'_>) -> usize {
    el.text().map(|t| t.trim().len()).sum()
}

fn link_density(el: ElementRef<'_>) -> f32 {
    let all = text_len(el).max(1) as f32;
    let links: usize = el.select(&sel("a")).map(text_len).sum();
    (links as f32 / all).min(1.0)
}

/// The element holding the page's main text, found the way Readability
/// does: each paragraph scores its parent fully and its grandparent by
/// half; scores are cut by link density and nudged by class names. The
/// winner is widened until it holds most of the paragraph text, so an
/// article split across sibling blocks is not cut short.
pub fn main_content(doc: &Html) -> ElementRef<'_> {
    use std::collections::HashMap;
    let fallback = || {
        ["article", "main", r#"[role="main"]"#, "body"]
            .iter()
            .find_map(|s| doc.select(&sel(s)).next())
            .unwrap_or_else(|| doc.root_element())
    };
    let mut scores: HashMap<ego_tree::NodeId, f32> = HashMap::new();
    let mut total = 0usize;
    for p in doc.select(&sel("p, pre, blockquote, li")) {
        let len = text_len(p);
        if len < 25 {
            continue;
        }
        total += len;
        let score = 1.0
            + p.text().map(|t| t.matches(',').count()).sum::<usize>() as f32
            + (len as f32 / 100.0).min(3.0);
        let mut node = p.parent();
        for share in [1.0, 0.5] {
            let Some(n) = node else { break };
            if ElementRef::wrap(n).is_some() {
                *scores.entry(n.id()).or_default() += score * share;
            }
            node = n.parent();
        }
    }
    let best = scores
        .iter()
        .filter_map(|(id, s)| {
            let el = ElementRef::wrap(doc.tree.get(*id)?)?;
            if matches!(el.value().name(), "html" | "body") {
                return None;
            }
            let tag = match el.value().name() {
                "article" | "main" => 10.0,
                "div" | "section" => 5.0,
                "ul" | "ol" | "form" | "aside" | "nav" | "footer" => -5.0,
                _ => 0.0,
            };
            let s = (s + tag + class_weight(el.value()) as f32) * (1.0 - link_density(el));
            Some((el, s))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let Some((mut el, score)) = best else {
        return fallback();
    };
    if score < 10.0 || total == 0 {
        return fallback();
    }
    let held = |e: ElementRef<'_>| -> usize {
        e.select(&sel("p, pre, blockquote, li"))
            .map(text_len)
            .filter(|l| *l >= 25)
            .sum()
    };
    for _ in 0..4 {
        if held(el) * 10 >= total * 6 {
            break;
        }
        match el.parent().and_then(ElementRef::wrap) {
            Some(up) if up.value().name() != "html" => el = up,
            _ => break,
        }
    }
    el
}

fn walk(el: ElementRef<'_>, out: &mut String, w: &mut Walk<'_>) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let t: &str = t;
                if t.trim().is_empty() {
                    if !out.ends_with([' ', '\n']) && !out.is_empty() {
                        out.push(' ');
                    }
                    continue;
                }
                if t.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n']) {
                    out.push(' ');
                }
                out.push_str(&collapse(t));
                if t.ends_with(char::is_whitespace) {
                    out.push(' ');
                }
            }
            Node::Element(e) => {
                let name = e.name();
                if SKIP.contains(&name)
                    || e.attr("hidden").is_some()
                    || e.attr("aria-hidden") == Some("true")
                    || (!BLOCK_KEEP.contains(&name) && class_weight(e) <= -25)
                {
                    continue;
                }
                let Some(child_el) = ElementRef::wrap(child) else {
                    continue;
                };
                let block = BLOCK.contains(&name);
                if block {
                    out.push_str("\n\n");
                }
                if name == "li" {
                    out.push_str("- ");
                }
                if w.md && name.len() == 2 && name.starts_with('h') {
                    if let Some(level) = name[1..]
                        .parse::<usize>()
                        .ok()
                        .filter(|l| (1..=6).contains(l))
                    {
                        out.push_str(&"#".repeat(level));
                        out.push(' ');
                    }
                } else if matches!(name, "h1" | "h2" | "h3") {
                    out.push_str("# ");
                }
                let href = (name == "a")
                    .then(|| e.attr("href"))
                    .flatten()
                    .and_then(|h| w.base.join(h.trim()).ok())
                    .filter(|u| matches!(u.scheme(), "http" | "https"));
                let start = out.len();
                walk(child_el, out, w);
                if let Some(mut url) = href {
                    let inner = collapse(&out[start..]);
                    let same_page = url.fragment().is_some() && {
                        let mut a = url.clone();
                        a.set_fragment(None);
                        let mut b = w.base.clone();
                        b.set_fragment(None);
                        a == b
                    };
                    if !inner.is_empty() && !same_page {
                        if w.md {
                            let lead = if out[start..].starts_with(' ') {
                                " "
                            } else {
                                ""
                            };
                            let trail = if out.ends_with(' ') { " " } else { "" };
                            out.truncate(start);
                            out.push_str(&format!(
                                "{lead}[{}]({}){trail}",
                                inner.replace('[', "(").replace(']', ")"),
                                url.as_str().replace(' ', "%20").replace(')', "%29")
                            ));
                        }
                        url.set_fragment(None);
                        w.links.push(Anchor {
                            text: inner,
                            url: url.to_string(),
                        });
                    }
                }
                if block {
                    out.push_str("\n\n");
                }
            }
            _ => {}
        }
    }
}

/// Trims each line and keeps at most one blank line between paragraphs.
pub fn tidy(s: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod readability_tests {
    use super::*;

    #[test]
    fn finds_the_article_among_furniture() {
        let para = "Retrieval augmented generation pairs a retriever with a generator, so answers cite what they used, and errors can be traced to a source.";
        let html = format!(
            r#"<html><body>
            <div class="sidebar"><p>Popular: {para}</p><ul><li><a href="/a">One link that is long enough to count</a></li></ul></div>
            <div class="cookie-banner">We use cookies to make this site better for everyone who visits it.</div>
            <div id="story"><h2>How RAG works</h2><p>{para}</p><p>{para}</p><p>{para}</p></div>
            <div class="share-tools">Share this on every network you can think of today.</div>
            <div class="comments"><p>Great post, thanks for writing it up so clearly.</p></div>
            </body></html>"#
        );
        let base = Url::parse("https://example.org/rag").unwrap();
        let e = extract_html(&html, &base);
        assert!(e.text.contains("How RAG works"), "{}", e.text);
        assert_eq!(
            e.text.matches("Retrieval augmented").count(),
            3,
            "{}",
            e.text
        );
        for furniture in ["Popular", "cookies", "Share this", "Great post"] {
            assert!(!e.text.contains(furniture), "kept {furniture}: {}", e.text);
        }
    }

    #[test]
    fn an_article_split_across_blocks_stays_whole() {
        let para = "Each block holds part of the article, and the reader must keep every one of them, not only the first.";
        let html = format!(
            "<html><body><div class='wrap'><div><p>{para} One.</p><p>{para}</p></div><div><p>{para} Two.</p><p>{para}</p></div><div><p>{para} Three.</p></div></div></body></html>"
        );
        let base = Url::parse("https://example.org/").unwrap();
        let e = extract_html(&html, &base);
        for part in ["One.", "Two.", "Three."] {
            assert!(e.text.contains(part), "lost {part}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head>
        <title> Attention Is All You Need </title>
        <meta name="description" content="The Transformer paper.">
        <link rel="shortcut icon" href="/static/icon.png">
        <script>var tracking = 1;</script><style>p{}</style>
      </head><body>
        <nav>Home | About</nav>
        <header>Site header</header>
        <main>
          <h1>Attention   Is All You Need</h1>
          <p>The dominant sequence models are based on <b>recurrent</b> networks.</p>
          <ul><li>Self-attention</li><li>No recurrence</li></ul>
          <div hidden>secret</div>
        </main>
        <footer>Copyright</footer>
      </body></html>"#;

    #[test]
    fn keeps_the_main_text_and_drops_chrome() {
        let base = Url::parse("https://arxiv.org/abs/1706.03762").unwrap();
        let e = extract_html(PAGE, &base);
        assert_eq!(e.title.as_deref(), Some("Attention Is All You Need"));
        assert_eq!(e.description.as_deref(), Some("The Transformer paper."));
        assert_eq!(e.icon.as_deref(), Some("https://arxiv.org/static/icon.png"));
        assert_eq!(
            e.text,
            "# Attention Is All You Need\n\nThe dominant sequence models are based on recurrent networks.\n\n- Self-attention\n\n- No recurrence"
        );
        for gone in ["tracking", "Home", "Site header", "Copyright", "secret"] {
            assert!(!e.text.contains(gone), "{gone} leaked");
        }
    }

    #[test]
    fn falls_back_to_body_and_handles_fragments() {
        let base = Url::parse("https://example.org/").unwrap();
        let e = extract_html("<p>one</p><p>two <i>three</i></p>", &base);
        assert_eq!(e.text, "one\n\ntwo three");
        assert_eq!(e.title, None);
        assert_eq!(e.icon, None);
    }

    #[test]
    fn keeps_links_and_heading_levels_in_markdown() {
        let base = Url::parse("https://example.org/docs/a").unwrap();
        let html = r##"<main><h2>Intro</h2>
            <p>Read <a href="/paper.html">the paper</a> and <a href="b#x">part [2]</a>.</p>
            <p><a href="#top">Top</a> <a href="mailto:x@y.z">mail</a> <a href="/paper.html">again</a></p></main>"##;
        let e = extract_html(html, &base);
        assert_eq!(
            e.text,
            "# Intro\n\nRead the paper and part [2].\n\nTop mail again"
        );
        assert_eq!(
            e.markdown,
            "## Intro\n\nRead [the paper](https://example.org/paper.html) and [part (2)](https://example.org/docs/b#x).\n\nTop mail [again](https://example.org/paper.html)"
        );
        let urls: Vec<_> = e.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.org/paper.html",
                "https://example.org/docs/b"
            ]
        );
    }
}
