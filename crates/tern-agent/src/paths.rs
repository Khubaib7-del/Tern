//! Paths the user types or pastes: `C:\Users\me\notes.txt`,
//! `"C:\My Files\a.md"`, `/home/me/x.md`, `~/x.md`, `file:///tmp/a.txt`.
//!
//! [`user_path`] turns one into a path on this computer (quotes and a
//! `file://` prefix taken off, `~` expanded); [`find_in_text`] picks the
//! first such path out of a message. The file tools use them so
//! "read C:\Users\...\notes.txt" works on Windows, macOS, and Linux.

use std::path::{Path, PathBuf};

/// Whether `s` is written as an absolute or home path on some system:
/// `/x`, `~`, `~/x`, `~\x`, `C:\x`, `C:/x`, or `\\server\share`.
pub fn looks_absolute(s: &str) -> bool {
    let s = strip_wrapping(s.trim());
    let b = s.as_bytes();
    s.starts_with('/')
        || s == "~"
        || s.starts_with("~/")
        || s.starts_with("~\\")
        || s.starts_with("\\\\")
        || s.starts_with("file://")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// Takes surrounding quotes or backticks off (`"..."`, `'...'`, `` `...` ``,
/// `<...>`), as a path pasted from Explorer's "Copy as path" has.
fn strip_wrapping(s: &str) -> &str {
    let s = s.trim();
    for (open, close) in [('"', '"'), ('\'', '\''), ('`', '`'), ('<', '>'), ('“', '”')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            return inner.trim();
        }
    }
    s
}

/// The user's home folder: `HOME`, else `USERPROFILE` (Windows).
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// A typed or pasted path as a path on this computer, with `home` for `~`.
/// Quotes, a `file://` prefix and trailing sentence punctuation are taken
/// off; `%20` in a `file://` address becomes a space.
pub fn user_path_with(raw: &str, home: Option<&Path>) -> PathBuf {
    let mut s = strip_wrapping(raw).to_owned();
    if let Some(rest) = s.strip_prefix("file://") {
        // file:///C:/x on Windows, file:///home/x elsewhere.
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        let b = rest.as_bytes();
        let rest = if b.len() >= 4 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            &rest[1..]
        } else {
            rest
        };
        s = rest.replace("%20", " ");
    }
    if s == "~" {
        if let Some(h) = home {
            return h.to_path_buf();
        }
    } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
        && let Some(h) = home
    {
        let mut p = h.to_path_buf();
        for part in rest.split(['/', '\\']).filter(|p| !p.is_empty()) {
            p.push(part);
        }
        return p;
    }
    PathBuf::from(s)
}

/// [`user_path_with`] with this computer's home folder.
pub fn user_path(raw: &str) -> PathBuf {
    user_path_with(raw, home().as_deref())
}

/// The first absolute or home path in a message ("read C:\Users\me\a.txt
/// please", `summarise "C:\My Files\b.md"`), as typed. A quoted path may
/// hold spaces; an unquoted one ends at whitespace. Trailing `.,;:!?)`
/// is not part of it.
pub fn find_in_text(text: &str) -> Option<String> {
    // Quoted first: "…", '…', `…`.
    for q in ['"', '\'', '`', '“'] {
        let close = if q == '“' { '”' } else { q };
        let mut rest = text;
        while let Some(i) = rest.find(q) {
            let after = &rest[i + q.len_utf8()..];
            let Some(j) = after.find(close) else { break };
            let inner = &after[..j];
            if looks_absolute(inner) {
                return Some(inner.trim().to_owned());
            }
            rest = &after[j + close.len_utf8()..];
        }
    }
    text.split_whitespace()
        .map(|w| w.trim_start_matches(['(', '@']))
        .map(|w| w.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']))
        .find(|w| looks_absolute(w) && w.len() > 1)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_forms_on_every_system() {
        for s in [
            "/home/me/x.md",
            "~/notes.txt",
            "~\\notes.txt",
            "~",
            "C:\\Users\\me\\notes.txt",
            "c:/Users/me/notes.txt",
            "\\\\server\\share\\a.txt",
            "\"C:\\My Files\\a.txt\"",
            "file:///tmp/a.txt",
        ] {
            assert!(looks_absolute(s), "{s}");
        }
        for s in ["notes.txt", "docs/a.md", "C:", "~me", ""] {
            assert!(!looks_absolute(s), "{s}");
        }
    }

    #[test]
    fn home_quotes_and_file_urls() {
        let home = Path::new("/home/me");
        assert_eq!(
            user_path_with("~/docs/a.md", Some(home)),
            Path::new("/home/me/docs/a.md")
        );
        assert_eq!(
            user_path_with("~\\docs\\a.md", Some(home)),
            Path::new("/home/me/docs/a.md")
        );
        assert_eq!(user_path_with("~", Some(home)), home);
        assert_eq!(
            user_path_with("  \"/tmp/My File.txt\" ", Some(home)),
            Path::new("/tmp/My File.txt")
        );
        assert_eq!(
            user_path_with("file:///tmp/My%20File.txt", Some(home)),
            Path::new("/tmp/My File.txt")
        );
        assert_eq!(
            user_path_with("file:///C:/Users/me/a.txt", None),
            PathBuf::from("C:/Users/me/a.txt")
        );
        assert_eq!(
            user_path_with("'C:\\Users\\me\\a.txt'", None),
            PathBuf::from("C:\\Users\\me\\a.txt")
        );
    }

    #[test]
    fn paths_found_in_a_message() {
        assert_eq!(
            find_in_text("read C:\\Users\\me\\notes.txt please").as_deref(),
            Some("C:\\Users\\me\\notes.txt")
        );
        assert_eq!(
            find_in_text("summarise \"C:\\My Files\\b.md\" for me").as_deref(),
            Some("C:\\My Files\\b.md")
        );
        assert_eq!(
            find_in_text("what is in /home/me/x.md?").as_deref(),
            Some("/home/me/x.md")
        );
        assert_eq!(find_in_text("open ~/a.txt.").as_deref(), Some("~/a.txt"));
        assert_eq!(find_in_text("who created LangChain?"), None);
        assert_eq!(find_in_text("a/b and c"), None);
    }
}
