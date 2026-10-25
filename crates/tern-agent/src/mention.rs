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
    // A quoted path being typed or pasted (`@"C:\My Files\a.txt"`) may
    // hold spaces: it runs from `@"` to its closing quote or the end.
    if let Some(i) = text.rfind("@\"") {
        let rest = &text[i + 2..];
        let quotes = rest.matches('"').count();
        let open = quotes == 0 || (quotes == 1 && rest.ends_with('"'));
        if open && text[..i].chars().last().is_none_or(char::is_whitespace) {
            return Some(Active {
                start: i,
                role: None,
                query: &text[i + 1..],
            });
        }
    }
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

/// The file an `@` query names when it is a full path the user typed or
/// pasted (`@C:\Users\me\a.txt`, `@/home/me/x.md`, `@~/x.md`,
/// `@"C:\My Files\a.txt"`), so the menu can offer to attach it.
pub fn typed_path(query: &str) -> Option<std::path::PathBuf> {
    let q = query.trim().trim_matches(['"', '\'']);
    (crate::paths::looks_absolute(q) && q.len() > 1).then(|| crate::paths::user_path(q))
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
            // An MCP resource is the server's data: it stays Context.
            (Some(a), Some(_)) if a.source == Source::Mcp => {}
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
        images: Vec::new(),
    }
}

/// One entry of a folder the `@` menu lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub path: std::path::PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// The folder a typed `@` query browses: the query when it is a folder
/// (`C:\Users\me\`, `~/docs/`), else the folder part of a partly typed
/// path (`~/docs/no` browses `~/docs` for names starting `no`). Returns
/// the folder and the name filter.
pub fn browsed_dir(query: &str) -> Option<(std::path::PathBuf, String)> {
    let q = query.trim().trim_matches('"');
    if !crate::paths::looks_absolute(q) {
        return None;
    }
    let full = crate::paths::user_path(q);
    if (q.ends_with('/') || q.ends_with('\\') || q == "~") && full.is_dir() {
        return Some((full, String::new()));
    }
    let cut = q.rfind(['/', '\\'])?;
    let (dir, rest) = (&q[..=cut], &q[cut + 1..]);
    let dir = crate::paths::user_path(dir);
    dir.is_dir().then(|| (dir, rest.to_owned()))
}

/// A folder's entries for the `@` menu: folders first, then files, by
/// name, hidden ones left out, those starting with `filter` (any case).
pub fn list_dir(dir: &std::path::Path, filter: &str, limit: usize) -> Vec<DirEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let f = filter.to_lowercase();
    let mut out: Vec<DirEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.to_lowercase().starts_with(&f) {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            Some(DirEntry {
                path: e.path(),
                name,
                is_dir,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out.truncate(limit);
    out
}

/// Files under a workspace folder whose names match `query`, best first,
/// for the `@` menu: walks at most a few thousand entries, skipping
/// hidden folders, `node_modules`, and `target`. Folders are offered only
/// for a non-empty query.
pub fn workspace_files(root: &std::path::Path, query: &str, limit: usize) -> Vec<DirEntry> {
    let mut found: Vec<(u8, DirEntry)> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    'walk: while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 4_000 {
                break 'walk;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if depth < 6 {
                    stack.push((e.path(), depth + 1));
                }
                if !query.is_empty() && matches(query, &name) {
                    let entry = DirEntry {
                        path: e.path(),
                        name,
                        is_dir: true,
                    };
                    found.push((rank(query, &entry.name).saturating_add(1), entry));
                }
            } else if matches(query, &name) {
                let entry = DirEntry {
                    path: e.path(),
                    name,
                    is_dir: false,
                };
                found.push((rank(query, &entry.name), entry));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    found.into_iter().map(|(_, e)| e).take(limit).collect()
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

    #[test]
    fn a_typed_or_pasted_path_is_offered() {
        let a = active("read @C:\\Users\\me\\notes.txt").unwrap();
        assert_eq!(a.query, "C:\\Users\\me\\notes.txt");
        assert_eq!(
            typed_path(a.query),
            Some(std::path::PathBuf::from("C:\\Users\\me\\notes.txt"))
        );
        let a = active("see @\"C:\\My Files\\a b.txt").unwrap();
        assert_eq!(a.start, 4);
        assert_eq!(
            typed_path(a.query),
            Some(std::path::PathBuf::from("C:\\My Files\\a b.txt"))
        );
        let a = active("see @\"/tmp/My Files/a.txt\"").unwrap();
        assert_eq!(
            typed_path(a.query),
            Some(std::path::PathBuf::from("/tmp/My Files/a.txt"))
        );
        assert!(typed_path(active("@/home/me/x.md").unwrap().query).is_some());
        assert_eq!(typed_path("README"), None);
        assert_eq!(typed_path("/"), None);
        // A closed quote earlier is not a mention being typed.
        assert!(active("see @\"/tmp/a.txt\" and more").is_none());
    }
}

#[cfg(test)]
mod browse_tests {
    use super::*;

    #[test]
    fn a_typed_folder_is_browsed_and_workspace_files_are_found() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("notes")).unwrap();
        std::fs::write(d.path().join("notes").join("plan.md"), "x").unwrap();
        std::fs::write(d.path().join("readme.txt"), "x").unwrap();
        std::fs::write(d.path().join(".hidden"), "x").unwrap();
        let root = format!("{}{}", d.path().display(), std::path::MAIN_SEPARATOR);
        let (dir, filter) = browsed_dir(&root).unwrap();
        assert_eq!(filter, "");
        let names: Vec<String> = list_dir(&dir, &filter, 10)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["notes", "readme.txt"], "folders first, hidden left out");
        let (dir, filter) = browsed_dir(&format!("{root}rea")).unwrap();
        assert_eq!(list_dir(&dir, &filter, 10)[0].name, "readme.txt");
        assert!(browsed_dir("notes/").is_none(), "only full paths browse");
        let hits = workspace_files(d.path(), "plan", 5);
        assert_eq!(hits[0].name, "plan.md");
        assert!(!hits[0].is_dir);
        // An empty query lists files only.
        assert!(workspace_files(d.path(), "", 10).iter().all(|e| !e.is_dir));
    }
}
