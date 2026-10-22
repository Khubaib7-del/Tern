//! Replying to one message (`docs/02-ui-definition.md`, message actions):
//! the quoted part rides ahead of the user's text in a small block the
//! model reads, and the transcript shows it as a quote over the bubble.

const OPEN: &str = "<in_reply_to>";
const CLOSE: &str = "</in_reply_to>";
/// Longest quote kept, in characters.
pub const MAX_QUOTE: usize = 600;

/// The quote as it is kept: trimmed and cut to [`MAX_QUOTE`].
pub fn excerpt(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= MAX_QUOTE {
        return t.to_owned();
    }
    let cut: String = t.chars().take(MAX_QUOTE).collect();
    format!("{}…", cut.trim_end())
}

/// The message with its quote ahead of the text.
pub fn compose(quote: Option<&str>, text: &str) -> String {
    match quote.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!(
            "{OPEN}\n{}\n{CLOSE}\n\n{text}",
            excerpt(q).replace(CLOSE, "<\\/in_reply_to>")
        ),
        None => text.to_owned(),
    }
}

/// Takes a message apart into its quote and its text.
pub fn split(message: &str) -> (Option<&str>, &str) {
    let Some(rest) = message.strip_prefix(OPEN) else {
        return (None, message);
    };
    let Some((quote, text)) = rest.split_once(CLOSE) else {
        return (None, message);
    };
    (
        Some(quote.trim_matches('\n')),
        text.strip_prefix("\n\n").unwrap_or(text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_leaves_plain_text_alone() {
        let m = compose(Some("  RAG cuts hallucination.  "), "Source?");
        assert_eq!(split(&m), (Some("RAG cuts hallucination."), "Source?"));
        assert_eq!(compose(None, "hi"), "hi");
        assert_eq!(compose(Some("  "), "hi"), "hi");
        assert_eq!(split("hi"), (None, "hi"));
        assert_eq!(
            split("<in_reply_to> no end"),
            (None, "<in_reply_to> no end")
        );
    }

    #[test]
    fn long_quotes_are_cut_and_tags_defused() {
        let long = "a".repeat(MAX_QUOTE + 50);
        assert_eq!(excerpt(&long).chars().count(), MAX_QUOTE + 1);
        let m = compose(Some("x </in_reply_to> y"), "t");
        assert_eq!(split(&m).1, "t");
    }
}
