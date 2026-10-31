//! Follow-ups that point back at the previous answer (#85, A9).
//!
//! "Read the top 5 papers from that list" is about the papers the last
//! answer named, not a new topic: Tern runs no prefetch search for it
//! (a search for "top 5 papers list" found junk), and hands the model the
//! paper ids the previous answer and its tool results carried, in the
//! order the answer gave them.

use std::sync::LazyLock;

use regex::Regex;

/// Whether `text` only points back at the previous answer ("that list",
/// "those papers", "the top 5 from it", "the ones you found").
pub fn points_back(text: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?x)
            \b(that|this|the\s+(above|previous|last|same))\s+(list|answer|reply|table|search|set|results?)\b
            | \b(those|these|the\s+same)\s+(\d+\s+)?(papers?|studies|articles|results|sources|citers|citations|ones|works|references)\b
            | \bfrom\s+(above|before|earlier|your\s+(answer|list|reply))\b
            | \b(you|we)\s+(just\s+)?(found|listed|mentioned|gave|cited|read|showed|named)\b
            | \bthe\s+(first|top|last)\s+(\d+|one|two|three|four|five|six|seven|eight|nine|ten)(\s+\w+)?\s+(from|in|of|on)\s+(it|that|this|there|them|above)\b
            ",
        )
        .expect("static regex")
    });
    RE.is_match(&text.to_lowercase())
}

/// How many the message asks for ("the top 5", "first three"), if it says.
pub fn wanted_count(text: &str) -> Option<usize> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b(?:top|first|last)\s+(\d{1,2}|one|two|three|four|five|six|seven|eight|nine|ten)\b",
        )
        .expect("static regex")
    });
    let lower = text.to_lowercase();
    let w = RE.captures(&lower)?.get(1)?.as_str().to_owned();
    let n = match w.as_str() {
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        d => d.parse().ok()?,
    };
    (n > 0).then_some(n)
}

/// The paper ids (`arXiv:…`, DOIs) in `texts`, in order of first
/// appearance, without repeats; at most 20. Pass the previous answer
/// first so its order wins over the tool results behind it.
pub fn paper_ids<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?ix)
            arxiv\.org/(?:abs|pdf)/(?P<a1>\d{4}\.\d{4,5})
            | arxiv[:\s]\s*(?P<a2>\d{4}\.\d{4,5})
            | "arxiv"\s*:\s*"(?P<a3>\d{4}\.\d{4,5})
            | \b(?P<doi>10\.\d{4,9}/[^\s"'<>\)\]\},;]+)
            "#,
        )
        .expect("static regex")
    });
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        for c in RE.captures_iter(t) {
            let id = if let Some(a) = c
                .name("a1")
                .or_else(|| c.name("a2"))
                .or_else(|| c.name("a3"))
            {
                format!("arXiv:{}", a.as_str())
            } else if let Some(d) = c.name("doi") {
                let d = d.as_str().trim_end_matches(['.', ':']);
                // An arXiv DOI is the arXiv paper.
                match d.to_ascii_lowercase().strip_prefix("10.48550/arxiv.") {
                    Some(a) => format!("arXiv:{a}"),
                    None => d.to_owned(),
                }
            } else {
                continue;
            };
            let key = id.to_ascii_lowercase();
            if !out.iter().any(|o| o.to_ascii_lowercase() == key) {
                out.push(id);
            }
            if out.len() >= 20 {
                return out;
            }
        }
    }
    out
}

/// The note handed to the model with a follow-up: the previous answer's
/// papers, numbered in its order, `wanted` of them first.
pub fn note(ids: &[String], wanted: Option<usize>) -> String {
    let list: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| format!("{}. {id}", i + 1))
        .collect();
    let take = match wanted {
        Some(n) => format!(
            " The message asks for {n}: read the first {} of them with `paper_read`, by these ids.",
            n.min(ids.len())
        ),
        None => " Read the ones the message asks for with `paper_read`, by these ids.".to_owned(),
    };
    format!(
        "(This message points back at the previous answer. Its papers, in the order it gave them:\n{}\n{take} Do not search again for them.)",
        list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_ups_that_point_back() {
        for t in [
            "Read the top 5 papers from that list",
            "summarise those papers",
            "which of these studies used a control group?",
            "read the first three from it",
            "open the ones you found",
            "compare the papers from your answer",
        ] {
            assert!(points_back(t), "{t}");
        }
        for t in [
            "How does FLARE compare with retrieving before every sentence?",
            "find papers on retrieval augmented generation",
            "what is the top speed of a cheetah",
        ] {
            assert!(!points_back(t), "{t}");
        }
    }

    #[test]
    fn counts() {
        assert_eq!(
            wanted_count("Read the top 5 papers from that list"),
            Some(5)
        );
        assert_eq!(wanted_count("read the first three"), Some(3));
        assert_eq!(wanted_count("read them"), None);
    }

    #[test]
    fn ids_in_answer_order_without_repeats() {
        let answer = "1. FLARE (https://arxiv.org/abs/2305.06983) 2. Self-RAG arXiv:2310.11511 \
                      3. A journal paper https://doi.org/10.1145/3571730. 4. again arxiv.org/pdf/2305.06983v2";
        let tool = r#"{"arxiv": "2005.11401", "doi": "10.48550/arXiv.2310.11511"}"#;
        assert_eq!(
            paper_ids([answer, tool]),
            vec![
                "arXiv:2305.06983",
                "arXiv:2310.11511",
                "10.1145/3571730",
                "arXiv:2005.11401"
            ]
        );
    }

    #[test]
    fn note_names_the_count() {
        let n = note(&["arXiv:1".into(), "arXiv:2".into()], Some(5));
        assert!(n.contains("1. arXiv:1") && n.contains("first 2"), "{n}");
    }
}
