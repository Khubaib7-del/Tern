//! Drawing. Unboxed transcript, a working line, the composer, and a footer.
//! Boxes only on dialogs (approval, help). No hue: bold, default, and dim
//! text until the palette decision (ADR 0002) lands.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use tern_core::Measure;
use unicode_width::UnicodeWidthChar;

use super::app::{App, Entry, Status};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_COMPOSER_ROWS: usize = 6;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}
fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Word-wraps `text` to `width` columns, breaking words longer than a line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut w = 0;
        for word in para.split(' ') {
            let ww: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
            let gap = usize::from(!line.is_empty());
            if w + gap + ww <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.push_str(word);
                w += gap + ww;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                w = 0;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if w + cw > width && !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                    w = 0;
                }
                line.push(c);
                w += cw;
            }
        }
        out.push(line);
    }
    out
}

fn indented(text: &str, first: &str, rest: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let pad = first.chars().count();
    wrap(text, width.saturating_sub(pad))
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            Line::from(vec![
                Span::styled(if i == 0 { first } else { rest }.to_owned(), style),
                Span::styled(l, style),
            ])
        })
        .collect()
}

/// Every transcript line at this width.
pub fn transcript_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if app.entries.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("TERN", bold()),
            Span::raw("  "),
            Span::styled("Research", Style::new().add_modifier(Modifier::ITALIC)),
            Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), dim()),
        ]));
        lines.extend(indented(
            &format!("{} · {}", app.provider, app.model),
            "",
            "",
            width,
            dim(),
        ));
        lines.push(Line::raw(""));
        lines.extend(indented(
            "Ask anything. Enter sends, Shift+Enter adds a line, ? shows shortcuts.",
            "",
            "",
            width,
            dim(),
        ));
        return lines;
    }
    for (i, e) in app.entries.iter().enumerate() {
        let after_step = i > 0 && matches!(app.entries[i - 1], Entry::Step(_));
        match e {
            Entry::User(t) => {
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                lines.extend(indented(t, "› ", "  ", width, bold()));
            }
            Entry::Assistant(t) => {
                lines.push(Line::raw(""));
                lines.extend(indented(t, "", "", width, Style::new()));
            }
            Entry::Step(t) => {
                if !after_step {
                    lines.push(Line::raw(""));
                }
                lines.extend(indented(t, "• ", "  ", width, dim()));
            }
            Entry::Notice(t) => {
                lines.push(Line::raw(""));
                lines.extend(indented(t, "! ", "  ", width, bold()));
            }
        }
    }
    lines
}

/// The composer's rows and where the cursor sits in them.
pub fn composer_rows(input: &[char], cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let inner = width.saturating_sub(2).max(1);
    let mut rows = vec![String::new()];
    let mut w = 0;
    let mut at = (0, 0);
    for (i, &c) in input.iter().enumerate() {
        if i == cursor {
            at = (rows.len() - 1, w);
        }
        if c == '\n' {
            rows.push(String::new());
            w = 0;
            continue;
        }
        let cw = c.width().unwrap_or(0);
        if w + cw > inner {
            rows.push(String::new());
            w = 0;
            if i == cursor {
                at = (rows.len() - 1, 0);
            }
        }
        rows.last_mut().expect("one row").push(c);
        w += cw;
    }
    if cursor >= input.len() {
        if w >= inner {
            rows.push(String::new());
            w = 0;
        }
        at = (rows.len() - 1, w);
    }
    (rows, at)
}

fn tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

pub fn footer_text(app: &App) -> String {
    let est = if app.usage.measure == Measure::Estimated {
        "~"
    } else {
        ""
    };
    format!(
        "? shortcuts · {} · {} · {} · {est}{} tokens",
        app.harness,
        app.provider,
        app.model,
        tokens(app.usage.total())
    )
}

fn truncate(s: &str, width: usize) -> String {
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= width {
        return s.to_owned();
    }
    let mut w = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    if area.width < 2 || area.height < 3 {
        return;
    }
    let width = area.width as usize;
    let (rows, (crow, ccol)) = composer_rows(&app.input, app.cursor, width);
    let shown = rows.len().min(MAX_COMPOSER_ROWS);
    let first = (crow + 1).saturating_sub(shown).min(rows.len() - shown);
    let busy_row = u16::from(app.is_busy());
    let [body, working, composer, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(busy_row),
        Constraint::Length(shown as u16 + 1),
        Constraint::Length(1),
    ])
    .areas(area);

    // Transcript, pinned to the bottom unless scrolled.
    let lines = transcript_lines(app, width);
    let h = body.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    let scroll = (app.scroll as usize).min(max_scroll);
    let start = lines.len().saturating_sub(h + scroll);
    let visible: Vec<Line<'_>> = lines.into_iter().skip(start).take(h).collect();
    f.render_widget(Paragraph::new(visible), body);

    if app.is_busy() {
        let secs = app.elapsed().map_or(0, |d| d.as_secs());
        let label = match &app.status {
            Status::Working { label, .. } => label.as_str(),
            Status::Approval { .. } => "Waiting for you",
            Status::Idle => "",
        };
        let hint = if app.stopping {
            "stopping after this step"
        } else {
            "Esc to stop"
        };
        let spin = SPINNER[app.tick % SPINNER.len()];
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!("{spin} ")),
                Span::styled(format!("{label}…"), bold()),
                Span::styled(format!(" {secs}s · {hint}"), dim()),
            ])),
            working,
        );
    }

    // Composer: a dim rule, then the input rows.
    let mut comp = vec![Line::styled("─".repeat(width), dim())];
    for (i, r) in rows.iter().enumerate().skip(first).take(shown) {
        let prefix = if i == 0 { "› " } else { "  " };
        comp.push(Line::from(vec![
            Span::styled(prefix, dim()),
            Span::raw(r.clone()),
        ]));
    }
    if app.input.is_empty() {
        let hint = if app.is_busy() {
            "Type your next message"
        } else {
            "Ask a question"
        };
        comp[1] = Line::from(vec![Span::styled("› ", dim()), Span::styled(hint, dim())]);
    }
    f.render_widget(Paragraph::new(comp), composer);

    f.render_widget(
        Paragraph::new(Line::styled(truncate(&footer_text(app), width), dim())),
        footer,
    );

    let cursor_y = composer.y + 1 + (crow - first) as u16;
    let cursor_x = composer.x + 2 + ccol as u16;
    if let Status::Approval { request, .. } = &app.status {
        draw_approval(f, area, request);
    } else if app.show_help {
        draw_help(f, area);
    } else if cursor_x < area.right() && cursor_y < area.bottom() {
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

fn dialog(area: Rect, want_w: u16, want_h: u16) -> Rect {
    let w = want_w.min(area.width);
    let h = want_h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_approval(f: &mut Frame<'_>, area: Rect, r: &tern_agent::ApprovalRequest) {
    let inner_w = area.width.min(64).saturating_sub(4) as usize;
    let args = serde_json::to_string(&r.args).unwrap_or_default();
    let mut lines = vec![Line::styled(r.summary.clone(), bold()), Line::raw("")];
    let row = |k: &str, v: &str| -> Vec<Line<'static>> {
        indented(v, &format!("{k:<8}"), "        ", inner_w, Style::new())
    };
    lines.extend(row("Tool", &r.tool));
    lines.extend(row("Effect", r.effect.label()));
    if let Some(t) = &r.target {
        lines.extend(row("Target", t));
    }
    lines.extend(row(
        "Input",
        &truncate(&args, inner_w.saturating_sub(8) * 3),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("y", bold()),
        Span::raw(" once   "),
        Span::styled("s", bold()),
        Span::raw(" this session   "),
        Span::styled("n", bold()),
        Span::raw(" no"),
    ]));
    let rect = dialog(area, 64, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title(Span::styled(" Approve this step? ", bold())),
        ),
        rect,
    );
}

const HELP: [(&str, &str); 9] = [
    ("Enter", "send"),
    ("Shift+Enter", "new line (also Alt+Enter, Ctrl+J)"),
    ("Esc", "stop the run"),
    ("PgUp PgDn", "scroll the transcript"),
    ("Ctrl+U", "clear to the start of the line"),
    ("Ctrl+C", "stop, then quit"),
    ("Ctrl+D", "quit when the composer is empty"),
    ("y s n", "approve once, for the session, or refuse"),
    ("?", "this help"),
];

fn draw_help(f: &mut Frame<'_>, area: Rect) {
    let lines: Vec<Line<'_>> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<12} "), bold()),
                Span::raw(*v),
            ])
        })
        .collect();
    let rect = dialog(area, 62, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title(Span::styled(" Shortcuts ", bold())),
        ),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;
    use tern_agent::{ApprovalRequest, Usage};
    use tern_core::EffectClass;

    fn screen(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn busy_app() -> App {
        let mut a = App::new("Groq", "llama-3.3-70b-versatile");
        a.start_turn("What did Vaswani et al. propose in 2017?".into());
        a.on_event("tool.executed", "Ran `web_search`");
        a.entries.push(Entry::Assistant(
            "The Transformer: an encoder-decoder built only on attention, no recurrence. "
                .repeat(6),
        ));
        a.usage = Usage::estimated(16_000, 3_200);
        a
    }

    #[test]
    fn wrap_breaks_on_words_and_splits_long_ones() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("你好世界", 4), ["你好", "世界"]);
    }

    #[test]
    fn composer_tracks_the_cursor_across_rows() {
        let input: Vec<char> = "abcdef".chars().collect();
        let (rows, at) = composer_rows(&input, 6, 5);
        assert_eq!(rows, ["abc", "def", ""]);
        assert_eq!(at, (2, 0));
        let (_, at) = composer_rows(&input, 4, 5);
        assert_eq!(at, (1, 1));
        let nl: Vec<char> = "a\nb".chars().collect();
        assert_eq!(
            composer_rows(&nl, 3, 20),
            (vec!["a".to_owned(), "b".to_owned()], (1, 1))
        );
    }

    #[test]
    fn launch_card_and_footer_fit_80_by_24() {
        let app = App::new("Groq", "llama-3.3-70b-versatile");
        let s = screen(&app, 80, 24);
        assert!(s.contains("TERN  Research"));
        assert!(s.contains("? shortcuts · Research · Groq · llama-3.3-70b-versatile · 0 tokens"));
        assert!(s.contains("› Ask a question"));
    }

    #[test]
    fn busy_screen_shows_working_line_steps_and_estimate() {
        let s = screen(&busy_app(), 80, 24);
        assert!(s.contains("Esc to stop"), "{s}");
        assert!(s.contains("• Ran `web_search`"));
        assert!(s.contains("~4.8k tokens"));
        assert!(s.lines().all(|l| l.chars().count() == 80));
    }

    #[test]
    fn transcript_follows_the_bottom_and_scrolls_back() {
        let mut a = busy_app();
        for i in 0..40 {
            a.entries.push(Entry::User(format!("question {i}")));
        }
        let s = screen(&a, 80, 24);
        assert!(s.contains("question 39"));
        assert!(!s.contains("question 0\n"));
        a.scroll = 1000;
        let s = screen(&a, 80, 24);
        assert!(s.contains("› What did Vaswani"));
    }

    #[test]
    fn approval_dialog_is_boxed_with_its_keys() {
        let mut a = busy_app();
        a.ask_approval(ApprovalRequest {
            tool: "publish".into(),
            summary: "Post a note publicly".into(),
            effect: EffectClass::ExternalSideEffect,
            target: Some("blog.example".into()),
            args: json!({"site": "blog.example", "body": "x".repeat(300)}),
        });
        for (w, h) in [(80, 24), (120, 40)] {
            let s = screen(&a, w, h);
            assert!(s.contains("Approve this step?"), "{s}");
            assert!(s.contains("blog.example"));
            assert!(s.contains("external side effect"));
            assert!(s.contains("y once"));
            assert!(s.contains("╭"));
        }
    }

    #[test]
    fn help_overlay_lists_shortcuts() {
        let mut a = App::new("p", "m");
        a.show_help = true;
        let s = screen(&a, 80, 24);
        assert!(s.contains("Shortcuts"));
        assert!(s.contains("stop the run"));
    }

    #[test]
    fn tiny_and_odd_sizes_never_panic() {
        let mut a = busy_app();
        a.input = "a long draft that wraps over several rows in a narrow terminal "
            .repeat(4)
            .chars()
            .collect();
        a.cursor = 10;
        for (w, h) in [
            (1, 1),
            (2, 3),
            (10, 4),
            (20, 5),
            (40, 10),
            (80, 24),
            (200, 60),
        ] {
            screen(&a, w, h);
        }
        a.ask_approval(ApprovalRequest {
            tool: "t".into(),
            summary: "s".into(),
            effect: EffectClass::Destructive,
            target: None,
            args: json!({}),
        });
        for (w, h) in [(2, 3), (10, 4), (30, 8)] {
            screen(&a, w, h);
        }
    }
}
