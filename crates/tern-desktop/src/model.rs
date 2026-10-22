//! What the desktop window shows, kept apart from GPUI so it can be tested
//! without a display. The view reads this; the bridge feeds it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tern_agent::attach::{self, Attachment, Chip, Role};
use tern_agent::sources::{page_of, push_unique};
use tern_agent::{
    ApprovalRequest, ContextUse, Harness, PageRef, PastTurn, RawFilter, Run, SessionSummary,
    TurnOutcome,
};
use tern_agent::{mention, reply};
use tern_core::{EventEnvelope, TurnState};

use crate::providers::ProvidersState;

/// What the centre of the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Chat,
    Providers,
    /// The vault's notes.
    Editor,
}

/// One block of the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// What the user sent, with chips for what they attached.
    User {
        text: String,
        chips: Vec<Chip>,
        /// The part of an earlier message this one replies to.
        quote: Option<String>,
    },
    /// A finished answer, with the run that led to it and how long it took.
    Assistant {
        text: String,
        run: Run,
        took: Duration,
        /// Pages read on the way, shown as source rows.
        pages: Vec<PageRef>,
    },
    /// A plain note from Tern (a stop reason, a phase that is not built yet).
    Notice(String),
}

/// A site's favicon, by site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteIcon {
    Loading,
    Ready(PathBuf),
    /// No usable icon; the row shows the site's initial.
    Missing,
}

/// Whether the window can take a message.
#[derive(Debug)]
pub enum Phase {
    Connecting,
    /// No provider could be opened. The text says what to do next.
    Offline(String),
    Ready,
    Working {
        since: Instant,
        activity: Activity,
        run: Run,
        /// Reply text streaming in, and which model call of the turn it is.
        draft: String,
        draft_call: u32,
    },
    Approval {
        since: Instant,
        run: Run,
        request: ApprovalRequest,
    },
}

/// What a running turn is doing, for the working line's mark and phrase
/// (`docs/02-ui-definition.md` §2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Thinking,
    Searching,
    Reading,
    /// Running a tool other than search or read.
    Tool,
    Writing,
    NeedsYou,
    Stopping,
}

/// The working line, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingLine {
    pub activity: Activity,
    pub phrase: String,
    pub seconds: u64,
    /// Bumps on every real event (a step, a page, a burst of text), so the
    /// mark can beat in time with the work rather than a timer.
    pub beat: usize,
}

/// A question, its answer, and the answer's sources as (title, url).
pub type AnswerNote = (String, String, Vec<(String, String)>);

/// The `@` menu: what is being typed and which row is highlighted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MentionMenu {
    /// Byte index of the `@` in the composer text.
    pub start: usize,
    pub query: String,
    pub role: Option<Role>,
    pub selected: usize,
}

/// What a row of the `@` menu picks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Attachment(usize),
    Source(PageRef),
    /// Opens the file and folder picker.
    Browse,
}

/// One row of the `@` menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub pick: Pick,
    pub label: String,
    pub detail: String,
}

/// What choosing a row does to the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub text: String,
    pub browse: bool,
}

const MENU_ROWS: usize = 8;

/// The run panel's three views of the same run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunView {
    #[default]
    Summary,
    Detailed,
    Raw,
}

impl RunView {
    pub const ALL: [RunView; 3] = [Self::Summary, Self::Detailed, Self::Raw];

    pub fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Detailed => "Detailed",
            Self::Raw => "Raw",
        }
    }
}

/// The run panel's own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPanel {
    pub view: RunView,
    pub filter: RawFilter,
    /// Raw rows opened to show their payload, by event seq and position.
    pub open: std::collections::HashSet<usize>,
    /// The answer whose run is shown (an index into `entries`), when the user
    /// picked an older one; otherwise the latest run.
    pub pinned: Option<usize>,
}

/// The connected model, shown under the composer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connection {
    pub provider: String,
    pub model: String,
    pub search: Option<String>,
    pub context_window: Option<u64>,
}

/// The whole window's state.
#[derive(Debug)]
pub struct Model {
    pub entries: Vec<Entry>,
    pub phase: Phase,
    pub connection: Option<Connection>,
    /// How full the model's context window is.
    pub context: ContextUse,
    pub harness: Harness,
    pub sessions: Vec<SessionSummary>,
    pub sidebar_open: bool,
    pub run_panel_open: bool,
    pub run_panel: RunPanel,
    /// The latest turn's run, kept even when it ended without an answer.
    pub last_run: Run,
    pub dark: bool,
    pub stopping: bool,
    pub screen: Screen,
    pub providers: ProvidersState,
    /// Pages read so far in the running turn.
    pub turn_pages: Vec<PageRef>,
    pub icons: HashMap<String, SiteIcon>,
    /// Attachments waiting in the composer, sent with the next message.
    pub attachments: Vec<Attachment>,
    /// The `@` menu over the composer, while a reference is being typed.
    pub mention: Option<MentionMenu>,
    /// The Editor screen.
    pub editor: crate::editor::EditorState,
    /// The answer just saved to the vault (its icon shows a check).
    pub saved_answer: Option<usize>,
    /// The quote the next message replies to.
    pub reply_to: Option<String>,
    /// The entry whose text was just copied (its icon shows a check).
    pub copied: Option<usize>,
    /// Attachments still being read from disk.
    pub attaching: usize,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            phase: Phase::Connecting,
            connection: None,
            context: ContextUse::default(),
            harness: Harness::Research,
            sessions: Vec::new(),
            sidebar_open: true,
            run_panel_open: false,
            run_panel: RunPanel::default(),
            last_run: Run::new(),
            dark: true,
            stopping: false,
            screen: Screen::Chat,
            providers: ProvidersState::default(),
            turn_pages: Vec::new(),
            icons: HashMap::new(),
            attachments: Vec::new(),
            mention: None,
            editor: crate::editor::EditorState::default(),
            saved_answer: None,
            reply_to: None,
            copied: None,
            attaching: 0,
        }
    }

    pub fn connected(&mut self, connection: Connection) {
        self.context = ContextUse {
            window: connection.context_window,
            ..ContextUse::default()
        };
        self.connection = Some(connection);
        self.phase = Phase::Ready;
    }

    pub fn connect_failed(&mut self, reason: String) {
        self.phase = Phase::Offline(reason);
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Working { .. } | Phase::Approval { .. })
    }

    /// Whether the composer takes typing (a chat is connected and idle).
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    pub fn can_send(&self) -> bool {
        matches!(self.phase, Phase::Ready) && self.attaching == 0
    }

    /// Starts a turn. Returns the message to send (the text plus any
    /// attachments), or `None` when the window cannot send now or there is
    /// nothing to send.
    pub fn start_turn(&mut self, text: &str) -> Option<String> {
        let mut text = text.trim();
        if !self.can_send() {
            return None;
        }
        if text.is_empty() {
            if self.attachments.is_empty() {
                return None;
            }
            text = "Read the attached files.";
        }
        let mut attachments = std::mem::take(&mut self.attachments);
        mention::apply(text, &mut attachments);
        self.mention = None;
        let chips = attachments
            .iter()
            .map(|a| Chip {
                name: a.name.clone(),
                role: a.role,
            })
            .collect();
        let quote = self.reply_to.take();
        let message = attach::compose(&reply::compose(quote.as_deref(), text), &attachments);
        self.entries.push(Entry::User {
            text: text.to_owned(),
            chips,
            quote,
        });
        self.turn_pages.clear();
        self.run_panel.pinned = None;
        self.run_panel.open.clear();
        self.phase = Phase::Working {
            since: Instant::now(),
            activity: Activity::Thinking,
            run: Run::new(),
            draft: String::new(),
            draft_call: 0,
        };
        self.stopping = false;
        Some(message)
    }

    /// The text of a message or answer, for Copy.
    pub fn entry_text(&self, i: usize) -> Option<&str> {
        match self.entries.get(i)? {
            Entry::User { text, .. } | Entry::Assistant { text, .. } => Some(text),
            Entry::Notice(_) => None,
        }
    }

    /// The question, answer, and sources of answer `i`, to save as a note.
    pub fn answer_for_note(&self, i: usize) -> Option<AnswerNote> {
        let Entry::Assistant { text, pages, .. } = self.entries.get(i)? else {
            return None;
        };
        let question = self.entries[..i]
            .iter()
            .rev()
            .find_map(|e| match e {
                Entry::User { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "Answer".into());
        let sources = pages
            .iter()
            .map(|p| (p.label().to_owned(), p.url.clone()))
            .collect();
        Some((question, text.clone(), sources))
    }

    /// Starts a reply to a message or answer.
    pub fn reply_to_entry(&mut self, i: usize) {
        if let Some(t) = self.entry_text(i) {
            self.reply_to = Some(reply::excerpt(t));
        }
    }

    /// How many turns a branch from entry `i` keeps: every turn up to and
    /// including the one `i` belongs to.
    pub fn turns_through(&self, i: usize) -> usize {
        self.entries
            .iter()
            .take(i + 1)
            .filter(|e| matches!(e, Entry::User { .. }))
            .count()
    }

    /// The composer changed: opens, updates, or closes the `@` menu.
    pub fn on_composer_text(&mut self, text: &str) {
        let Some(a) = mention::active(text) else {
            self.mention = None;
            return;
        };
        let selected = match &self.mention {
            Some(m) if m.start == a.start && m.query == a.query => m.selected,
            _ => 0,
        };
        self.mention = Some(MentionMenu {
            start: a.start,
            query: a.query.to_owned(),
            role: a.role,
            selected,
        });
        let n = self.mention_candidates().len();
        if let Some(m) = &mut self.mention {
            m.selected = m.selected.min(n.saturating_sub(1));
        }
    }

    /// Sources read earlier in this chat, newest first, without repeats.
    pub fn chat_sources(&self) -> Vec<PageRef> {
        let mut out: Vec<PageRef> = Vec::new();
        for e in self.entries.iter().rev() {
            if let Entry::Assistant { pages, .. } = e {
                for p in pages {
                    if !out.iter().any(|q| q.url == p.url) {
                        out.push(p.clone());
                    }
                }
            }
        }
        out
    }

    /// The `@` menu's rows: matching attachments, then sources from this
    /// chat, then a row that opens the file picker.
    pub fn mention_candidates(&self) -> Vec<Candidate> {
        let Some(m) = &self.mention else {
            return Vec::new();
        };
        let q = m.query.as_str();
        let mut rows: Vec<(u8, Candidate)> = Vec::new();
        for (i, a) in self.attachments.iter().enumerate() {
            if mention::matches(q, &a.name) {
                rows.push((
                    mention::rank(q, &a.name),
                    Candidate {
                        pick: Pick::Attachment(i),
                        label: a.name.clone(),
                        detail: format!("{} · {}", a.role.label(), a.detail()),
                    },
                ));
            }
        }
        for p in self.chat_sources() {
            let label = p.label().to_owned();
            let attached = self
                .attachments
                .iter()
                .any(|a| a.source == attach::Source::Link && a.files[0].path == p.url);
            if !attached && (mention::matches(q, &label) || mention::matches(q, &p.site)) {
                rows.push((
                    mention::rank(q, &label).min(mention::rank(q, &p.site)) + 4,
                    Candidate {
                        pick: Pick::Source(p.clone()),
                        detail: p.site.clone(),
                        label,
                    },
                ));
            }
        }
        rows.sort_by_key(|(r, _)| *r);
        let mut out: Vec<Candidate> = rows
            .into_iter()
            .map(|(_, c)| c)
            .take(MENU_ROWS - 1)
            .collect();
        out.push(Candidate {
            pick: Pick::Browse,
            label: "Attach a file or folder…".into(),
            detail: "Files, folders, and .zip".into(),
        });
        out
    }

    /// Moves the highlight up (-1) or down (+1), wrapping.
    pub fn move_mention(&mut self, delta: i32) {
        let n = self.mention_candidates().len() as i32;
        if let Some(m) = &mut self.mention
            && n > 0
        {
            m.selected = (m.selected as i32 + delta).rem_euclid(n) as usize;
        }
    }

    /// Picks a row (the highlighted one when `index` is `None`) and returns
    /// the composer's new text.
    pub fn choose_mention(&mut self, text: &str, index: Option<usize>) -> Option<Chosen> {
        let m = self.mention.clone()?;
        let rows = self.mention_candidates();
        let row = rows.get(index.unwrap_or(m.selected))?.clone();
        self.mention = None;
        let name = match row.pick {
            Pick::Browse => {
                return Some(Chosen {
                    text: mention::clear_active(text),
                    browse: true,
                });
            }
            Pick::Attachment(i) => {
                let a = self.attachments.get_mut(i)?;
                if let Some(role) = m.role {
                    a.role = role;
                }
                a.name.clone()
            }
            Pick::Source(page) => {
                let mut a = mention::source_attachment(&page);
                if let Some(role) = m.role {
                    a.role = role;
                }
                let name = a.name.clone();
                self.attachments.push(a);
                name
            }
        };
        Some(Chosen {
            text: mention::complete(text, &name),
            browse: false,
        })
    }

    /// Text over the long-paste limit becomes a `Pasted text` attachment.
    /// Returns true when it did, so the composer can be cleared.
    pub fn take_long_text(&mut self, text: &str) -> bool {
        if text.chars().count() <= attach::LONG_PASTE_CHARS {
            return false;
        }
        let mut a = Attachment::paste(text);
        let n = self
            .attachments
            .iter()
            .filter(|x| x.name.starts_with("Pasted text"))
            .count();
        if n > 0 {
            a.name = format!("Pasted text {}", n + 1);
        }
        self.attachments.push(a);
        true
    }

    /// A file, folder, or archive finished reading.
    pub fn attached(&mut self, result: Result<Attachment, String>) {
        self.attaching = self.attaching.saturating_sub(1);
        match result {
            Ok(a) => self.attachments.push(a),
            Err(e) => self.entries.push(Entry::Notice(e)),
        }
    }

    pub fn remove_attachment(&mut self, i: usize) {
        if i < self.attachments.len() {
            self.attachments.remove(i);
        }
    }

    /// Switches an attachment between Context and Prompt.
    pub fn toggle_role(&mut self, i: usize) {
        if let Some(a) = self.attachments.get_mut(i) {
            a.role = a.role.toggled();
        }
    }

    /// An event from the running turn.
    pub fn on_event(&mut self, e: &EventEnvelope) {
        if let Some(p) = page_of(&e.kind, &e.payload)
            && self.is_busy()
        {
            push_unique(&mut self.turn_pages, p);
        }
        let next = match e.kind.as_str() {
            "turn.started" | "tool.executed" | "tool.failed" | "tool.rejected"
            | "model.replied" | "approval.answered" => Some(Activity::Thinking),
            "tool.started" => Some(match e.payload["tool"].as_str() {
                Some("web_search") => Activity::Searching,
                Some("read_url") => Activity::Reading,
                _ => Activity::Tool,
            }),
            _ => None,
        };
        match &mut self.phase {
            Phase::Working { activity, run, .. } => {
                if let Some(next) = next {
                    *activity = next;
                }
                run.push(e);
            }
            Phase::Approval { run, .. } => run.push(e),
            _ => {}
        }
    }

    /// Reply text from the stream. A new call number starts the draft over,
    /// since the previous reply was a tool request, not the answer.
    pub fn on_text(&mut self, call: u32, text: &str) {
        if let Phase::Working {
            draft,
            draft_call,
            activity,
            ..
        } = &mut self.phase
        {
            if *draft_call != call {
                draft.clear();
                *draft_call = call;
            }
            draft.push_str(text);
            if draft_visible(draft).is_some() {
                *activity = Activity::Writing;
            }
        }
    }

    /// The streaming answer to show, without any tool request it turns into.
    pub fn draft(&self) -> Option<&str> {
        match &self.phase {
            Phase::Working { draft, .. } => draft_visible(draft),
            _ => None,
        }
    }

    pub fn ask_approval(&mut self, request: ApprovalRequest) {
        let (since, run) = match std::mem::replace(&mut self.phase, Phase::Ready) {
            Phase::Working { since, run, .. } | Phase::Approval { since, run, .. } => (since, run),
            _ => (Instant::now(), Run::new()),
        };
        self.phase = Phase::Approval {
            since,
            run,
            request,
        };
    }

    pub fn approval_answered(&mut self) {
        if let Phase::Approval { since, run, .. } = std::mem::replace(&mut self.phase, Phase::Ready)
        {
            self.phase = Phase::Working {
                since,
                activity: Activity::Thinking,
                run,
                draft: String::new(),
                draft_call: 0,
            };
        }
    }

    pub fn request_stop(&mut self) {
        if self.is_busy() {
            self.stopping = true;
        }
    }

    pub fn finish_turn(&mut self, out: &TurnOutcome, context: ContextUse) {
        let (took, run) = match std::mem::replace(&mut self.phase, Phase::Ready) {
            Phase::Working { since, run, .. } | Phase::Approval { since, run, .. } => {
                (since.elapsed(), run)
            }
            _ => (Duration::ZERO, Run::new()),
        };
        self.last_run = run.clone();
        self.context = context;
        self.stopping = false;
        match &out.answer {
            Some(a) if !a.is_empty() => self.entries.push(Entry::Assistant {
                text: a.clone(),
                run,
                took,
                pages: std::mem::take(&mut self.turn_pages),
            }),
            _ => {}
        }
        if out.state != TurnState::Completed
            && let Some(r) = &out.reason
        {
            self.entries.push(Entry::Notice(r.clone()));
        }
    }

    /// Switching harness. Study is not built yet, so the switch explains that
    /// and leaves the chat in Research.
    pub fn choose_harness(&mut self, harness: Harness) {
        match harness {
            Harness::Research => self.harness = Harness::Research,
            Harness::Study => self.entries.push(Entry::Notice(
                "The Study harness (tutor, flashcards, quizzes, viva practice) arrives in phase 7. This chat stays in Research."
                    .into(),
            )),
        }
    }

    /// Fills the transcript from a reopened chat's past turns.
    pub fn restore(&mut self, turns: &[PastTurn]) {
        self.entries.clear();
        self.run_panel.pinned = None;
        self.run_panel.open.clear();
        self.last_run = turns.last().map(|t| t.run.clone()).unwrap_or_default();
        let mut missing = 0;
        for t in turns {
            match &t.message {
                Some(m) => {
                    let (text, chips) = attach::split(m);
                    let (quote, text) = reply::split(text);
                    self.entries.push(Entry::User {
                        text: text.to_owned(),
                        chips,
                        quote: quote.map(str::to_owned),
                    });
                }
                None => missing += 1,
            }
            if let Some(a) = &t.answer {
                self.entries.push(Entry::Assistant {
                    text: a.clone(),
                    run: t.run.clone(),
                    took: t.took,
                    pages: t.pages.clone(),
                });
            }
            if let Some(why) = &t.stop {
                self.entries.push(Entry::Notice(why.clone()));
            }
        }
        if missing > 0 {
            let note = format!(
                "{missing} earlier {} from before Tern kept chat text, so {} not shown.",
                if missing == 1 { "turn is" } else { "turns are" },
                if missing == 1 { "it is" } else { "they are" },
            );
            self.entries.insert(0, Entry::Notice(note));
        }
    }

    /// Shows a new title in the list at once; the bridge saves it.
    pub fn rename(&mut self, id: tern_core::SessionId, title: &str) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.title = title.to_owned();
        }
    }

    /// Sites whose favicon is needed and not asked for yet, as
    /// `(site, favicon url)`. Marks them loading; sites with no favicon url
    /// are marked missing.
    pub fn wanted_icons(&mut self) -> Vec<(String, String)> {
        let mut want = Vec::new();
        let pages = self.entries.iter().flat_map(|e| match e {
            Entry::Assistant { pages, .. } => pages.as_slice(),
            _ => &[],
        });
        for p in pages.chain(self.turn_pages.iter()) {
            if self.icons.contains_key(&p.site) {
                continue;
            }
            match &p.favicon {
                Some(url) => {
                    self.icons.insert(p.site.clone(), SiteIcon::Loading);
                    want.push((p.site.clone(), url.clone()));
                }
                None => {
                    self.icons.insert(p.site.clone(), SiteIcon::Missing);
                }
            }
        }
        want
    }

    pub fn icon_loaded(&mut self, site: String, path: Option<PathBuf>) {
        let state = path.map_or(SiteIcon::Missing, SiteIcon::Ready);
        self.icons.insert(site, state);
    }

    /// Clears the transcript for a new chat. The caller opens a new session.
    pub fn new_chat(&mut self) {
        if self.is_busy() {
            return;
        }
        self.entries.clear();
        self.reply_to = None;
        self.copied = None;
        self.context = ContextUse {
            window: self.context.window,
            ..ContextUse::default()
        };
        if self.connection.is_some() {
            self.phase = Phase::Ready;
        } else {
            self.phase = Phase::Connecting;
        }
    }

    /// The working line while a turn runs: what it is doing, the phrase,
    /// whole seconds, and a beat counter driven by real events.
    pub fn working_line(&self) -> Option<WorkingLine> {
        let (since, run, activity, draft) = match &self.phase {
            Phase::Working {
                since,
                run,
                activity,
                draft,
                ..
            } => (since, run, *activity, draft.len()),
            Phase::Approval { since, run, .. } => (since, run, Activity::NeedsYou, 0),
            _ => return None,
        };
        let activity = if self.stopping {
            Activity::Stopping
        } else {
            activity
        };
        let started = |tool: &str| {
            run.steps
                .iter()
                .filter(|s| s.event == "tool.started" && s.payload["tool"] == tool)
                .count()
        };
        let phrase = match activity {
            Activity::Thinking => "Thinking".to_owned(),
            Activity::Searching => match started("web_search") {
                1 => "Searching · 1 query".to_owned(),
                n => format!("Searching · {n} queries"),
            },
            Activity::Reading => format!("Reading page {}", started("read_url").max(1)),
            Activity::Tool => "Using a tool".to_owned(),
            Activity::Writing => "Writing".to_owned(),
            Activity::NeedsYou => "Needs your approval".to_owned(),
            Activity::Stopping => "Stopping".to_owned(),
        };
        Some(WorkingLine {
            activity,
            phrase,
            seconds: since.elapsed().as_secs(),
            // Text beats every ~60 characters streamed.
            beat: run.steps.len() + draft / 60,
        })
    }

    /// The run the panel shows: the one in progress, else a pinned answer's,
    /// else the latest turn's.
    pub fn panel_run(&self) -> &Run {
        match &self.phase {
            Phase::Working { run, .. } | Phase::Approval { run, .. } => run,
            _ => self
                .run_panel
                .pinned
                .and_then(|i| match self.entries.get(i) {
                    Some(Entry::Assistant { run, .. }) => Some(run),
                    _ => None,
                })
                .unwrap_or(&self.last_run),
        }
    }

    /// Opens the panel on an answer's run.
    pub fn show_run_of(&mut self, entry: usize) {
        if matches!(self.entries.get(entry), Some(Entry::Assistant { .. })) {
            self.run_panel.pinned = Some(entry);
            self.run_panel.open.clear();
            self.run_panel_open = true;
        }
    }

    /// Whether the panel shows an older answer's run rather than the latest.
    pub fn panel_is_pinned(&self) -> bool {
        let latest = self
            .entries
            .iter()
            .rposition(|e| matches!(e, Entry::Assistant { .. }));
        !self.is_busy() && self.run_panel.pinned.is_some_and(|i| Some(i) != latest)
    }

    pub fn set_run_view(&mut self, view: RunView) {
        self.run_panel.view = view;
    }

    pub fn set_raw_filter(&mut self, filter: RawFilter) {
        self.run_panel.filter = filter;
        self.run_panel.open.clear();
    }

    /// Opens or closes one Raw row's payload.
    pub fn toggle_raw(&mut self, row: usize) {
        if !self.run_panel.open.remove(&row) {
            self.run_panel.open.insert(row);
        }
    }

    /// Context used, from 0.0 to 1.0, when the model's window is known.
    pub fn context_fraction(&self) -> Option<f32> {
        self.context.fraction()
    }

    /// "1.2k of 128k tokens", or just the count when the window is unknown.
    /// A leading "~" marks an estimate.
    pub fn context_label(&self) -> String {
        let c = &self.context;
        let used = format!(
            "{}{}",
            if c.estimated { "~" } else { "" },
            compact(c.used())
        );
        match c.window {
            Some(w) => format!("{used} of {} tokens", compact(w)),
            None => format!("{used} tokens"),
        }
    }

    /// How full the window is, as a level the ring is drawn in.
    pub fn context_level(&self) -> ContextLevel {
        match self.context.fraction() {
            Some(f) if f >= 0.9 => ContextLevel::Full,
            Some(f) if f >= 0.7 => ContextLevel::Filling,
            _ => ContextLevel::Fine,
        }
    }

    /// The hover card's lines, as (label, value) with exact numbers.
    pub fn context_details(&self) -> Vec<(&'static str, String)> {
        let c = &self.context;
        let mut rows = Vec::new();
        let pct = c
            .fraction()
            .map(|f| format!(" ({:.1}%)", f * 100.0))
            .unwrap_or_default();
        let window = c
            .window
            .map(|w| format!(" of {}", grouped(w)))
            .unwrap_or_default();
        rows.push(("Context", format!("{}{window}{pct}", grouped(c.used()))));
        if c.calls > 0 {
            rows.push(("Last prompt", grouped(c.prompt)));
            rows.push(("Last reply", grouped(c.reply)));
        } else if c.used() > 0 {
            rows.push(("History", grouped(c.prompt)));
        }
        if let Some(left) = c.left() {
            rows.push(("Free", grouped(left)));
        }
        if c.calls > 0 {
            rows.push((
                "This chat",
                format!(
                    "{} tokens over {} call{}",
                    grouped(c.session_tokens),
                    c.calls,
                    if c.calls == 1 { "" } else { "s" }
                ),
            ));
        }
        rows.push((
            "Counted",
            if c.estimated {
                "estimated, 4 characters a token".into()
            } else {
                "by the provider".into()
            },
        ));
        rows
    }
}

/// How full the context window is, for the ring's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextLevel {
    Fine,
    Filling,
    Full,
}

/// 12,345 with thousands separators.
pub fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Events that show as a step under an answer.
/// The part of a streaming reply worth showing: everything before a tool
/// fence, minus a fence that is still arriving. `None` when nothing is left.
pub fn draft_visible(draft: &str) -> Option<&str> {
    const FENCE: &str = "```tool";
    let mut text = match draft.find(FENCE) {
        Some(i) => &draft[..i],
        None => draft,
    };
    for k in (1..FENCE.len()).rev() {
        if let Some(cut) = text.strip_suffix(&FENCE[..k]) {
            text = cut;
            break;
        }
    }
    let text = text.trim_end();
    (!text.trim_start().is_empty()).then_some(text)
}

/// 950 → "950", 1_234 → "1.2k", 128_000 → "128k", 1_000_000 → "1M".
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(n as f64 / 1_000.0, "k"),
        _ => trim(n as f64 / 1_000_000.0, "M"),
    }
}

fn trim(v: f64, unit: &str) -> String {
    if v >= 100.0 || (v - v.round()).abs() < 0.05 {
        format!("{}{unit}", v.round() as u64)
    } else {
        format!("{v:.1}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_agent::Role;
    use tern_core::{EffectClass, SessionId, TurnId};

    fn ev(kind: &str, summary: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new(
            SessionId::new(),
            kind,
            EffectClass::Analysis,
            summary,
            payload,
        )
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            text: text.into(),
            chips: Vec::new(),
            quote: None,
        }
    }

    fn read(p: PageRef) -> EventEnvelope {
        ev(
            "tool.executed",
            "Ran `read_url`",
            json!({"tool": "read_url", "args": {"url": p.url}, "meta": {"page": p}}),
        )
    }

    fn search(q: &str) -> EventEnvelope {
        ev(
            "tool.executed",
            "Ran `web_search`",
            json!({"tool": "web_search", "args": {"query": q}, "meta": {"results": 5}}),
        )
    }

    fn outcome(state: TurnState, answer: Option<&str>, reason: Option<&str>) -> TurnOutcome {
        TurnOutcome {
            turn_id: TurnId::new(),
            state,
            answer: answer.map(str::to_owned),
            reason: reason.map(str::to_owned),
            states: vec![],
            usage: tern_agent::Usage::measured(0, 0),
            model_calls: 1,
            tool_calls: 0,
        }
    }

    #[test]
    fn drafts_hide_tool_requests_and_restart_per_call() {
        let mut m = ready();
        m.start_turn("hi").unwrap();
        assert_eq!(m.draft(), None);
        m.on_text(1, "```to");
        assert_eq!(
            m.draft(),
            None,
            "a fence that is still arriving stays hidden"
        );
        m.on_text(1, "ol\n{\"name\":\"web_search\"}");
        assert_eq!(m.draft(), None);
        m.on_text(2, "Paris is the ");
        m.on_text(2, "capital.");
        assert_eq!(m.draft(), Some("Paris is the capital."));
        assert_eq!(m.working_line().unwrap().phrase, "Writing");
        m.on_text(3, "Let me check. `");
        assert_eq!(m.draft(), Some("Let me check."));
        assert_eq!(draft_visible("Answer\n```tool\n{}"), Some("Answer"));
        assert_eq!(draft_visible("   "), None);
    }

    fn page(site: &str, favicon: bool) -> PageRef {
        PageRef {
            title: format!("{site} page"),
            url: format!("https://{site}/p"),
            site: site.into(),
            builtin: None,
            favicon: favicon.then(|| format!("https://{site}/favicon.ico")),
        }
    }

    #[test]
    fn pages_follow_the_turn_and_icons_are_asked_once() {
        let mut m = ready();
        m.start_turn("q");
        m.on_event(&read(page("a.org", true)));
        m.on_event(&read(page("a.org", true)));
        m.on_event(&read(page("b.org", false)));
        assert_eq!(m.turn_pages.len(), 2);
        assert_eq!(
            m.wanted_icons(),
            [("a.org".to_owned(), "https://a.org/favicon.ico".to_owned())]
        );
        assert_eq!(m.icons["b.org"], SiteIcon::Missing);
        assert!(m.wanted_icons().is_empty(), "asked once");
        m.icon_loaded("a.org".into(), Some(PathBuf::from("/c/a.org")));
        assert_eq!(m.icons["a.org"], SiteIcon::Ready(PathBuf::from("/c/a.org")));
        m.finish_turn(&outcome(TurnState::Completed, Some("A"), None), ctx(1, 1));
        assert!(m.turn_pages.is_empty());
        match m.entries.last() {
            Some(Entry::Assistant { pages, .. }) => assert_eq!(pages.len(), 2),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn rename_updates_the_list_row() {
        let mut m = ready();
        let id = tern_core::SessionId::new();
        m.sessions.push(SessionSummary {
            id,
            title: "Chat".into(),
            harness: Harness::Research,
            created_at: String::new(),
            events: 3,
        });
        m.rename(id, "Attention paper");
        assert_eq!(m.sessions[0].title, "Attention paper");
        m.rename(tern_core::SessionId::new(), "Other");
        assert_eq!(m.sessions[0].title, "Attention paper");
    }

    #[test]
    fn restore_rebuilds_the_transcript() {
        let mut m = ready();
        m.entries.push(Entry::Notice("old".into()));
        let mut run = Run::new();
        run.push(&ev("model.replied", "replied", json!({})));
        run.push(&search("rag"));
        let turns = vec![
            PastTurn::default(),
            PastTurn {
                message: Some("What is RAG?".into()),
                answer: Some("Retrieval [1].".into()),
                run: run.clone(),
                took: Duration::from_secs(3),
                ..PastTurn::default()
            },
            PastTurn {
                message: Some("More".into()),
                stop: Some("You stopped the run.".into()),
                ..PastTurn::default()
            },
        ];
        m.restore(&turns);
        assert_eq!(
            m.entries,
            [
                Entry::Notice(
                    "1 earlier turn is from before Tern kept chat text, so it is not shown.".into()
                ),
                user("What is RAG?"),
                Entry::Assistant {
                    text: "Retrieval [1].".into(),
                    run: run.clone(),
                    took: Duration::from_secs(3),
                    pages: Vec::new(),
                },
                user("More"),
                Entry::Notice("You stopped the run.".into()),
            ]
        );
    }

    /// What the session reports after one measured call.
    fn ctx(prompt: u64, reply: u64) -> ContextUse {
        ContextUse {
            prompt,
            reply,
            window: Some(128_000),
            estimated: false,
            session_tokens: prompt + reply,
            calls: 1,
        }
    }

    #[test]
    fn context_details_are_exact() {
        let mut m = ready();
        assert_eq!(m.context_label(), "0 of 128k tokens");
        m.context = ctx(90_000, 2_345);
        assert_eq!(m.context_level(), ContextLevel::Filling);
        let d = m.context_details();
        assert_eq!(d[0], ("Context", "92,345 of 128,000 (72.1%)".to_owned()));
        assert_eq!(d[1], ("Last prompt", "90,000".to_owned()));
        assert_eq!(d[3], ("Free", "35,655".to_owned()));
        assert_eq!(d[4], ("This chat", "92,345 tokens over 1 call".to_owned()));
        m.context = ContextUse {
            prompt: 8,
            window: Some(128_000),
            estimated: true,
            ..ContextUse::default()
        };
        assert_eq!(m.context_label(), "~8 of 128k tokens");
        assert_eq!(m.context_details()[1], ("History", "8".to_owned()));
        m.context.prompt = 120_000;
        assert_eq!(m.context_level(), ContextLevel::Full);
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
    }

    fn ready() -> Model {
        let mut m = Model::new();
        m.connected(Connection {
            provider: "Groq".into(),
            model: "llama-3.3-70b".into(),
            search: Some("Tavily".into()),
            context_window: Some(128_000),
        });
        m
    }

    #[test]
    fn nothing_is_sent_while_connecting_or_empty() {
        let mut m = Model::new();
        assert_eq!(m.start_turn("hi"), None);
        m.connect_failed("No provider".into());
        assert_eq!(m.start_turn("hi"), None);
        let mut m = ready();
        assert_eq!(m.start_turn("   "), None);
        assert_eq!(m.start_turn("  hi \n"), Some("hi".into()));
        assert!(m.is_busy());
        assert_eq!(m.start_turn("again"), None, "one turn at a time");
    }

    #[test]
    fn steps_follow_the_run_and_stay_with_the_answer() {
        let mut m = ready();
        m.start_turn("What is RAG?");
        m.on_event(&ev("turn.started", "Turn started", json!({})));
        m.on_event(&search("rag"));
        m.on_event(&ev("policy.decided", "read_url allowed", json!({})));
        assert_eq!(m.panel_run().step_count(), 1);
        assert_eq!(m.working_line().unwrap().phrase, "Thinking");

        m.finish_turn(
            &outcome(TurnState::Completed, Some("RAG is…"), None),
            ctx(1000, 234),
        );
        assert!(!m.is_busy());
        match m.entries.last().unwrap() {
            Entry::Assistant { text, run, .. } => {
                assert_eq!(text, "RAG is…");
                assert_eq!(run.step_count(), 1);
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(m.panel_run().steps.len(), 3);
        assert_eq!(m.panel_run().summary().searches, 1);
        assert_eq!(m.context_label(), "1.2k of 128k tokens");
        assert!((m.context_fraction().unwrap() - 1234.0 / 128_000.0).abs() < 1e-6);
    }

    #[test]
    fn the_working_line_follows_real_events() {
        let mut m = ready();
        m.start_turn("q");
        m.on_event(&ev(
            "tool.started",
            "web_search",
            json!({"tool": "web_search"}),
        ));
        m.on_event(&ev(
            "tool.started",
            "web_search",
            json!({"tool": "web_search"}),
        ));
        let line = m.working_line().unwrap();
        assert_eq!(line.activity, Activity::Searching);
        assert_eq!(line.phrase, "Searching · 2 queries");
        m.on_event(&ev("tool.started", "read_url", json!({"tool": "read_url"})));
        assert_eq!(m.working_line().unwrap().phrase, "Reading page 1");
        assert!(m.working_line().unwrap().beat > line.beat, "events beat");
    }

    #[test]
    fn approvals_pause_and_resume_the_same_run() {
        let mut m = ready();
        m.start_turn("q");
        m.on_event(&search("one"));
        m.ask_approval(ApprovalRequest {
            tool: "read_url".into(),
            summary: "Read example.com".into(),
            effect: EffectClass::Network,
            target: Some("example.com".into()),
            args: json!({}),
        });
        assert!(matches!(m.phase, Phase::Approval { .. }));
        assert_eq!(m.working_line().unwrap().activity, Activity::NeedsYou);
        m.approval_answered();
        assert!(matches!(m.phase, Phase::Working { .. }));
        assert_eq!(m.panel_run().step_count(), 1);
    }

    #[test]
    fn stopped_turns_leave_a_notice() {
        let mut m = ready();
        m.start_turn("q");
        m.request_stop();
        assert_eq!(m.working_line().unwrap().phrase, "Stopping");
        m.finish_turn(
            &outcome(TurnState::Cancelled, None, Some("You stopped this turn.")),
            ctx(0, 0),
        );
        assert_eq!(
            m.entries.last(),
            Some(&Entry::Notice("You stopped this turn.".into()))
        );
        assert!(m.can_send());
    }

    #[test]
    fn study_explains_itself_and_new_chat_clears() {
        let mut m = ready();
        m.choose_harness(Harness::Study);
        assert_eq!(m.harness, Harness::Research);
        assert!(matches!(m.entries.last(), Some(Entry::Notice(t)) if t.contains("phase 7")));
        m.new_chat();
        assert!(m.entries.is_empty());
        assert!(m.can_send());
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(1_234), "1.2k");
        assert_eq!(compact(128_000), "128k");
        assert_eq!(compact(2_000), "2k");
        assert_eq!(compact(1_000_000), "1M");
    }

    #[test]
    fn the_panel_follows_the_latest_run_unless_pinned() {
        let mut m = ready();
        for q in ["first", "second"] {
            m.start_turn(q);
            m.on_event(&search(q));
            m.finish_turn(&outcome(TurnState::Completed, Some("A"), None), ctx(1, 1));
        }
        assert_eq!(m.panel_run().steps[0].target, "second");
        assert!(!m.panel_is_pinned());
        m.show_run_of(1);
        assert!(m.run_panel_open);
        assert!(m.panel_is_pinned());
        assert_eq!(m.panel_run().steps[0].target, "first");
        m.show_run_of(0);
        assert_eq!(m.run_panel.pinned, Some(1), "only answers can be pinned");
        m.start_turn("third");
        assert!(m.panel_run().is_empty(), "a new turn takes over");
        m.on_event(&ev("run.cancelled", "You stopped the turn", json!({})));
        m.finish_turn(
            &outcome(TurnState::Cancelled, None, Some("You stopped the turn")),
            ctx(1, 1),
        );
        assert_eq!(
            m.panel_run().problems().count(),
            1,
            "stopped runs stay visible"
        );
    }

    #[test]
    fn raw_rows_open_and_filters_reset_them() {
        let mut m = ready();
        m.toggle_raw(2);
        assert!(m.run_panel.open.contains(&2));
        m.toggle_raw(2);
        assert!(m.run_panel.open.is_empty());
        m.toggle_raw(1);
        m.set_raw_filter(RawFilter::Search);
        assert!(m.run_panel.open.is_empty());
        m.set_run_view(RunView::Raw);
        assert_eq!(m.run_panel.view, RunView::Raw);
    }

    #[test]
    fn reply_copy_and_branch_from_a_message() {
        let mut m = ready();
        m.start_turn("What is RAG?").unwrap();
        m.finish_turn(
            &outcome(
                TurnState::Completed,
                Some("Retrieval-augmented generation."),
                None,
            ),
            ctx(1, 1),
        );
        assert_eq!(m.entry_text(1), Some("Retrieval-augmented generation."));
        assert_eq!(m.turns_through(1), 1);
        m.reply_to_entry(1);
        let msg = m.start_turn("Who coined it?").unwrap();
        assert!(msg.starts_with("<in_reply_to>\nRetrieval-augmented generation.\n</in_reply_to>"));
        assert!(m.reply_to.is_none());
        assert!(matches!(
            &m.entries[2],
            Entry::User { text, quote: Some(q), .. } if text == "Who coined it?" && q.starts_with("Retrieval")
        ));
        assert_eq!(m.turns_through(2), 2);

        m.restore(&[PastTurn {
            message: Some(msg),
            ..PastTurn::default()
        }]);
        assert!(matches!(
            &m.entries[0],
            Entry::User { text, quote: Some(_), .. } if text == "Who coined it?"
        ));
    }

    #[test]
    fn the_at_menu_picks_attachments_sources_and_files() {
        let mut m = ready();
        let mut rubric = Attachment::paste("Grade on clarity.");
        rubric.name = "rubric.md".into();
        m.attachments.push(rubric);
        m.entries.push(Entry::Assistant {
            text: "a".into(),
            run: Run::new(),
            took: Duration::ZERO,
            pages: vec![PageRef {
                title: "RAG survey".into(),
                url: "https://arxiv.org/abs/2312.10997".into(),
                site: "arxiv.org".into(),
                builtin: None,
                favicon: None,
            }],
        });
        m.on_composer_text("mail me@x.com");
        assert!(m.mention.is_none(), "an email is not a mention");

        m.on_composer_text("Grade it with @prompt:rub");
        let rows = m.mention_candidates();
        assert_eq!(rows[0].label, "rubric.md");
        assert_eq!(rows.last().unwrap().pick, Pick::Browse);
        let chosen = m.choose_mention("Grade it with @prompt:rub", None).unwrap();
        assert_eq!(chosen.text, "Grade it with @prompt:rubric.md ");
        assert_eq!(m.attachments[0].role, Role::Prompt);
        assert!(m.mention.is_none());

        m.on_composer_text("compare with @arx");
        let rows = m.mention_candidates();
        assert!(matches!(rows[0].pick, Pick::Source(_)));
        let chosen = m.choose_mention("compare with @arx", Some(0)).unwrap();
        assert_eq!(chosen.text, "compare with @RAG-survey ");
        assert_eq!(m.attachments.len(), 2);
        assert_eq!(m.attachments[1].source, attach::Source::Link);
        m.on_composer_text("again @arx");
        assert!(
            !m.mention_candidates()
                .iter()
                .any(|c| matches!(c.pick, Pick::Source(_))),
            "an attached source is offered once"
        );

        m.on_composer_text("open @");
        let n = m.mention_candidates().len();
        m.move_mention(-1);
        assert_eq!(m.mention.as_ref().unwrap().selected, n - 1);
        let chosen = m.choose_mention("open @", None).unwrap();
        assert!(chosen.browse);
        assert_eq!(chosen.text, "open ");

        // Roles typed by hand apply on send.
        let msg = m.start_turn("Summarise with @context:rubric.md").unwrap();
        assert!(msg.contains(r#"name="rubric.md" role="context""#));
    }

    #[test]
    fn attachments_ride_with_the_next_message() {
        let mut m = ready();
        assert!(!m.take_long_text("short"));
        let long = "word ".repeat(1_000);
        assert!(m.take_long_text(&long));
        assert!(m.take_long_text(&long));
        assert_eq!(m.attachments[1].name, "Pasted text 2");
        m.toggle_role(1);
        assert_eq!(m.attachments[1].role, Role::Prompt);
        m.remove_attachment(0);
        m.attaching = 1;
        assert_eq!(m.start_turn("hi"), None, "waits for files still reading");
        m.attached(Err("Tern cannot read x.pdf yet.".into()));
        assert_eq!(m.attaching, 0);
        let msg = m.start_turn("").unwrap();
        assert!(msg.starts_with("Read the attached files."));
        assert!(msg.contains("role=\"prompt\""));
        assert!(m.attachments.is_empty());
        assert!(matches!(
            &m.entries[1],
            Entry::User { chips, .. } if chips.len() == 1 && chips[0].role == Role::Prompt
        ));
        assert_eq!(
            m.entries[0],
            Entry::Notice("Tern cannot read x.pdf yet.".into())
        );
    }

    #[test]
    fn reopened_messages_show_chips_not_the_block() {
        let mut m = ready();
        let msg = attach::compose("Summarise", &[Attachment::paste("notes")]);
        m.restore(&[PastTurn {
            message: Some(msg),
            ..PastTurn::default()
        }]);
        assert_eq!(
            m.entries[0],
            Entry::User {
                text: "Summarise".into(),
                chips: vec![Chip {
                    name: "Pasted text".into(),
                    role: Role::Context
                }],
                quote: None,
            }
        );
    }
}
