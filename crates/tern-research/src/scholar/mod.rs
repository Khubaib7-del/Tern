//! Scholarly search, without the network: the papers the six scholarly
//! services return, read into one shape ([`Paper`]), merged across
//! services, ranked by the rerank funnel ([`funnel`]), with an honest
//! "not enough evidence" when nothing found supports the question; PDF
//! text cut into passages that carry a page and a section ([`pdf`]); and
//! citation snowballing with a stop rule ([`snowball`]).
//!
//! The HTTP lives in `tern-crawl` (`tern_crawl::scholar`); the tools that
//! the turn loop calls live in `tern-agent`. The parsers ([`parse`]) are
//! here so the offline evaluation can replay recorded answers through the
//! same code a live search uses.

use serde::{Deserialize, Serialize};

pub mod funnel;
pub mod parse;
pub mod pdf;
pub mod snowball;

/// A scholarly service Tern can ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    SemanticScholar,
    OpenAlex,
    Arxiv,
    Crossref,
    Unpaywall,
    Core,
}

/// What a service needs from the user, if anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyNeed {
    /// Free and keyless.
    None,
    /// Works without one; a key lifts the shared rate limit.
    OptionalKey,
    /// Works without one; an email puts Tern in the polite pool.
    OptionalEmail,
    /// Asks for an email on every request; Tern sends a project address
    /// until the user gives their own.
    Email,
    /// Needs a free key.
    Key,
}

impl Service {
    pub const ALL: [Service; 6] = [
        Self::SemanticScholar,
        Self::OpenAlex,
        Self::Arxiv,
        Self::Crossref,
        Self::Unpaywall,
        Self::Core,
    ];

    /// The id used in settings, the keychain (`scholar.<id>`), and the
    /// built-in logo's name.
    pub fn id(self) -> &'static str {
        match self {
            Self::SemanticScholar => "semanticscholar",
            Self::OpenAlex => "openalex",
            Self::Arxiv => "arxiv",
            Self::Crossref => "crossref",
            Self::Unpaywall => "unpaywall",
            Self::Core => "core",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SemanticScholar => "Semantic Scholar",
            Self::OpenAlex => "OpenAlex",
            Self::Arxiv => "arXiv",
            Self::Crossref => "Crossref",
            Self::Unpaywall => "Unpaywall",
            Self::Core => "CORE",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        let id = match id.as_str() {
            "s2" | "semantic_scholar" | "semantic-scholar" => "semanticscholar",
            "open_alex" => "openalex",
            other => other,
        }
        .to_owned();
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    /// One line on what it gives, for its settings row.
    pub fn blurb(self) -> &'static str {
        match self {
            Self::SemanticScholar => {
                "200M+ papers with citation counts and the citation graph; full-text snippets with their section. Free; a key lifts the shared rate limit."
            }
            Self::OpenAlex => {
                "Open catalogue of 250M+ works: venues, citations, open-access links. Free; a key raises the daily allowance."
            }
            Self::Arxiv => {
                "Preprints in physics, maths, CS and more, with their PDFs. Free, one request every 3 s."
            }
            Self::Crossref => {
                "DOI registry: the publisher's own metadata and retraction notices. Free; your email joins the polite pool."
            }
            Self::Unpaywall => {
                "Finds a legal open-access PDF for a DOI. Free; asks for an email with each request."
            }
            Self::Core => "Open-access full texts from repositories worldwide. Needs a free key.",
        }
    }

    pub fn needs(self) -> KeyNeed {
        match self {
            Self::SemanticScholar | Self::OpenAlex => KeyNeed::OptionalKey,
            Self::Arxiv => KeyNeed::None,
            Self::Crossref => KeyNeed::OptionalEmail,
            Self::Unpaywall => KeyNeed::Email,
            Self::Core => KeyNeed::Key,
        }
    }

    /// Whether the saved value is an email address rather than a key.
    pub fn takes_email(self) -> bool {
        matches!(self.needs(), KeyNeed::Email | KeyNeed::OptionalEmail)
    }

    /// Whether the user can save something for it at all.
    pub fn takes_secret(self) -> bool {
        self.needs() != KeyNeed::None
    }

    /// Where to get a key, when it takes one.
    pub fn key_url(self) -> &'static str {
        match self {
            Self::SemanticScholar => "https://www.semanticscholar.org/product/api#api-key-form",
            Self::OpenAlex => "https://openalex.org/settings/api",
            Self::Core => "https://core.ac.uk/services/api#form",
            _ => "",
        }
    }

    /// Whether it answers keyword searches (Unpaywall only looks up DOIs).
    pub fn searches(self) -> bool {
        !matches!(self, Self::Unpaywall)
    }

    /// The least time between two requests, in milliseconds, from each
    /// service's published limits: Semantic Scholar 1 request per second,
    /// arXiv one every 3 s, CORE's free tier about one every 6 s, and a
    /// tenth of a second for the services that allow far more.
    pub fn gap_ms(self) -> u64 {
        match self {
            Self::SemanticScholar => 1_000,
            Self::Arxiv => 3_000,
            Self::Core => 6_000,
            Self::OpenAlex | Self::Crossref | Self::Unpaywall => 100,
        }
    }

    /// The API's address.
    pub fn default_base(self) -> &'static str {
        match self {
            Self::SemanticScholar => "https://api.semanticscholar.org",
            Self::OpenAlex => "https://api.openalex.org",
            Self::Arxiv => "https://export.arxiv.org",
            Self::Crossref => "https://api.crossref.org",
            Self::Unpaywall => "https://api.unpaywall.org",
            Self::Core => "https://api.core.ac.uk",
        }
    }

    /// The services a search asks when the user named none: the keyless
    /// ones, plus CORE when it has a key.
    pub fn default_search(core_key: bool) -> Vec<Service> {
        let mut out = vec![
            Self::SemanticScholar,
            Self::OpenAlex,
            Self::Arxiv,
            Self::Crossref,
        ];
        if core_key {
            out.push(Self::Core);
        }
        out
    }
}

/// One paper, as one or more services described it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub venue: Option<String>,
    /// Lower case, without `https://doi.org/`.
    #[serde(default)]
    pub doi: Option<String>,
    /// Without the version (`1706.03762`).
    #[serde(default)]
    pub arxiv: Option<String>,
    #[serde(default)]
    pub s2_id: Option<String>,
    #[serde(default)]
    pub openalex: Option<String>,
    #[serde(default)]
    pub core_id: Option<String>,
    #[serde(default)]
    pub citations: Option<u64>,
    /// Semantic Scholar's influential citation count: citations that
    /// build on the paper rather than mention it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub influential_citations: Option<u64>,
    /// As a reference or citation of a snowball seed: Semantic Scholar
    /// marks the link influential.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub influential: bool,
    #[serde(default, rename = "abstract")]
    pub abstract_text: Option<String>,
    /// A legal open-access PDF.
    #[serde(default)]
    pub oa_pdf: Option<String>,
    /// The paper's landing page as the first service gave it.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub found_by: Vec<Service>,
    /// A retraction notice was found for it.
    #[serde(default)]
    pub retracted: bool,
}

impl Paper {
    /// The key two descriptions of one paper share: its DOI, else its
    /// arXiv id, else its title reduced to letters and digits.
    pub fn key(&self) -> String {
        if let Some(d) = &self.doi {
            // arXiv's own DOIs name the preprint, so they meet its id.
            if let Some(id) = d.strip_prefix("10.48550/arxiv.") {
                return format!("arxiv:{}", strip_version(id));
            }
            return format!("doi:{d}");
        }
        if let Some(a) = &self.arxiv {
            return format!("arxiv:{}", a.to_ascii_lowercase());
        }
        format!("title:{}", normalize_title(&self.title))
    }

    /// Every key this paper answers to, so a paper with a DOI meets its
    /// arXiv preprint and both meet a title-only record.
    pub fn keys(&self) -> Vec<String> {
        let mut out = vec![self.key()];
        if let Some(a) = &self.arxiv {
            out.push(format!("arxiv:{}", a.to_ascii_lowercase()));
        }
        let t = normalize_title(&self.title);
        if t.split(' ').count() >= 4 {
            out.push(format!("title:{t}"));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Fills what this description lacks from `other`, keeps the larger
    /// citation count, and records both services.
    pub fn absorb(&mut self, other: Paper) {
        fn fill<T>(a: &mut Option<T>, b: Option<T>) {
            if a.is_none() {
                *a = b;
            }
        }
        if self.title.trim().is_empty() {
            self.title = other.title;
        }
        if self.authors.is_empty() {
            self.authors = other.authors;
        }
        fill(&mut self.year, other.year);
        fill(&mut self.venue, other.venue);
        fill(&mut self.doi, other.doi);
        fill(&mut self.arxiv, other.arxiv);
        fill(&mut self.s2_id, other.s2_id);
        fill(&mut self.openalex, other.openalex);
        fill(&mut self.core_id, other.core_id);
        let longer = match (&self.abstract_text, &other.abstract_text) {
            (Some(a), Some(b)) => b.len() > a.len(),
            (None, Some(_)) => true,
            _ => false,
        };
        if longer {
            self.abstract_text = other.abstract_text;
        }
        fill(&mut self.oa_pdf, other.oa_pdf);
        self.citations = match (self.citations, other.citations) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.influential_citations = match (self.influential_citations, other.influential_citations)
        {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.influential |= other.influential;
        if self.url.is_empty() {
            self.url = other.url;
        }
        for s in other.found_by {
            if !self.found_by.contains(&s) {
                self.found_by.push(s);
            }
        }
        self.retracted |= other.retracted;
    }

    /// The link a citation opens: the DOI, else the arXiv page, else the
    /// landing page a service gave.
    pub fn link(&self) -> String {
        if let Some(d) = &self.doi {
            return format!("https://doi.org/{d}");
        }
        if let Some(a) = &self.arxiv {
            return format!("https://arxiv.org/abs/{a}");
        }
        self.url.clone()
    }

    /// "Vaswani, Shazeer et al." style.
    pub fn authors_short(&self) -> String {
        let last = |n: &str| n.split_whitespace().next_back().unwrap_or(n).to_owned();
        match self.authors.as_slice() {
            [] => String::new(),
            [a] => last(a),
            [a, b] => format!("{} and {}", last(a), last(b)),
            [a, ..] => format!("{} et al.", last(a)),
        }
    }

    /// One line of metadata: "Vaswani et al. · 2017 · NeurIPS · 120,345
    /// citations · doi:10.x/y · open access".
    pub fn meta_line(&self) -> String {
        let mut parts = Vec::new();
        let a = self.authors_short();
        if !a.is_empty() {
            parts.push(a);
        }
        if let Some(y) = self.year {
            parts.push(y.to_string());
        }
        if let Some(v) = self.venue.as_ref().filter(|v| !v.trim().is_empty()) {
            parts.push(v.trim().to_owned());
        }
        if let Some(c) = self.citations {
            parts.push(format!(
                "{} citation{}",
                thousands(c),
                if c == 1 { "" } else { "s" }
            ));
        }
        if let Some(i) = self.influential_citations.filter(|i| *i > 0) {
            parts.push(format!("{} influential", thousands(i)));
        }
        if let Some(d) = &self.doi {
            parts.push(format!("doi:{d}"));
        } else if let Some(a) = &self.arxiv {
            parts.push(format!("arXiv:{a}"));
        }
        if self.oa_pdf.is_some() {
            parts.push("open-access PDF".into());
        }
        if self.retracted {
            parts.push("RETRACTED".into());
        }
        parts.join(" · ")
    }

    /// What the funnel reads: the title and the abstract.
    pub fn text(&self) -> String {
        match &self.abstract_text {
            Some(a) => format!("{}. {}", self.title, a),
            None => self.title.clone(),
        }
    }
}

/// Text quoted from a paper without the paper's own reference markers
/// ("[17]", "[9, 18]", "[3–5]"): they number the paper's bibliography,
/// not Tern's sources, and once copied into an answer the claim check
/// read them as citations of sources Tern never read (#83). Bracketed
/// text with any letter in it, and intervals such as "[0, 1]", stay.
pub fn strip_ref_markers(text: &str) -> String {
    fn is_marker(inner: &str) -> bool {
        let t = inner.trim();
        if t.is_empty() || t.len() > 40 {
            return false;
        }
        if !t
            .chars()
            .all(|c| c.is_ascii_digit() || " ,;-\u{2013}\u{2014}".contains(c))
        {
            return false;
        }
        let numbers: Vec<&str> = t
            .split(|c: char| !c.is_ascii_digit())
            .filter(|n| !n.is_empty())
            .collect();
        !numbers.is_empty()
            && numbers
                .iter()
                .all(|n| n.len() <= 3 && n.trim_start_matches('0') == *n)
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        if let Some(j) = after.find(']')
            && is_marker(&after[..j])
        {
            out.push_str(rest[..i].trim_end_matches(' '));
            rest = &after[j + 1..];
            continue;
        }
        out.push_str(&rest[..=i]);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `1234567` as `1,234,567`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A DOI in its plain form: lower case, without a resolver or `doi:`.
pub fn normalize_doi(s: &str) -> Option<String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let mut rest = lower.as_str();
    for p in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi.org/",
        "doi:",
    ] {
        if let Some(r) = rest.strip_prefix(p) {
            rest = r;
            break;
        }
    }
    let rest = rest.trim().trim_end_matches(['.', ',', ';']);
    (rest.starts_with("10.") && rest.contains('/')).then(|| rest.to_owned())
}

/// An arXiv id without its version (`1706.03762v7` → `1706.03762`).
pub fn strip_version(id: &str) -> String {
    let id = id.trim();
    match id.rfind('v') {
        Some(i)
            if i > 0
                && i + 1 < id.len()
                && id[i + 1..].chars().all(|c| c.is_ascii_digit())
                && id[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit()) =>
        {
            id[..i].to_owned()
        }
        _ => id.to_owned(),
    }
}

/// An arXiv id from an id, an `arXiv:` reference, or an arxiv.org link.
pub fn arxiv_id(s: &str) -> Option<String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let rest = if let Some(i) = lower.find("arxiv.org/abs/") {
        &t[i + "arxiv.org/abs/".len()..]
    } else if let Some(i) = lower.find("arxiv.org/pdf/") {
        &t[i + "arxiv.org/pdf/".len()..]
    } else if let Some(r) = lower.strip_prefix("arxiv:") {
        &t[t.len() - r.len()..]
    } else {
        t
    };
    let rest = rest.trim_end_matches(".pdf").trim_end_matches('/');
    let new_style = rest.len() >= 9
        && rest.chars().take(4).all(|c| c.is_ascii_digit())
        && rest.chars().nth(4) == Some('.')
        && rest[5..]
            .chars()
            .take_while(|c| *c != 'v')
            .all(|c| c.is_ascii_digit());
    let old_style = rest.contains('/')
        && rest
            .split('/')
            .next_back()
            .is_some_and(|n| n.len() >= 7 && n.chars().take(7).all(|c| c.is_ascii_digit()));
    (new_style || old_style).then(|| strip_version(rest))
}

/// A title reduced to lower-case words of letters and digits.
pub fn normalize_title(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merges descriptions of the same paper (by DOI, arXiv id, or title),
/// keeping the order in which papers were first seen.
pub fn merge_all(papers: Vec<Paper>) -> Vec<Paper> {
    let mut out: Vec<Paper> = Vec::new();
    for p in papers {
        if p.title.trim().is_empty() && p.doi.is_none() && p.arxiv.is_none() {
            continue;
        }
        let keys = p.keys();
        match out
            .iter()
            .position(|q| q.keys().iter().any(|k| keys.contains(k)))
        {
            Some(i) => out[i].absorb(p),
            None => out.push(p),
        }
    }
    out
}

/// Where a passage sits in its paper.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator {
    /// 1-based PDF page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// The section heading it falls under ("3 Method", "Abstract").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Character offsets in the service's text (Semantic Scholar
    /// snippets) or the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<(usize, usize)>,
}

impl Locator {
    /// "p. 4 · 3 Method", "§ Abstract", or empty.
    pub fn label(&self) -> String {
        match (&self.page, &self.section) {
            (Some(p), Some(s)) => format!("p. {p} · {s}"),
            (Some(p), None) => format!("p. {p}"),
            (None, Some(s)) => format!("§ {s}"),
            (None, None) => String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.page.is_none() && self.section.is_none()
    }
}

/// Where in a paper a passage was taken from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassageKind {
    Title,
    #[default]
    Abstract,
    /// The paper's body (a snippet or a PDF passage).
    Body,
}

/// A passage of a paper, the unit the funnel ranks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Passage {
    /// The paper's [`Paper::key`].
    pub paper: String,
    pub text: String,
    #[serde(default)]
    pub kind: PassageKind,
    #[serde(default)]
    pub locator: Locator,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(title: &str) -> Paper {
        Paper {
            title: title.into(),
            ..Paper::default()
        }
    }

    #[test]
    fn dois_and_arxiv_ids_are_normalised() {
        assert_eq!(
            normalize_doi("https://doi.org/10.1038/S41586-021-03819-2").as_deref(),
            Some("10.1038/s41586-021-03819-2")
        );
        assert_eq!(normalize_doi("doi:10.1/x.").as_deref(), Some("10.1/x"));
        assert_eq!(normalize_doi("not a doi"), None);
        assert_eq!(arxiv_id("1706.03762v7").as_deref(), Some("1706.03762"));
        assert_eq!(
            arxiv_id("https://arxiv.org/abs/2005.11401v4").as_deref(),
            Some("2005.11401")
        );
        assert_eq!(arxiv_id("arXiv:2310.11511").as_deref(), Some("2310.11511"));
        assert_eq!(
            arxiv_id("hep-th/9901001").as_deref(),
            Some("hep-th/9901001")
        );
        assert_eq!(arxiv_id("hello"), None);
        assert_eq!(strip_version("2101.00001v12"), "2101.00001");
    }

    #[test]
    fn the_same_paper_from_three_services_is_one_row() {
        let mut a = paper("Attention Is All You Need");
        a.doi = Some("10.48550/arxiv.1706.03762".into());
        a.found_by = vec![Service::Crossref];
        let mut b = paper("Attention is all you need");
        b.arxiv = Some("1706.03762".into());
        b.citations = Some(10);
        b.found_by = vec![Service::Arxiv];
        let mut c = paper("Attention is All you Need.");
        c.citations = Some(99);
        c.venue = Some("NeurIPS".into());
        c.found_by = vec![Service::SemanticScholar];
        let d = paper("Something else entirely about tides");
        let out = merge_all(vec![a, b, c, d]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].citations, Some(99));
        assert_eq!(out[0].venue.as_deref(), Some("NeurIPS"));
        assert_eq!(out[0].found_by.len(), 3);
        assert_eq!(out[0].link(), "https://doi.org/10.48550/arxiv.1706.03762");
    }

    #[test]
    fn a_papers_own_reference_markers_leave_quotes() {
        assert_eq!(
            strip_ref_markers(
                "models such as BERT [17, 18] and GPT [9]. Attention [3\u{2013}5] helps [12; 14]."
            ),
            "models such as BERT and GPT. Attention helps."
        );
        assert_eq!(strip_ref_markers("a [1][2] b"), "a b");
        // Not markers: locators, intervals, words.
        for keep in [
            "[p. 4 \u{b7} 3 Method] text",
            "values in [0, 1]",
            "see [Smith 2020]",
            "array[i]",
            "unclosed [12",
        ] {
            assert_eq!(strip_ref_markers(keep), keep);
        }
    }

    #[test]
    fn meta_line_and_locator_read_plainly() {
        let mut p = paper("X");
        p.authors = vec![
            "Ashish Vaswani".into(),
            "Noam Shazeer".into(),
            "N. P.".into(),
        ];
        p.year = Some(2017);
        p.venue = Some("NeurIPS".into());
        p.citations = Some(123_456);
        p.doi = Some("10.1/x".into());
        p.oa_pdf = Some("https://x/y.pdf".into());
        assert_eq!(
            p.meta_line(),
            "Vaswani et al. · 2017 · NeurIPS · 123,456 citations · doi:10.1/x · open-access PDF"
        );
        let l = Locator {
            page: Some(4),
            section: Some("3 Method".into()),
            offset: None,
        };
        assert_eq!(l.label(), "p. 4 · 3 Method");
        assert_eq!(Locator::default().label(), "");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn services_parse_by_id_and_alias() {
        for s in Service::ALL {
            assert_eq!(Service::parse(s.id()), Some(s));
        }
        assert_eq!(Service::parse("S2"), Some(Service::SemanticScholar));
        assert!(!Service::Unpaywall.searches());
        assert!(Service::Unpaywall.takes_email());
        assert!(!Service::Arxiv.takes_secret());
    }
}
