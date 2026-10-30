//! Each scholarly service's answer read into [`Paper`]s and [`Passage`]s.
//!
//! Pure functions over the JSON (or arXiv's Atom XML) a service returned,
//! so recorded answers replay through the same code offline. Missing
//! fields are left empty, never guessed.

use serde_json::Value;

use super::{
    Locator, Paper, Passage, PassageKind, Service, arxiv_id, normalize_doi, strip_version,
};

fn s(v: &Value) -> Option<String> {
    v.as_str()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
}

fn year_of(v: &Value) -> Option<i32> {
    v.as_i64()
        .map(|y| y as i32)
        .or_else(|| v.as_str().and_then(|t| t.get(..4)?.parse().ok()))
        .filter(|y| (1500..=2200).contains(y))
}

/// Names from `[{"name": …}]`, `["…"]`, or `[{"given", "family"}]`.
fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            if let Some(n) = s(a) {
                return Some(n);
            }
            if let Some(n) = s(&a["name"]).or_else(|| s(&a["display_name"])) {
                return Some(n);
            }
            if let Some(n) = s(&a["author"]["display_name"]) {
                return Some(n);
            }
            let given = s(&a["given"]).unwrap_or_default();
            let family = s(&a["family"])?;
            Some(format!("{given} {family}").trim().to_owned())
        })
        .collect()
}

/// Tags and entities stripped from an abstract (Crossref sends JATS).
pub fn plain_text(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut in_tag = false;
    for c in t.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn unescape(t: &str) -> String {
    t.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// One Semantic Scholar paper object (`/graph/v1/paper/search`, a
/// reference's `citedPaper`, a citation's `citingPaper`).
pub fn s2_paper(v: &Value) -> Option<Paper> {
    let title = s(&v["title"])?;
    let ids = &v["externalIds"];
    let doi = s(&ids["DOI"]).and_then(|d| normalize_doi(&d));
    let arxiv = s(&ids["ArXiv"]).map(|a| strip_version(&a));
    let s2_id = s(&v["paperId"]);
    let url = s(&v["url"])
        .or_else(|| {
            s2_id
                .as_ref()
                .map(|id| format!("https://www.semanticscholar.org/paper/{id}"))
        })
        .unwrap_or_default();
    let venue = s(&v["venue"]).or_else(|| s(&v["publicationVenue"]["name"]));
    Some(Paper {
        title,
        authors: names(&v["authors"]),
        year: year_of(&v["year"]),
        venue,
        doi,
        arxiv,
        s2_id,
        citations: v["citationCount"].as_u64(),
        influential_citations: v["influentialCitationCount"].as_u64(),
        abstract_text: s(&v["abstract"]),
        oa_pdf: s(&v["openAccessPdf"]["url"]),
        url,
        found_by: vec![Service::SemanticScholar],
        ..Paper::default()
    })
}

/// Semantic Scholar's paper search: `{"data": [paper, …]}`.
pub fn semantic_scholar(v: &Value) -> Vec<Paper> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(s2_paper)
        .collect()
}

/// Semantic Scholar's references or citations of one paper:
/// `{"data": [{"citedPaper": …}]}` or `{"data": [{"citingPaper": …}]}`.
pub fn semantic_scholar_links(v: &Value) -> Vec<Paper> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let p = if e["citedPaper"].is_object() {
                &e["citedPaper"]
            } else {
                &e["citingPaper"]
            };
            s2_paper(p).map(|mut p| {
                p.influential = e["isInfluential"].as_bool().unwrap_or(false);
                p
            })
        })
        .collect()
}

/// Semantic Scholar's snippet search (`/graph/v1/snippet/search`):
/// passages of a paper's full text with their section and offsets. Returns
/// the papers and one passage per snippet.
pub fn semantic_scholar_snippets(v: &Value) -> (Vec<Paper>, Vec<Passage>) {
    let mut papers = Vec::new();
    let mut passages = Vec::new();
    for e in v["data"].as_array().into_iter().flatten() {
        let sn = &e["snippet"];
        let Some(text) = s(&sn["text"]) else {
            continue;
        };
        let pv = &e["paper"];
        let Some(title) = s(&pv["title"]) else {
            continue;
        };
        let corpus = pv["corpusId"]
            .as_u64()
            .map(|n| n.to_string())
            .or_else(|| s(&pv["corpusId"]));
        let doi = s(&pv["externalIds"]["DOI"]).and_then(|d| normalize_doi(&d));
        let arxiv = s(&pv["externalIds"]["ArXiv"]).map(|a| strip_version(&a));
        let paper = Paper {
            title,
            authors: names(&pv["authors"]),
            year: year_of(&pv["year"]),
            doi,
            arxiv,
            s2_id: corpus.as_ref().map(|c| format!("CorpusId:{c}")),
            oa_pdf: s(&pv["openAccessInfo"]["url"]),
            url: corpus
                .as_ref()
                .map(|c| format!("https://api.semanticscholar.org/CorpusId:{c}"))
                .unwrap_or_default(),
            found_by: vec![Service::SemanticScholar],
            ..Paper::default()
        };
        let kind = match sn["snippetKind"].as_str() {
            Some("title") => PassageKind::Title,
            Some("abstract") => PassageKind::Abstract,
            _ => PassageKind::Body,
        };
        let section = s(&sn["section"]).or_else(|| match kind {
            PassageKind::Abstract => Some("Abstract".to_owned()),
            _ => None,
        });
        let offset = match (
            sn["snippetOffset"]["start"].as_u64(),
            sn["snippetOffset"]["end"].as_u64(),
        ) {
            (Some(a), Some(b)) => Some((a as usize, b as usize)),
            _ => None,
        };
        passages.push(Passage {
            paper: paper.key(),
            text,
            kind,
            locator: Locator {
                page: None,
                section,
                offset,
            },
        });
        papers.push(paper);
    }
    (papers, passages)
}

/// OpenAlex keeps abstracts as an inverted index; this puts the words
/// back in order.
pub fn openalex_abstract(v: &Value) -> Option<String> {
    let idx = v.as_object()?;
    let mut words: Vec<(u64, &str)> = Vec::new();
    for (w, at) in idx {
        for p in at
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            words.push((p, w.as_str()));
        }
    }
    if words.is_empty() {
        return None;
    }
    words.sort_by_key(|(p, _)| *p);
    Some(
        words
            .into_iter()
            .map(|(_, w)| w)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// One OpenAlex work.
pub fn openalex_work(v: &Value) -> Option<Paper> {
    let title = s(&v["display_name"]).or_else(|| s(&v["title"]))?;
    let doi = s(&v["doi"])
        .or_else(|| s(&v["ids"]["doi"]))
        .and_then(|d| normalize_doi(&d));
    let openalex = s(&v["id"]).map(|id| id.trim_start_matches("https://openalex.org/").to_owned());
    let oa_pdf = s(&v["best_oa_location"]["pdf_url"])
        .or_else(|| s(&v["primary_location"]["pdf_url"]))
        .or_else(|| {
            s(&v["open_access"]["oa_url"]).filter(|u| u.to_ascii_lowercase().ends_with(".pdf"))
        });
    let venue = s(&v["primary_location"]["source"]["display_name"]);
    let arxiv = s(&v["ids"]["arxiv"]).and_then(|a| arxiv_id(&a));
    let url = s(&v["id"]).unwrap_or_default();
    Some(Paper {
        title,
        authors: names(&v["authorships"]),
        year: year_of(&v["publication_year"]),
        venue,
        doi,
        arxiv,
        openalex,
        citations: v["cited_by_count"].as_u64(),
        abstract_text: openalex_abstract(&v["abstract_inverted_index"]),
        oa_pdf,
        url,
        found_by: vec![Service::OpenAlex],
        retracted: v["is_retracted"].as_bool().unwrap_or(false),
        ..Paper::default()
    })
}

/// OpenAlex's works search: `{"results": [work, …]}`.
pub fn openalex(v: &Value) -> Vec<Paper> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(openalex_work)
        .collect()
}

/// The text inside the first `<tag …>…</tag>` of `xml`, and the rest.
fn tag<'a>(xml: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{name}");
    let mut from = 0;
    loop {
        let i = from + xml[from..].find(&open)?;
        let after = &xml[i + open.len()..];
        // `<author>` must not match `<authority>`.
        if after.starts_with(['>', ' ', '/', '\n', '\t', '\r']) {
            let gt = after.find('>')?;
            if after[..gt].ends_with('/') {
                from = i + open.len();
                continue;
            }
            let body = &after[gt + 1..];
            let close = format!("</{name}>");
            let end = body.find(&close)?;
            return Some((&body[..end], &body[end + close.len()..]));
        }
        from = i + open.len();
    }
}

/// Every `<tag …>` element's text in `xml`.
fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some((body, after)) = tag(rest, name) {
        out.push(body);
        rest = after;
    }
    out
}

/// `attr="…"` inside one start tag.
fn attr<'a>(start: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let i = start.find(&key)? + key.len();
    let end = start[i..].find('"')?;
    Some(&start[i..i + end])
}

/// arXiv's API answer (an Atom feed).
pub fn arxiv(xml: &str) -> Vec<Paper> {
    tags(xml, "entry")
        .into_iter()
        .filter_map(|e| {
            let title = plain_text(tag(e, "title")?.0);
            if title.is_empty() {
                return None;
            }
            let id_url = plain_text(tag(e, "id")?.0);
            let id = arxiv_id(&id_url)?;
            let authors: Vec<String> = tags(e, "author")
                .into_iter()
                .filter_map(|a| tag(a, "name").map(|(n, _)| plain_text(n)))
                .filter(|n| !n.is_empty())
                .collect();
            let year = tag(e, "published").and_then(|(p, _)| p.trim().get(..4)?.parse().ok());
            let summary = tag(e, "summary").map(|(t, _)| plain_text(t));
            let doi = tag(e, "arxiv:doi").and_then(|(d, _)| normalize_doi(&plain_text(d)));
            let venue = tag(e, "arxiv:journal_ref").map(|(j, _)| plain_text(j));
            let mut pdf = None;
            let mut rest = e;
            while let Some(i) = rest.find("<link") {
                let start = &rest[i..];
                let end = start.find('>').unwrap_or(start.len());
                let l = &start[..end];
                if attr(l, "title") == Some("pdf") || attr(l, "type") == Some("application/pdf") {
                    pdf = attr(l, "href").map(unescape);
                    break;
                }
                rest = &start[end.min(start.len())..];
                if end == 0 {
                    break;
                }
            }
            Some(Paper {
                title,
                authors,
                year,
                venue: venue
                    .filter(|v| !v.is_empty())
                    .or_else(|| Some("arXiv".to_owned())),
                doi,
                arxiv: Some(id.clone()),
                abstract_text: summary.filter(|t| !t.is_empty()),
                oa_pdf: pdf.or_else(|| Some(format!("https://arxiv.org/pdf/{id}"))),
                url: format!("https://arxiv.org/abs/{id}"),
                found_by: vec![Service::Arxiv],
                ..Paper::default()
            })
        })
        .collect()
}

/// One Crossref work (`/works` items, or `/works/{doi}`'s `message`).
pub fn crossref_work(v: &Value) -> Option<Paper> {
    let title = v["title"]
        .as_array()
        .and_then(|t| t.first())
        .and_then(s)
        .or_else(|| s(&v["title"]))?;
    let doi = s(&v["DOI"]).and_then(|d| normalize_doi(&d));
    let year = v["issued"]["date-parts"][0][0]
        .as_i64()
        .or_else(|| v["published"]["date-parts"][0][0].as_i64())
        .map(|y| y as i32);
    let venue = v["container-title"]
        .as_array()
        .and_then(|t| t.first())
        .and_then(s);
    let oa_pdf = v["link"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| l["content-type"] == "application/pdf")
        .and_then(|l| s(&l["URL"]));
    let retracted = v["updated-by"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            v["relation"]["is-retracted-by"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .any(|u| u["type"].as_str().is_none_or(|t| t.contains("retract")))
        || title.to_ascii_uppercase().starts_with("RETRACTED");
    Some(Paper {
        url: doi
            .as_ref()
            .map(|d| format!("https://doi.org/{d}"))
            .or_else(|| s(&v["URL"]))
            .unwrap_or_default(),
        title,
        authors: names(&v["author"]),
        year: year.filter(|y| (1500..=2200).contains(y)),
        venue,
        doi,
        citations: v["is-referenced-by-count"].as_u64(),
        abstract_text: v["abstract"]
            .as_str()
            .map(plain_text)
            .filter(|t| !t.is_empty()),
        oa_pdf,
        found_by: vec![Service::Crossref],
        retracted,
        ..Paper::default()
    })
}

/// Crossref's works search: `{"message": {"items": [work, …]}}`.
pub fn crossref(v: &Value) -> Vec<Paper> {
    let items = &v["message"]["items"];
    if items.is_array() {
        items
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(crossref_work)
            .collect()
    } else {
        crossref_work(&v["message"]).into_iter().collect()
    }
}

/// Unpaywall's answer for one DOI.
pub fn unpaywall(v: &Value) -> Option<Paper> {
    let doi = s(&v["doi"]).and_then(|d| normalize_doi(&d))?;
    let best = &v["best_oa_location"];
    let oa_pdf = s(&best["url_for_pdf"]).or_else(|| {
        v["oa_locations"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|l| s(&l["url_for_pdf"]))
    });
    Some(Paper {
        title: s(&v["title"]).unwrap_or_default(),
        authors: names(&v["z_authors"]),
        year: year_of(&v["year"]),
        venue: s(&v["journal_name"]),
        url: format!("https://doi.org/{doi}"),
        doi: Some(doi),
        oa_pdf,
        found_by: vec![Service::Unpaywall],
        ..Paper::default()
    })
}

/// CORE's works search: `{"results": [work, …]}`.
pub fn core(v: &Value) -> Vec<Paper> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let title = s(&w["title"])?;
            let id = w["id"]
                .as_u64()
                .map(|n| n.to_string())
                .or_else(|| s(&w["id"]));
            let download = s(&w["downloadUrl"]).or_else(|| {
                w["links"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|l| l["type"] == "download")
                    .and_then(|l| s(&l["url"]))
            });
            let venue = w["journals"]
                .as_array()
                .and_then(|j| j.first())
                .and_then(|j| s(&j["title"]))
                .or_else(|| s(&w["publisher"]));
            Some(Paper {
                title,
                authors: names(&w["authors"]),
                year: year_of(&w["yearPublished"]),
                venue,
                doi: s(&w["doi"]).and_then(|d| normalize_doi(&d)),
                arxiv: s(&w["arxivId"]).map(|a| strip_version(&a)),
                core_id: id.clone(),
                citations: w["citationCount"].as_u64(),
                abstract_text: s(&w["abstract"]),
                oa_pdf: download,
                url: id
                    .map(|i| format!("https://core.ac.uk/works/{i}"))
                    .unwrap_or_default(),
                found_by: vec![Service::Core],
                ..Paper::default()
            })
        })
        .collect()
}

/// A recorded or live answer from `service`, read into papers (and, for
/// Semantic Scholar snippets, passages). `body` is JSON text, or Atom XML
/// for arXiv.
pub fn answer(service: Service, body: &str) -> Result<Vec<Paper>, String> {
    if service == Service::Arxiv {
        return Ok(arxiv(body));
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|e| format!("{} sent a reply Tern could not read: {e}", service.name()))?;
    Ok(match service {
        Service::SemanticScholar => semantic_scholar(&v),
        Service::OpenAlex => openalex(&v),
        Service::Crossref => crossref(&v),
        Service::Unpaywall => unpaywall(&v).into_iter().collect(),
        Service::Core => core(&v),
        Service::Arxiv => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn semantic_scholar_papers_and_snippets() {
        let v = json!({"total": 1, "data": [{
            "paperId": "204e3073870fae3d05bcbc2f6a8e263d9b72e776",
            "title": "Attention is All you Need",
            "authors": [{"name": "Ashish Vaswani"}, {"name": "Noam M. Shazeer"}],
            "year": 2017, "venue": "Neural Information Processing Systems",
            "externalIds": {"DOI": "10.48550/arXiv.1706.03762", "ArXiv": "1706.03762"},
            "citationCount": 120000,
            "openAccessPdf": {"url": "https://arxiv.org/pdf/1706.03762"},
            "abstract": "The dominant sequence transduction models ..."}]});
        let ps = semantic_scholar(&v);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].doi.as_deref(), Some("10.48550/arxiv.1706.03762"));
        assert_eq!(ps[0].arxiv.as_deref(), Some("1706.03762"));
        assert_eq!(ps[0].citations, Some(120000));
        assert!(ps[0].url.contains("semanticscholar.org/paper/204e"));
        let sn = json!({"data": [{"score": 0.9,
            "snippet": {"text": "We propose the Transformer.", "snippetKind": "body",
                        "section": "1 Introduction", "snippetOffset": {"start": 10, "end": 37}},
            "paper": {"corpusId": 13756489, "title": "Attention is All you Need",
                      "authors": ["Ashish Vaswani"]}}]});
        let (papers, passages) = semantic_scholar_snippets(&sn);
        assert_eq!(papers.len(), 1);
        assert_eq!(
            passages[0].locator.section.as_deref(),
            Some("1 Introduction")
        );
        assert_eq!(passages[0].locator.offset, Some((10, 37)));
        assert_eq!(passages[0].kind, PassageKind::Body);
        assert_eq!(papers[0].authors, vec!["Ashish Vaswani".to_owned()]);
    }

    #[test]
    fn openalex_rebuilds_the_abstract() {
        let v = json!({"results": [{
            "id": "https://openalex.org/W2963403868", "doi": "https://doi.org/10.1/abc",
            "display_name": "Deep residual learning", "publication_year": 2016,
            "authorships": [{"author": {"display_name": "Kaiming He"}}],
            "primary_location": {"source": {"display_name": "CVPR"}},
            "cited_by_count": 200000,
            "best_oa_location": {"pdf_url": "https://x.org/resnet.pdf"},
            "abstract_inverted_index": {"Deeper": [0], "networks": [1, 4], "are": [2], "hard": [3]},
            "is_retracted": false}]});
        let p = &openalex(&v)[0];
        assert_eq!(
            p.abstract_text.as_deref(),
            Some("Deeper networks are hard networks")
        );
        assert_eq!(p.openalex.as_deref(), Some("W2963403868"));
        assert_eq!(p.venue.as_deref(), Some("CVPR"));
        assert_eq!(p.oa_pdf.as_deref(), Some("https://x.org/resnet.pdf"));
        assert_eq!(p.authors, vec!["Kaiming He".to_owned()]);
    }

    #[test]
    fn arxiv_atom_is_read() {
        let xml = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
<entry><id>http://arxiv.org/abs/2005.11401v4</id><published>2020-05-22T17:56:00Z</published>
<title>Retrieval-Augmented Generation for
  Knowledge-Intensive NLP Tasks</title>
<summary>Large pre-trained language models &amp; more.</summary>
<author><name>Patrick Lewis</name></author><author><name>Ethan Perez</name></author>
<arxiv:doi xmlns:arxiv="http://arxiv.org/schemas/atom">10.5555/x.y</arxiv:doi>
<link href="http://arxiv.org/abs/2005.11401v4" rel="alternate" type="text/html"/>
<link title="pdf" href="http://arxiv.org/pdf/2005.11401v4" rel="related" type="application/pdf"/>
</entry></feed>"#;
        let ps = arxiv(xml);
        assert_eq!(ps.len(), 1);
        let p = &ps[0];
        assert_eq!(
            p.title,
            "Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks"
        );
        assert_eq!(p.arxiv.as_deref(), Some("2005.11401"));
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.authors.len(), 2);
        assert_eq!(
            p.abstract_text.as_deref(),
            Some("Large pre-trained language models & more.")
        );
        assert_eq!(
            p.oa_pdf.as_deref(),
            Some("http://arxiv.org/pdf/2005.11401v4")
        );
        assert_eq!(p.doi.as_deref(), Some("10.5555/x.y"));
    }

    #[test]
    fn crossref_unpaywall_and_core() {
        let v = json!({"message": {"items": [{
            "DOI": "10.1000/XYZ", "title": ["A study"], "author": [{"given": "Ada", "family": "Lovelace"}],
            "issued": {"date-parts": [[1843, 1]]}, "container-title": ["Notes"],
            "is-referenced-by-count": 7, "abstract": "<jats:p>Some &amp; text</jats:p>",
            "updated-by": [{"type": "retraction", "DOI": "10.1000/r"}]}]}});
        let p = &crossref(&v)[0];
        assert_eq!(p.doi.as_deref(), Some("10.1000/xyz"));
        assert_eq!(p.authors, vec!["Ada Lovelace".to_owned()]);
        assert_eq!(p.year, Some(1843));
        assert_eq!(p.abstract_text.as_deref(), Some("Some & text"));
        assert!(p.retracted);
        let u = json!({"doi": "10.1000/xyz", "title": "A study", "year": 1843,
            "best_oa_location": {"url_for_pdf": "https://repo.org/a.pdf"}});
        assert_eq!(
            unpaywall(&u).unwrap().oa_pdf.as_deref(),
            Some("https://repo.org/a.pdf")
        );
        let c = json!({"results": [{"id": 42, "title": "Open paper", "authors": [{"name": "B. C."}],
            "yearPublished": 2020, "downloadUrl": "https://core.ac.uk/download/42.pdf"}]});
        let p = &core(&c)[0];
        assert_eq!(p.core_id.as_deref(), Some("42"));
        assert_eq!(
            p.oa_pdf.as_deref(),
            Some("https://core.ac.uk/download/42.pdf")
        );
        assert!(answer(Service::Crossref, "not json").is_err());
    }
}
