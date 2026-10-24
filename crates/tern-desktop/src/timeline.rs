//! The message timeline rail: a thin rail at the right edge of the chat
//! with one tick per question the user asked. Hovering a tick shows the
//! question's start and when it was asked; clicking scrolls to it; ticks
//! for questions on screen are lit. Built from the transcript, so it works
//! the same for old chats, Research, and Study. Hidden below two questions.
//!
//! No GPUI here: the window passes the user messages and what is on
//! screen, and draws the [`Tick`]s.

/// Characters of the question shown in the hover card.
pub const PREVIEW_CHARS: usize = 80;
/// The rail shows from this many questions.
pub const MIN_TICKS: usize = 2;
/// Space between ticks, at most (px); a long chat packs them closer.
pub const MAX_GAP: f32 = 18.0;
/// Space between ticks, at least (px).
pub const MIN_GAP: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// The transcript entry this question is.
    pub entry: usize,
    /// Which question (0 for the first).
    pub ordinal: usize,
    /// The first [`PREVIEW_CHARS`] characters on one line.
    pub preview: String,
    /// When it was asked, as the chat shows it ("Today 14:05").
    pub when: Option<String>,
    /// Whether the question or its answer is on screen.
    pub lit: bool,
}

/// One question for the rail: its entry index, text, and time label.
#[derive(Clone, Debug)]
pub struct Question<'a> {
    pub entry: usize,
    pub text: &'a str,
    pub when: Option<String>,
}

/// The ticks for `questions` (in transcript order). `on_screen` is the
/// range of entries visible now (first..=last); a question is lit when it
/// or anything before the next question (its answer) is in that range.
pub fn ticks(questions: &[Question<'_>], on_screen: Option<(usize, usize)>) -> Vec<Tick> {
    if questions.len() < MIN_TICKS {
        return Vec::new();
    }
    questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let end = questions.get(i + 1).map_or(usize::MAX, |n| n.entry);
            let lit = on_screen.is_some_and(|(a, b)| q.entry <= b && end > a);
            Tick {
                entry: q.entry,
                ordinal: i,
                preview: preview(q.text),
                when: q.when.clone(),
                lit,
            }
        })
        .collect()
}

/// A question on one line, cut at [`PREVIEW_CHARS`] with an ellipsis.
pub fn preview(text: &str) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= PREVIEW_CHARS {
        return one;
    }
    let cut: String = one.chars().take(PREVIEW_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// The gap between ticks so `n` of them fit in `height` px.
pub fn gap(n: usize, height: f32) -> f32 {
    if n < 2 {
        return MAX_GAP;
    }
    (height / (n - 1) as f32).clamp(MIN_GAP, MAX_GAP)
}

/// The scroll offset (negative, as the scroll handle keeps it) that puts
/// an item whose unscrolled top is `item_top` a little below the top of a
/// viewport starting at `view_top`, clamped to the scrollable range.
pub fn scroll_target(view_top: f32, item_top: f32, max_scroll: f32, margin: f32) -> f32 {
    (view_top - item_top + margin).clamp(-max_scroll.max(0.0), 0.0)
}

/// One step of a smooth scroll from `from` to `to`, `t` (0..1) through,
/// on the house curve's shape (fast start, soft landing).
pub fn ease_scroll(from: f32, to: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    from + (to - from) * e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(entry: usize, text: &str) -> Question<'_> {
        Question {
            entry,
            text,
            when: Some("14:05".into()),
        }
    }

    #[test]
    fn hidden_below_two_questions() {
        assert!(ticks(&[q(0, "only one")], None).is_empty());
        assert_eq!(ticks(&[q(0, "a"), q(2, "b")], None).len(), 2);
    }

    #[test]
    fn previews_are_one_line_and_cut_at_80() {
        let long = "word ".repeat(40);
        let p = preview(&format!("first line\n\n{long}"));
        assert!(!p.contains('\n'));
        assert!(p.ends_with('…'));
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1);
        assert_eq!(preview("  short  question "), "short question");
    }

    #[test]
    fn a_question_is_lit_while_it_or_its_answer_is_on_screen() {
        // Entries: 0 Q, 1 A, 2 notice, 3 Q, 4 A, 5 Q, 6 A.
        let qs = [q(0, "a"), q(3, "b"), q(5, "c")];
        let lit = |r| {
            ticks(&qs, Some(r))
                .iter()
                .map(|t| t.lit)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            lit((1, 2)),
            [true, false, false],
            "only the first answer shows"
        );
        assert_eq!(lit((2, 4)), [true, true, false]);
        assert_eq!(lit((6, 6)), [false, false, true]);
        assert_eq!(ticks(&qs, None).iter().filter(|t| t.lit).count(), 0);
        assert_eq!(ticks(&qs, None)[2].ordinal, 2);
    }

    #[test]
    fn ticks_pack_closer_in_long_chats() {
        assert_eq!(gap(3, 400.), MAX_GAP);
        assert!(gap(200, 400.) < MAX_GAP);
        assert_eq!(gap(1000, 400.), MIN_GAP);
        assert!(
            gap(1000, 400.) * 999. >= 400.,
            "the floor wins over fitting"
        );
    }

    #[test]
    fn scroll_targets_clamp_and_ease() {
        // Viewport top at 100, item at 900 unscrolled: scroll up by 800 - margin.
        assert_eq!(scroll_target(100., 900., 2000., 16.), -784.);
        assert_eq!(scroll_target(100., 50., 2000., 16.), 0.);
        assert_eq!(scroll_target(100., 5000., 2000., 16.), -2000.);
        assert_eq!(ease_scroll(0., -100., 0.), 0.);
        assert_eq!(ease_scroll(0., -100., 1.), -100.);
        assert!(ease_scroll(0., -100., 0.5) < -50., "fast start");
    }
}
