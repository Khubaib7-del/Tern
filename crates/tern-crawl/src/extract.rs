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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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

    let root = ["article", "main", r#"[role="main"]"#, "body"]
        .iter()
        .find_map(|s| doc.select(&sel(s)).next())
        .unwrap_or_else(|| doc.root_element());
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
