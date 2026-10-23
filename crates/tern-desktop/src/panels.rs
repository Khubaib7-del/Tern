//! Side panel sizes, maximising, the tab strip's overflow, and the
//! Archived chats view. GPUI-free so it is unit tested.
//!
//! | Panel | Default | Min | Max |
//! | --- | --- | --- | --- |
//! | Sidebar | 264 | 200 | 420 |
//! | Right pane (run panel, side chat, browser) | 300 | 280 | 720 |
//! | Centre | – | 520 | – |
//!
//! A drag handle sits on each inner edge (4 px hit area); double-click
//! resets it. Widths are saved in preferences after a drag ends.

use tern_agent::SessionSummary;

pub const SIDEBAR_DEFAULT: f32 = 264.;
pub const SIDEBAR_MIN: f32 = 200.;
pub const SIDEBAR_MAX: f32 = 420.;
pub const RIGHT_DEFAULT: f32 = 300.;
pub const RIGHT_MIN: f32 = 280.;
pub const RIGHT_MAX: f32 = 720.;
pub const CENTRE_MIN: f32 = 520.;
/// The drag handle's hit area.
pub const HANDLE_W: f32 = 4.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Between the sidebar and the centre.
    Sidebar,
    /// Between the centre and the right pane.
    Right,
}

/// Which pane fills the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Normal,
    /// The right pane takes the centre; the chat folds away.
    RightMax,
    /// The chat alone: sidebar and right pane folded.
    CentreMax,
}

/// Panel widths and the drag in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panels {
    pub sidebar: f32,
    pub right: f32,
    pub focus: Focus,
    /// The edge being dragged, and the pointer x and width when it began.
    drag: Option<(Edge, f32, f32)>,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            sidebar: SIDEBAR_DEFAULT,
            right: RIGHT_DEFAULT,
            focus: Focus::Normal,
            drag: None,
        }
    }
}

impl Panels {
    /// Widths from preferences; out-of-range values are clamped.
    pub fn from_prefs(sidebar: Option<u32>, right: Option<u32>) -> Self {
        Self {
            sidebar: sidebar.map_or(SIDEBAR_DEFAULT, |w| {
                (w as f32).clamp(SIDEBAR_MIN, SIDEBAR_MAX)
            }),
            right: right.map_or(RIGHT_DEFAULT, |w| (w as f32).clamp(RIGHT_MIN, RIGHT_MAX)),
            ..Self::default()
        }
    }

    /// The widths to save; `None` for a default width.
    pub fn to_prefs(self) -> (Option<u32>, Option<u32>) {
        let keep = |w: f32, d: f32| ((w - d).abs() >= 1.).then_some(w.round() as u32);
        (
            keep(self.sidebar, SIDEBAR_DEFAULT),
            keep(self.right, RIGHT_DEFAULT),
        )
    }

    pub fn start_drag(&mut self, edge: Edge, x: f32) {
        let w = match edge {
            Edge::Sidebar => self.sidebar,
            Edge::Right => self.right,
        };
        self.drag = Some((edge, x, w));
    }

    pub fn dragging(&self) -> Option<Edge> {
        self.drag.map(|(e, _, _)| e)
    }

    /// The pointer moved to `x` in a window `window_w` wide, with the other
    /// panel taking `other` (0 when closed). Keeps the centre at least
    /// [`CENTRE_MIN`].
    pub fn drag_to(&mut self, x: f32, window_w: f32, rail: f32, other: f32) {
        let Some((edge, x0, w0)) = self.drag else {
            return;
        };
        let room = (window_w - rail - other - CENTRE_MIN).max(0.);
        match edge {
            Edge::Sidebar => {
                let max = SIDEBAR_MAX.min(room).max(SIDEBAR_MIN);
                self.sidebar = (w0 + (x - x0)).clamp(SIDEBAR_MIN, max);
            }
            Edge::Right => {
                let max = RIGHT_MAX.min(room).max(RIGHT_MIN);
                // The right pane grows as the pointer moves left.
                self.right = (w0 - (x - x0)).clamp(RIGHT_MIN, max);
            }
        }
    }

    /// The drag ended; `true` when a width changed and should be saved.
    pub fn end_drag(&mut self) -> bool {
        match self.drag.take() {
            Some((Edge::Sidebar, _, w0)) => (self.sidebar - w0).abs() >= 1.,
            Some((Edge::Right, _, w0)) => (self.right - w0).abs() >= 1.,
            None => false,
        }
    }

    /// Double-click on a handle.
    pub fn reset(&mut self, edge: Edge) {
        match edge {
            Edge::Sidebar => self.sidebar = SIDEBAR_DEFAULT,
            Edge::Right => self.right = RIGHT_DEFAULT,
        }
    }

    pub fn toggle_right_max(&mut self) {
        self.focus = if self.focus == Focus::RightMax {
            Focus::Normal
        } else {
            Focus::RightMax
        };
    }

    pub fn toggle_centre_max(&mut self) {
        self.focus = if self.focus == Focus::CentreMax {
            Focus::Normal
        } else {
            Focus::CentreMax
        };
    }

    /// Whether the chat column shows.
    pub fn shows_centre(&self) -> bool {
        self.focus != Focus::RightMax
    }

    /// Whether the sidebar may show (it still needs to be open).
    pub fn shows_sidebar(&self) -> bool {
        self.focus == Focus::Normal
    }

    pub fn shows_right(&self) -> bool {
        self.focus != Focus::CentreMax
    }
}

/// How many tabs of `widths` fit in `room` px with an overflow button of
/// `more_w` px; the active tab is always among those shown. Returns the
/// indices shown, in order, and the hidden ones for the ▾ menu.
pub fn tab_overflow(
    widths: &[f32],
    room: f32,
    more_w: f32,
    active: usize,
) -> (Vec<usize>, Vec<usize>) {
    let total: f32 = widths.iter().sum();
    if total <= room {
        return ((0..widths.len()).collect(), Vec::new());
    }
    let room = (room - more_w).max(0.);
    let mut shown: Vec<usize> = Vec::new();
    let mut used = widths.get(active).copied().unwrap_or(0.);
    if active < widths.len() {
        shown.push(active);
    }
    for (i, w) in widths.iter().enumerate() {
        if i == active {
            continue;
        }
        if used + w <= room {
            used += w;
            shown.push(i);
        }
    }
    shown.sort_unstable();
    let hidden = (0..widths.len()).filter(|i| !shown.contains(i)).collect();
    (shown, hidden)
}

/// A rough width for a tab title, before layout: characters × 7 px plus
/// padding and the close button, capped at 200.
pub fn tab_width(title: &str) -> f32 {
    (title.chars().count() as f32 * 7. + 44.).min(200.)
}

/// The Archived chats view: which list, and its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArchiveTab {
    #[default]
    Archived,
    Deleted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchiveView {
    pub tab: ArchiveTab,
    pub archived: Vec<SessionSummary>,
    pub deleted: Vec<SessionSummary>,
    pub loaded: bool,
    /// A chat waiting for "Delete for good?" to be confirmed, or `None`
    /// with `confirm_empty` for Empty trash.
    pub confirm: Option<tern_core::SessionId>,
    pub confirm_empty: bool,
    pub note: Option<String>,
}

impl ArchiveView {
    pub fn loaded(&mut self, archived: Vec<SessionSummary>, deleted: Vec<SessionSummary>) {
        self.archived = archived;
        self.deleted = deleted;
        self.loaded = true;
        self.confirm = None;
        self.confirm_empty = false;
    }

    pub fn rows(&self) -> &[SessionSummary] {
        match self.tab {
            ArchiveTab::Archived => &self.archived,
            ArchiveTab::Deleted => &self.deleted,
        }
    }

    /// Takes a row out at once (the bridge does the rest).
    pub fn drop_row(&mut self, id: tern_core::SessionId) {
        self.archived.retain(|s| s.id != id);
        self.deleted.retain(|s| s.id != id);
        if self.confirm == Some(id) {
            self.confirm = None;
        }
    }

    /// "3 archived", "1 in the trash".
    pub fn counts(&self) -> (String, String) {
        (
            format!("{} archived", self.archived.len()),
            format!("{} in the trash", self.deleted.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_clamp_persist_and_reset() {
        let p = Panels::from_prefs(Some(1000), Some(10));
        assert_eq!(p.sidebar, SIDEBAR_MAX);
        assert_eq!(p.right, RIGHT_MIN);
        assert_eq!(Panels::default().to_prefs(), (None, None));
        let mut p = Panels::default();
        p.start_drag(Edge::Sidebar, 300.);
        p.drag_to(340., 1920., 52., 300.);
        assert_eq!(p.sidebar, SIDEBAR_DEFAULT + 40.);
        assert!(p.end_drag());
        assert_eq!(p.to_prefs(), (Some(304), None));
        let back = Panels::from_prefs(Some(304), None);
        assert_eq!(back.sidebar, 304.);
        p.reset(Edge::Sidebar);
        assert_eq!(p.sidebar, SIDEBAR_DEFAULT);
        assert!(!p.end_drag(), "no drag, nothing to save");
    }

    #[test]
    fn right_pane_grows_leftwards_and_keeps_the_centre() {
        let mut p = Panels::default();
        p.start_drag(Edge::Right, 1000.);
        p.drag_to(900., 1920., 52., 264.);
        assert_eq!(p.right, RIGHT_DEFAULT + 100.);
        // At 1280 with the sidebar open the centre keeps 520.
        p.drag_to(0., 1280., 52., 264.);
        assert_eq!(p.right, 1280. - 52. - 264. - CENTRE_MIN);
        p.drag_to(5000., 1280., 52., 264.);
        assert_eq!(p.right, RIGHT_MIN);
        // Even in a tiny window the pane keeps its minimum.
        p.drag_to(0., 600., 52., 264.);
        assert_eq!(p.right, RIGHT_MIN);
        assert!(p.end_drag());
    }

    #[test]
    fn maximise_and_restore() {
        let mut p = Panels::default();
        p.toggle_right_max();
        assert!(!p.shows_centre() && p.shows_right() && !p.shows_sidebar());
        p.toggle_right_max();
        assert_eq!(p.focus, Focus::Normal);
        p.toggle_centre_max();
        assert!(p.shows_centre() && !p.shows_right() && !p.shows_sidebar());
        p.toggle_right_max();
        assert_eq!(p.focus, Focus::RightMax, "one maximised pane at a time");
    }

    #[test]
    fn tabs_overflow_into_a_menu_keeping_the_active_one() {
        let w = [100., 100., 100., 100., 100., 100., 100., 100.];
        let (shown, hidden) = tab_overflow(&w, 2000., 30., 0);
        assert_eq!(shown.len(), 8);
        assert!(hidden.is_empty());
        let (shown, hidden) = tab_overflow(&w, 430., 30., 7);
        assert_eq!(shown, [0, 1, 2, 7]);
        assert_eq!(hidden, [3, 4, 5, 6]);
        assert_eq!(shown.len() + hidden.len(), 8, "every tab reachable");
        assert!(tab_width("A very long chat title that keeps going and going") <= 200.);
    }

    #[test]
    fn archive_view_lists_and_drops() {
        let mut a = ArchiveView::default();
        let s = |t: &str| SessionSummary {
            id: tern_core::SessionId::new(),
            title: t.into(),
            harness: tern_agent::Harness::Research,
            created_at: String::new(),
            events: 2,
            parent: None,
            anchor: None,
            plan: None,
            project: None,
        };
        let (x, y) = (s("Old"), s("Gone"));
        let yid = y.id;
        a.loaded(vec![x], vec![y]);
        assert_eq!(a.rows().len(), 1);
        a.tab = ArchiveTab::Deleted;
        assert_eq!(a.rows()[0].title, "Gone");
        a.confirm = Some(yid);
        a.drop_row(yid);
        assert!(a.rows().is_empty());
        assert!(a.confirm.is_none());
        assert_eq!(a.counts(), ("1 archived".into(), "0 in the trash".into()));
    }
}
