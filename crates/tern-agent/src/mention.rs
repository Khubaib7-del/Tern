//! `@` references in the composer (`docs/02-ui-definition.md`, composer
//! input).
//!
//! | Typed | Means |
//! | --- | --- |
//! | `@name` | Refers to an attachment (or a source from this chat) as context |
//! | `@prompt:name` | The same, read as the user's instructions |
//! | `@context:name` | Back to plain material |
//!
//! A name is the attachment's name as one token: spaces become `-`
//! (`Pasted text 2` is `@Pasted-text-2`). Matching ignores case.

use crate::attach::{Attachment, FileText, Role, Source};
use crate::sources::PageRef;

/// The `@` being typed at the end of the text: where it starts (byte
/// index of `@`), the role it asks for, and what follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active<'a> {
    pub start: usize,
    pub role: Option<Role>,
    pub query: &'a str,
}

/// The `@` token the cursor sits on, when the composer ends in one. An
/// `@` inside a word (an email address) is not a mention.
pub fn active(text: &str) -> Option<Active<'_>> {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map(|(i, _)| i)?;
    let token = text[start..].strip_prefix('@')?;
    let (role, query) = with_role(token);
    Some(Active { start, role, query })
}

fn with_role(token: &str) -> (Option<Role>, &str) {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("prompt:") {
        (Some(Role::Prompt), &token["prompt:".len()..])
    } else if lower.starts_with("context:") {
        (Some(Role::Context), &token["context:".len()..])
    } else {
        (None, token)
    }
}

/// A name as one token.
pub fn slug(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Whether `query` matches `name`: every character of the query appears
/// in order (case-insensitive), so `@rd` finds `README.md`.
pub fn matches(query: &str, name: &str) -> bool {
    let name = slug(name).to_lowercase();
    let mut chars = name.chars();
    query
        .to_lowercase()
        .chars()
        .all(|q| chars.by_ref().any(|c| c == q))
}

/// Ranks a match: exact, then prefix, then anywhere. Lower is better.
pub fn rank(query: &str, name: &str) -> u8 {
    let (q, n) = (query.to_lowercase(), slug(name).to_lowercase());
    if q == n {
        0
    } else if n.starts_with(&q) {
        1
    } else if n.contains(&q) {
        2
    } else {
        3
    }
}

/// Replaces the `@` being typed with the chosen name, keeping its role
/// prefix, and adds a space so typing goes on.
pub fn complete(text: &str, name: &str) -> String {
    let Some(a) = active(text) else {
        return text.to_owned();
    };
    let prefix = match a.role {
        Some(Role::Prompt) => "prompt:",
        Some(Role::Context) => "context:",
        None => "",
    };
    format!("{}@{prefix}{} ", &text[..a.start], slug(name))
}

/// Drops the `@` being typed (used when it opens the file picker instead).
pub fn clear_active(text: &str) -> String {
    match active(text) {
        Some(a) => text[..a.start].to_owned(),
        None => text.to_owned(),
    }
}

/// Every `@` reference in a message, as (role asked for, name).
pub fn references(text: &str) -> Vec<(Option<Role>, String)> {
    text.split_whitespace()
        .filter_map(|w| {
            let token = w.strip_prefix('@')?;
            let token = token.trim_end_matches(|c: char| ",.;:!?)]}\"'".contains(c));
            let (role, name) = with_role(token);
            (!name.is_empty()).then(|| (role, name.to_owned()))
        })
        .collect()
}

/// Applies a message's `@prompt:` and `@context:` references to the
/// attachments they name. Returns the names that matched nothing.
pub fn apply(text: &str, attachments: &mut [Attachment]) -> Vec<String> {
    let mut unknown = Vec::new();
    for (role, name) in references(text) {
        let found = attachments
            .iter_mut()
            .find(|a| slug(&a.name).eq_ignore_ascii_case(&name));
        match (found, role) {
            (Some(a), Some(role)) => a.role = role,
            (Some(_), None) => {}
            (None, _) => unknown.push(name),
        }
    }
    unknown
}

/// A source read earlier in this chat, as an attachment the model can
/// open again with `read_url`.
pub fn source_attachment(page: &PageRef) -> Attachment {
    let title = if page.title.trim().is_empty() {
        page.site.clone()
    } else {
        page.title.clone()
    };
    Attachment {
        name: title.clone(),
        source: Source::Link,
        role: Role::Context,
        files: vec![FileText {
            path: page.url.clone(),
            text: format!(
                "A source from earlier in this chat: {title}\n{}\n\
                 Read it again with read_url if you need its text.",
                page.url
            ),
            cut: false,
        }],
        skipped: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str) -> Attachment {
        let mut a = Attachment::paste("x");
        a.name = name.into();
        a
    }

    #[test]
    fn finds_the_mention_being_typed() {
        assert_eq!(active("hello"), None);
        assert_eq!(active("hello "), None);
        assert_eq!(active("mail me@x.com"), None);
        let a = active("compare @read").unwrap();
        assert_eq!((a.start, a.role, a.query), (8, None, "read"));
        let a = active("@prompt:rub").unwrap();
        assert_eq!((a.start, a.role, a.query), (0, Some(Role::Prompt), "rub"));
        assert_eq!(active("see @").unwrap().query, "");
        assert_eq!(active("ünï @é").unwrap().start, "ünï ".len());
    }

    #[test]
    fn matches_in_order_and_ranks() {
        assert!(matches("rd", "README.md"));
        assert!(matches("pasted-t", "Pasted text 2"));
        assert!(!matches("zz", "README.md"));
        assert!(rank("readme.md", "README.md") < rank("read", "README.md"));
        assert!(rank("read", "README.md") < rank("me", "README.md"));
    }

    #[test]
    fn completes_and_keeps_the_role() {
        assert_eq!(complete("use @pa", "Pasted text 2"), "use @Pasted-text-2 ");
        assert_eq!(complete("@prompt:ru", "rubric.md"), "@prompt:rubric.md ");
        assert_eq!(clear_active("open @fi"), "open ");
    }

    #[test]
    fn references_set_roles_and_report_unknowns() {
        let mut list = vec![att("rubric.md"), att("Pasted text 2"), att("notes")];
        list[2].role = Role::Prompt;
        let unknown = apply(
            "Grade with @prompt:Rubric.md, using @Pasted-text-2 and @context:notes. Also @nope",
            &mut list,
        );
        assert_eq!(list[0].role, Role::Prompt);
        assert_eq!(list[1].role, Role::Context);
        assert_eq!(list[2].role, Role::Context);
        assert_eq!(unknown, vec!["nope".to_owned()]);
    }

    #[test]
    fn a_source_becomes_a_link_attachment() {
        let page = PageRef {
            title: "RAG survey".into(),
            url: "https://arxiv.org/abs/2312.10997".into(),
            site: "arxiv.org".into(),
            builtin: None,
            favicon: None,
        };
        let a = source_attachment(&page);
        assert_eq!(a.source, Source::Link);
        assert_eq!(a.name, "RAG survey");
        assert!(a.files[0].text.contains("arxiv.org/abs/2312.10997"));
    }
}
