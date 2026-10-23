//! What the desktop window shows, kept apart from GPUI so it can be tested
//! without a display. The view reads this; the bridge feeds it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tern_agent::attach::{self, Attachment, Chip, Role};
use tern_agent::mcp::ItemKind;
use tern_agent::sources::{page_of, push_unique};
use tern_agent::{
    ApprovalRequest, ContextUse, Harness, PageRef, PastTurn, RawFilter, ResearchPlan, Run,
    SessionSummary, TurnOutcome,
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
    /// The vault repository: checkpoints, branches, push, pull requests.
    Git,
    /// The palette, type, glyphs, and Scout, for review.
    Appearance,
    /// Questions on watch and what changed since their last run.
    Watches,
    /// The Study harness: materials, topics, tutor, quizzes, viva.
    Study,
    /// Decision model settings: off, local, or a remote provider.
    Decision,
    /// The Library of everything.
    Library,
    /// Skills, MCP servers, providers, and the effective instructions.
    Customize,
    /// Evaluation runs: scores per suite, failures, the diff against the
    /// run before.
    Evaluation,
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
        run: Box<Run>,
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
    /// A deep research plan waits for the user: run, edit, skip, or cancel.
    Plan {
        since: Instant,
        run: Run,
        plan: ResearchPlan,
        /// The plan card shows text fields for its sub-questions.
        editing: bool,
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
    /// Checking the answer's claims against their sources.
    Checking,
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
    /// A resource or template an MCP server offers.
    Resource(Box<tern_agent::mcp::MenuItem>),
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
    /// An MCP resource to read and attach.
    pub fetch: Option<(String, String)>,
}

/// What choosing a `/` row asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashChosen {
    /// Run one of Tern's commands; the composer is cleared.
    Command(crate::commands::TernCommand),
    /// Put this text in the composer.
    Text(String),
    /// Ask the server to fill in this prompt with these arguments.
    Prompt(String, String, std::collections::BTreeMap<String, String>),
    /// The argument form opened; the composer is cleared.
    Form,
}

const MENU_ROWS: usize = 8;

/// The run panel's three views of the same run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunView {
    #[default]
    Summary,
    Detailed,
    Raw,
    /// The run report: claim verdicts with their evidence, and the files.
    Report,
    /// Deep research: one row per worker.
    Workers,
    /// The source and budget ledgers.
    Ledger,
    /// What the decision model decided, and why.
    Decisions,
}

impl RunView {
    /// The views a run offers: Workers too for a deep research run.
    pub fn for_run(run: &Run) -> Vec<RunView> {
        let mut v = vec![Self::Summary];
        if run.deep.is_some() {
            v.push(Self::Workers);
        }
        v.extend([Self::Detailed, Self::Raw, Self::Report, Self::Ledger]);
        if !run.decisions.is_empty() {
            v.push(Self::Decisions);
        }
        v
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Workers => "Workers",
            Self::Summary => "Summary",
            Self::Detailed => "Detailed",
            Self::Raw => "Raw",
            Self::Report => "Report",
            Self::Ledger => "Ledger",
            Self::Decisions => "Decisions",
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
    /// The claim the Report view points at (from a badge in the answer).
    pub claim: Option<usize>,
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
    /// The Decision model settings page.
    pub decision: crate::decision::DecisionPage,
    /// The Evaluation page.
    pub eval: crate::evaluation::EvalPage,
    /// Pages read so far in the running turn.
    pub turn_pages: Vec<PageRef>,
    pub icons: HashMap<String, SiteIcon>,
    /// Attachments waiting in the composer, sent with the next message.
    pub attachments: Vec<Attachment>,
    /// Pictures from the attachments of the turn just started, taken by the
    /// window when it hands the message to the session.
    pub outgoing_images: Vec<tern_agent::ImageInput>,
    /// The `@` menu over the composer, while a reference is being typed.
    pub mention: Option<MentionMenu>,
    /// The `/` menu over the composer, while a command is being typed.
    pub slash: Option<crate::commands::SlashMenu>,
    /// The form a prompt's arguments or a template's parts are typed into.
    pub arg_form: Option<crate::commands::ArgForm>,
    /// The composer holds text an MCP server wrote.
    pub inserted: Option<crate::commands::InsertedBanner>,
    /// The prompts and resources the chat's MCP servers offer.
    pub mcp_offers: Vec<tern_agent::mcp::MenuItem>,
    /// The built-in browser pane.
    pub browser: crate::browser::BrowserState,
    /// The Git screen.
    pub git: crate::git::GitState,
    /// "Credit Tern as co-author" on the Git screen, before it is saved.
    pub git_credit: bool,
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
    /// The composer's Deep research mode: the next message is planned
    /// and researched by parallel workers.
    pub deep_mode: bool,
    /// The running turn's steering inbox: the composer sends to it.
    pub steering: Option<tern_agent::Steering>,
    /// This chat's run that stopped and can be resumed.
    pub resume_offer: Option<ResumeOffer>,
    /// The running turn's last model reply hit the output limit.
    pub cut_off: bool,
    /// The last answer was cut off: the composer offers Continue.
    pub continue_offer: bool,
    /// Questions on watch.
    pub watches: Vec<tern_agent::watch::Watch>,
    /// The watch the Watches screen shows.
    pub watch_selected: Option<String>,
    /// Watches running in the background now.
    pub watches_running: usize,
    /// The Study harness's screen state.
    pub study: crate::study::StudyState,
    /// Message IDs, range actions, the side chat, highlights, the Library.
    pub cont: crate::continuity::Continuity,
    /// The Customize page.
    pub customize: crate::extensions::CustomizeState,
    /// The sidebar's project folders.
    pub projects: crate::extensions::ProjectTree,
}

/// A stopped or failed run the chat can pick up again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeOffer {
    pub question: String,
    /// "Run failed: …", "Stopped by you: …", "This run was interrupted: …".
    pub headline: String,
    /// "Deep research · 3 of 5 done · 2 failed", from the workers as they stand.
    pub progress: String,
    /// When it was saved, on the local clock ("Fri 23 Oct, 9:31 PM").
    pub saved_at: String,
    pub kind: tern_agent::OfferKind,
    /// The cause points at web search: the banner offers to fix it.
    pub about_search: bool,
}

impl ResumeOffer {
    /// The offer for a checkpoint, given how the run ended (`None` when the
    /// chat was reopened and the run was found interrupted). `None` when
    /// nothing can be picked up.
    pub fn of(
        ck: &tern_agent::RunCheckpoint,
        ended: Option<(TurnState, Option<&str>)>,
    ) -> Option<Self> {
        let o = ck.offer(ended)?;
        Some(Self {
            about_search: o.about_search(),
            question: o.question,
            headline: o.headline,
            progress: o.status,
            saved_at: tern_agent::checkpoint::local_label(
                &o.saved_at,
                tern_agent::checkpoint::local_offset(),
            ),
            kind: o.kind,
        })
    }

    pub fn action(&self) -> &'static str {
        match self.kind {
            tern_agent::OfferKind::Resume => "Resume",
            tern_agent::OfferKind::Retry => "Retry failed workers",
        }
    }
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
            decision: Default::default(),
            eval: Default::default(),
            turn_pages: Vec::new(),
            icons: HashMap::new(),
            attachments: Vec::new(),
            outgoing_images: Vec::new(),
            mention: None,
            slash: None,
            arg_form: None,
            inserted: None,
            mcp_offers: Vec::new(),
            browser: crate::browser::BrowserState::default(),
            git: crate::git::GitState::default(),
            git_credit: false,
            editor: crate::editor::EditorState::default(),
            saved_answer: None,
            reply_to: None,
            copied: None,
            attaching: 0,
            deep_mode: false,
            steering: None,
            resume_offer: None,
            cut_off: false,
            continue_offer: false,
            watches: Vec::new(),
            watch_selected: None,
            watches_running: 0,
            study: crate::study::StudyState::new(),
            cont: {
                let mut c = crate::continuity::Continuity::default();
                c.ids.reset();
                c
            },
            customize: Default::default(),
            projects: Default::default(),
        }
    }

    /// A plain line in the transcript.
    pub fn notice_line(&mut self, text: String) {
        self.entries.push(Entry::Notice(text));
    }

    /// Which user message (0 for the first) entry `i` is or answers.
    pub fn user_ordinal(&self, i: usize) -> Option<usize> {
        let upto = self.entries.get(..=i)?;
        let users = upto
            .iter()
            .filter(|e| matches!(e, Entry::User { .. }))
            .count();
        users.checked_sub(1)
    }

    /// The turn number entry `i` belongs to (its message or its answer).
    pub fn turn_of_entry(&self, i: usize) -> Option<u32> {
        self.cont.ids.numbers.get(self.user_ordinal(i)?).copied()
    }

    /// The ID shown on entry `i`'s turn (`Q3`, `Q12.4`).
    pub fn label_of_entry(&self, i: usize) -> Option<String> {
        self.cont.ids.label(self.user_ordinal(i)?)
    }

    /// Whether the composer sends a steering note rather than a message:
    /// a turn is running and has an inbox.
    pub fn can_steer(&self) -> bool {
        matches!(self.phase, Phase::Working { .. }) && self.steering.is_some() && !self.stopping
    }

    /// Sends a steering note to the running turn. It lands at the run's
    /// next safe point; the run panel shows what it did.
    pub fn steer(&mut self, text: &str) -> bool {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() || !self.can_steer() {
            return false;
        }
        let Some(inbox) = &self.steering else {
            return false;
        };
        if !inbox.send(&text) {
            return false;
        }
        let what = tern_agent::steer::parse_steer(&text).describe();
        self.entries.push(Entry::Notice(format!(
            "Steer sent: {what}. It lands at the run's next safe point."
        )));
        true
    }

    /// Sets the chat's resume banner from its checkpoint, if it has one
    /// and something in it can be picked up. `ended` is how the run just
    /// ended; `None` when the chat was reopened.
    pub fn set_resume_offer(
        &mut self,
        ck: Option<&tern_agent::RunCheckpoint>,
        ended: Option<(TurnState, Option<&str>)>,
    ) {
        self.resume_offer = ck.and_then(|c| ResumeOffer::of(c, ended));
    }

    /// Starts resuming the chat's stopped run. `false` when it cannot now.
    pub fn start_resume(&mut self) -> bool {
        if !self.can_send() || self.resume_offer.is_none() {
            return false;
        }
        self.resume_offer = None;
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
        true
    }

    /// The watch list after a load, an add, a run, or a removal.
    pub fn watches_loaded(&mut self, watches: Vec<tern_agent::watch::Watch>) {
        if self
            .watch_selected
            .as_ref()
            .is_some_and(|id| !watches.iter().any(|w| &w.id == id))
        {
            self.watch_selected = None;
        }
        self.watches = watches;
    }

    /// The question asked before answer `i`, to put on watch.
    pub fn question_before(&self, i: usize) -> Option<String> {
        self.entries[..i.min(self.entries.len())]
            .iter()
            .rev()
            .find_map(|e| match e {
                Entry::User { text, .. } => Some(text.clone()),
                _ => None,
            })
    }

    /// Whether `question` is already on watch.
    pub fn is_watched(&self, question: &str) -> bool {
        let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
        self.watches
            .iter()
            .any(|w| w.question.eq_ignore_ascii_case(&q))
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

    /// Scout's mood from the run's state (`docs/02-ui-definition.md` §7).
    pub fn scout_mood(&self) -> crate::scout::Mood {
        use crate::scout::Mood;
        match &self.phase {
            Phase::Offline(_) => Mood::Blocked,
            Phase::Approval { .. } | Phase::Plan { .. } => Mood::NeedsYou,
            Phase::Working { activity, .. } => match activity {
                Activity::Searching => Mood::Searching,
                Activity::Reading | Activity::Checking => Mood::Reading,
                Activity::NeedsYou => Mood::NeedsYou,
                Activity::Thinking | Activity::Tool | Activity::Writing | Activity::Stopping => {
                    Mood::Thinking
                }
            },
            Phase::Connecting | Phase::Ready
                if self.screen == Screen::Study
                    && self.study.view == crate::study::StudyView::Cards =>
            {
                Mood::Studying
            }
            Phase::Connecting | Phase::Ready => Mood::Idle,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Working { .. } | Phase::Approval { .. } | Phase::Plan { .. }
        )
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
        self.slash = None;
        self.inserted = None;
        self.cut_off = false;
        self.continue_offer = false;
        let chips = attachments
            .iter()
            .map(|a| Chip {
                name: a.name.clone(),
                role: a.role,
            })
            .collect();
        let quote = self.reply_to.take();
        self.outgoing_images = attach::images(&attachments);
        let message = attach::compose(&reply::compose(quote.as_deref(), text), &attachments);
        self.entries.push(Entry::User {
            text: text.to_owned(),
            chips,
            quote,
        });
        self.cont.ids.started();
        self.cont.highlight = None;
        self.turn_pages.clear();
        self.run_panel.pinned = None;
        self.run_panel.open.clear();
        if self.deep_mode {
            self.run_panel.view = RunView::Workers;
        } else if self.run_panel.view == RunView::Workers {
            self.run_panel.view = RunView::Summary;
        }
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

    /// The composer changed: opens, updates, or closes the `/` and `@`
    /// menus.
    pub fn on_composer_text(&mut self, text: &str) {
        match crate::commands::active(text) {
            Some(q) => {
                let selected = match &self.slash {
                    Some(m) if m.query == q => m.selected,
                    _ => 0,
                };
                self.slash = Some(crate::commands::SlashMenu {
                    query: q.to_owned(),
                    selected,
                });
                let n = self.slash_rows().len();
                if let Some(m) = &mut self.slash {
                    m.selected = m.selected.min(n.saturating_sub(1));
                }
            }
            None => self.slash = None,
        }
        if text.trim().is_empty() {
            self.inserted = None;
        }
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

    /// The skills that are on, as `(name, description)`, for the `/` menu.
    fn skill_rows(&self) -> Vec<(String, String)> {
        self.customize
            .skills
            .enabled()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect()
    }

    /// The `/` menu's rows.
    pub fn slash_rows(&self) -> Vec<crate::commands::SlashRow> {
        match &self.slash {
            Some(m) => crate::commands::rows(&m.query, &self.skill_rows(), &self.mcp_offers),
            None => Vec::new(),
        }
    }

    /// Moves the `/` highlight up (-1) or down (+1), wrapping.
    pub fn move_slash(&mut self, delta: i32) {
        let n = self.slash_rows().len() as i32;
        if let Some(m) = &mut self.slash
            && n > 0
        {
            m.selected = (m.selected as i32 + delta).rem_euclid(n) as usize;
        }
    }

    /// Picks a `/` row (the highlighted one when `index` is `None`).
    pub fn choose_slash(&mut self, index: Option<usize>) -> Option<SlashChosen> {
        use crate::commands::{ArgForm, FormFor, SlashPick};
        let m = self.slash.clone()?;
        let row = self.slash_rows().get(index.unwrap_or(m.selected))?.clone();
        self.slash = None;
        Some(match row.pick {
            SlashPick::Command(c) => SlashChosen::Command(c),
            SlashPick::Skill(name) => SlashChosen::Text(format!("Use the {name} skill. ")),
            SlashPick::Prompt(item) if item.args.is_empty() => {
                SlashChosen::Prompt(item.server, item.name, Default::default())
            }
            SlashPick::Prompt(item) => {
                self.arg_form = Some(ArgForm::new(*item, FormFor::Prompt));
                SlashChosen::Form
            }
        })
    }

    /// A prompt came back from its server: its text replaces the composer
    /// (the window does that) under a banner naming the server. Returns
    /// the text, or `None` after noting why it failed.
    pub fn prompt_ready(&mut self, r: Result<tern_agent::mcp::Inserted, String>) -> Option<String> {
        match r {
            Ok(ins) => {
                let logo = self
                    .mcp_offers
                    .iter()
                    .find(|i| i.server == ins.server)
                    .map(crate::commands::server_logo)
                    .unwrap_or(crate::commands::RowLogo::Server(
                        crate::extensions::ServerGlyph::Terminal,
                    ));
                let text = ins.text.clone();
                self.inserted = Some(crate::commands::InsertedBanner {
                    inserted: ins,
                    logo,
                });
                Some(text)
            }
            Err(e) => {
                self.notice_line(format!("Tern could not get that prompt: {e}"));
                None
            }
        }
    }

    /// A resource came back from its server: it joins the attachments as
    /// Context.
    pub fn resource_ready(&mut self, r: Result<Attachment, String>) {
        self.attaching = self.attaching.saturating_sub(1);
        match r {
            Ok(a) => {
                if !self.attachments.iter().any(|x| {
                    x.source == attach::Source::Mcp && x.files == a.files && x.name == a.name
                }) {
                    self.attachments.push(a);
                }
            }
            Err(e) => self.notice_line(format!("Tern could not read that resource: {e}")),
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
        for item in self
            .mcp_offers
            .iter()
            .filter(|i| i.kind != ItemKind::Prompt)
        {
            let label = item.label().to_owned();
            let attached = self
                .attachments
                .iter()
                .any(|a| a.source == attach::Source::Mcp && a.name == label);
            if !attached && (mention::matches(q, &label) || mention::matches(q, &item.name)) {
                let what = if item.kind == ItemKind::Template {
                    "template"
                } else {
                    "resource"
                };
                rows.push((
                    mention::rank(q, &label).min(mention::rank(q, &item.name)) + 2,
                    Candidate {
                        pick: Pick::Resource(Box::new(item.clone())),
                        detail: format!("{} · {what}", item.server),
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
                    fetch: None,
                });
            }
            Pick::Resource(item) if item.kind == ItemKind::Template => {
                self.arg_form = Some(crate::commands::ArgForm::new(
                    *item,
                    crate::commands::FormFor::Resource,
                ));
                return Some(Chosen {
                    text: mention::clear_active(text),
                    browse: false,
                    fetch: None,
                });
            }
            Pick::Resource(item) => {
                self.attaching += 1;
                return Some(Chosen {
                    text: mention::complete(text, item.label()),
                    browse: false,
                    fetch: Some((item.server.clone(), item.uri.clone())),
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
            fetch: None,
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
        // An MCP resource is the server's data: it stays Context.
        if let Some(a) = self.attachments.get_mut(i)
            && a.source != attach::Source::Mcp
        {
            a.role = a.role.toggled();
        }
    }

    /// An event from the running turn.
    pub fn on_event(&mut self, e: &EventEnvelope) {
        // The answer's own model call (not a plan or a worker's notes):
        // did it stop at the output limit?
        if e.kind == "model.replied"
            && !matches!(e.payload["purpose"].as_str(), Some("plan" | "notes"))
            && e.payload.get("worker").is_none()
        {
            self.cut_off = tern_agent::is_length_stop(e.payload["finish_reason"].as_str());
        }
        if let Some(p) = page_of(&e.kind, &e.payload)
            && self.is_busy()
        {
            push_unique(&mut self.turn_pages, p);
        }
        let next = match e.kind.as_str() {
            "turn.started" | "tool.executed" | "tool.failed" | "tool.rejected"
            | "model.replied" | "approval.answered" => Some(Activity::Thinking),
            "claims.checking" => Some(Activity::Checking),
            "worker.started" => Some(Activity::Searching),
            "worker.finished" | "plan.answered" => Some(Activity::Thinking),
            "tool.started" => Some(match e.payload["tool"].as_str() {
                Some("web_search") => Activity::Searching,
                Some("read_url") => Activity::Reading,
                _ => Activity::Tool,
            }),
            _ => None,
        };
        match &mut self.phase {
            Phase::Working { activity, run, .. } => {
                let next = match (e.kind.as_str(), e.payload.get("worker")) {
                    ("tool.executed", Some(_)) if e.payload["tool"] == "read_url" => {
                        Some(Activity::Reading)
                    }
                    ("tool.executed", Some(_)) => Some(Activity::Searching),
                    _ => next,
                };
                if let Some(next) = next {
                    *activity = next;
                }
                run.push(e);
            }
            Phase::Approval { run, .. } | Phase::Plan { run, .. } => run.push(e),
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

    /// A drafted plan to put in front of the user. The run panel opens on
    /// the workers view.
    pub fn ask_plan(&mut self, plan: ResearchPlan) {
        let (since, run) = match std::mem::replace(&mut self.phase, Phase::Ready) {
            Phase::Working { since, run, .. }
            | Phase::Approval { since, run, .. }
            | Phase::Plan { since, run, .. } => (since, run),
            _ => (Instant::now(), Run::new()),
        };
        self.phase = Phase::Plan {
            since,
            run,
            plan,
            editing: false,
        };
        self.run_panel.view = RunView::Workers;
        self.run_panel_open = true;
    }

    /// The plan card's Edit and Discard edits.
    pub fn set_plan_editing(&mut self, on: bool) {
        if let Phase::Plan { editing, .. } = &mut self.phase {
            *editing = on;
        }
    }

    /// The plan waiting for the user, if any.
    pub fn pending_plan(&self) -> Option<(&ResearchPlan, bool)> {
        match &self.phase {
            Phase::Plan { plan, editing, .. } => Some((plan, *editing)),
            _ => None,
        }
    }

    /// The plan was answered; the run goes on (or stops, if cancelled).
    pub fn plan_answered(&mut self) {
        if let Phase::Plan { since, run, .. } = std::mem::replace(&mut self.phase, Phase::Ready) {
            self.phase = Phase::Working {
                since,
                activity: Activity::Thinking,
                run,
                draft: String::new(),
                draft_call: 0,
            };
        }
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
            Phase::Working { since, run, .. }
            | Phase::Approval { since, run, .. }
            | Phase::Plan { since, run, .. } => (since.elapsed(), run),
            _ => (Duration::ZERO, Run::new()),
        };
        self.last_run = run.clone();
        self.context = context;
        self.stopping = false;
        self.steering = None;
        self.continue_offer = std::mem::take(&mut self.cut_off)
            && out.answer.as_deref().is_some_and(|a| !a.is_empty());
        match &out.answer {
            Some(a) if !a.is_empty() => self.entries.push(Entry::Assistant {
                text: a.clone(),
                run: Box::new(run),
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
        if self.continue_offer {
            self.entries.push(Entry::Notice(
                "Cut off at the length limit. Continue asks for the rest.".into(),
            ));
        }
    }

    /// Switching harness. Study opens the Study screen (materials, topics,
    /// tutor, quizzes, viva); Research goes back to the chat.
    /// A chat of `harness` was opened (from the list, or on switching):
    /// the window takes that harness, keeping the chat screen.
    pub fn harness_of_chat(&mut self, harness: Harness) {
        if self.harness != harness {
            self.harness = harness;
            if harness == Harness::Study {
                self.deep_mode = false;
            }
        }
    }

    pub fn choose_harness(&mut self, harness: Harness) {
        self.harness = harness;
        match harness {
            Harness::Research => {
                if self.screen == Screen::Study {
                    self.screen = Screen::Chat;
                }
            }
            Harness::Study => {
                self.screen = Screen::Study;
                self.deep_mode = false;
            }
        }
    }

    /// Fills the transcript from a reopened chat's past turns.
    pub fn restore(&mut self, turns: &[PastTurn]) {
        self.entries.clear();
        self.run_panel.pinned = None;
        self.run_panel.open.clear();
        self.cont.highlight = None;
        self.last_run = turns.last().map(|t| t.run.clone()).unwrap_or_default();
        self.cont.ids.restored(
            turns
                .iter()
                .filter(|t| t.message.is_some())
                .map(|t| t.number)
                .collect(),
        );
        let mut missing = 0;
        for t in turns {
            if let Some(c) = &t.context {
                self.entries.push(Entry::Notice(carried_note(c)));
            }
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
                    run: Box::new(t.run.clone()),
                    took: t.took,
                    pages: t.pages.clone(),
                });
            }
            if let Some(why) = &t.stop {
                self.entries.push(Entry::Notice(why.clone()));
            }
            for r in &t.reports {
                self.entries.push(Entry::Notice(report_note(r)));
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
        let browsed = self.browser.page.iter().map(|b| &b.page);
        // The ledger lists pages that were never read; their icons come
        // from the site's /favicon.ico.
        let ledger: Vec<PageRef> = if self.run_panel.view == RunView::Ledger {
            self.panel_run()
                .ledger()
                .sources
                .iter()
                .filter(|s| !s.site.is_empty())
                .map(|s| PageRef {
                    title: s.title.clone(),
                    url: s.url.clone(),
                    site: s.site.clone(),
                    builtin: None,
                    favicon: Some(format!("https://{}/favicon.ico", s.site)),
                })
                .collect()
        } else {
            Vec::new()
        };
        for p in pages
            .chain(self.turn_pages.iter())
            .chain(browsed)
            .chain(ledger.iter())
        {
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
        self.resume_offer = None;
        self.cont.ids.reset();
        self.cont.highlight = None;
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
            Phase::Approval { since, run, .. } | Phase::Plan { since, run, .. } => {
                (since, run, Activity::NeedsYou, 0)
            }
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
            Activity::Checking => {
                let n = run
                    .steps
                    .iter()
                    .rev()
                    .find(|s| s.event == "claims.checking")
                    .and_then(|s| s.payload["claims"].as_u64())
                    .unwrap_or(0);
                match n {
                    1 => "Checking 1 claim".to_owned(),
                    n => format!("Checking {n} claims"),
                }
            }
            Activity::NeedsYou if matches!(self.phase, Phase::Plan { .. }) => {
                "Review the research plan".to_owned()
            }
            Activity::NeedsYou => "Needs your approval".to_owned(),
            Activity::Stopping => "Stopping".to_owned(),
        };
        // A deep run's workers say how far along they are.
        let phrase = match (&run.deep, activity) {
            (Some(d), Activity::Thinking | Activity::Searching | Activity::Reading)
                if !d.workers.is_empty() && d.finished() < d.workers.len() =>
            {
                format!(
                    "Researching · {} running · {} of {} done",
                    d.running(),
                    d.finished(),
                    d.workers.len()
                )
            }
            _ => phrase,
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
            Phase::Working { run, .. } | Phase::Approval { run, .. } | Phase::Plan { run, .. } => {
                run
            }
            _ => self
                .run_panel
                .pinned
                .and_then(|i| match self.entries.get(i) {
                    Some(Entry::Assistant { run, .. }) => Some(&**run),
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
        if view != RunView::Report {
            self.run_panel.claim = None;
        }
    }

    /// Opens the panel's Report view on one claim of an answer (a badge
    /// in the answer was clicked).
    pub fn show_claim(&mut self, entry: usize, claim: usize) {
        if matches!(self.entries.get(entry), Some(Entry::Assistant { .. })) {
            self.show_run_of(entry);
            self.run_panel.view = RunView::Report;
            self.run_panel.claim = Some(claim);
            self.browser.visible = false;
        }
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
/// The transcript line for context carried into a chat.
pub fn carried_note(markdown: &str) -> String {
    let first = markdown
        .lines()
        .map(|l| l.trim_start_matches(['#', ' ']))
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Context carried over");
    format!("↳ {}", tern_agent::range::clip(first, 160))
}

/// The transcript line for a sub-session's report back.
pub fn report_note(r: &tern_agent::SubReport) -> String {
    format!(
        "↳ {} reported back · {}: {}",
        r.refs.last().map(String::as_str).unwrap_or("Sub-session"),
        tern_agent::range::clip(&r.question, 80),
        r.summary
    )
}

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
            parent: None,
            anchor: None,
            plan: None,
            project: None,
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
                    run: Box::new(run.clone()),
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
    fn deep_research_waits_for_the_plan_then_shows_workers() {
        use tern_agent::deep::{PlanBudget, fallback_plan};
        let mut m = ready();
        m.deep_mode = true;
        m.start_turn("Compare Rust vs Go");
        assert_eq!(m.run_panel.view, RunView::Workers);
        m.on_event(&ev(
            "turn.started",
            "Started a deep research turn",
            json!({"mode": "deep", "workers": 2}),
        ));
        let plan = fallback_plan("Compare Rust vs Go", PlanBudget::default(), 5);
        m.on_event(&ev("plan.drafted", "Planned", json!({"plan": plan})));
        m.ask_plan(plan.clone());
        assert!(m.is_busy() && m.run_panel_open);
        assert_eq!(m.working_line().unwrap().phrase, "Review the research plan");
        assert_eq!(
            m.pending_plan().map(|(p, e)| (p.sub_questions.len(), e)),
            Some((3, false))
        );
        m.set_plan_editing(true);
        assert!(m.pending_plan().unwrap().1);
        m.on_event(&ev(
            "plan.answered",
            "You approved the plan",
            json!({"answer": "run", "plan": plan}),
        ));
        m.plan_answered();
        assert!(matches!(m.phase, Phase::Working { .. }));
        m.on_event(&ev(
            "worker.started",
            "Worker 1 started",
            json!({"worker": 1}),
        ));
        let line = m.working_line().unwrap();
        assert_eq!(line.phrase, "Researching · 1 running · 0 of 3 done");
        m.on_event(&ev(
            "tool.executed",
            "Worker 1 read",
            json!({"tool": "read_url", "worker": 1, "meta": {"worker": 1, "page": {"title": "Go", "url": "https://go.dev", "site": "go.dev"}}}),
        ));
        assert_eq!(m.working_line().unwrap().activity, Activity::Reading);
        assert_eq!(m.turn_pages.len(), 1);
        let run = m.panel_run();
        assert_eq!(RunView::for_run(run)[1], RunView::Workers);
        assert_eq!(run.deep.as_ref().unwrap().workers[0].pages.len(), 1);
        assert_eq!(RunView::for_run(&Run::new()).len(), 5);
        // A normal turn after a deep one goes back to the summary.
        m.finish_turn(
            &outcome(TurnState::Completed, Some("A [1]."), None),
            ctx(0, 0),
        );
        m.deep_mode = false;
        m.start_turn("q");
        assert_eq!(m.run_panel.view, RunView::Summary);
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
    fn study_opens_its_screen_and_research_returns_to_chat() {
        let mut m = ready();
        m.choose_harness(Harness::Study);
        assert_eq!(m.harness, Harness::Study);
        assert_eq!(m.screen, Screen::Study);
        m.choose_harness(Harness::Research);
        assert_eq!(m.screen, Screen::Chat);
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
            run: Box::new(Run::new()),
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

    fn offer(kind: ItemKind, name: &str, uri: &str, args: &[&str]) -> tern_agent::mcp::MenuItem {
        tern_agent::mcp::MenuItem {
            server: "notes".into(),
            kind,
            name: name.into(),
            title: None,
            description: "From the notes server.".into(),
            uri: uri.into(),
            mime: None,
            args: args
                .iter()
                .map(|a| tern_agent::mcp::PromptArg {
                    name: (*a).into(),
                    title: None,
                    description: String::new(),
                    required: true,
                })
                .collect(),
            http: false,
            target: "notes-mcp".into(),
        }
    }

    #[test]
    fn slash_and_at_offer_mcp_prompts_and_resources() {
        use crate::commands::{FormFor, RowLogo};
        let mut m = ready();
        m.mcp_offers = vec![
            offer(ItemKind::Prompt, "recap", "", &[]),
            offer(ItemKind::Prompt, "review", "", &["code"]),
            offer(ItemKind::Resource, "week-1", "notes://week-1", &[]),
            offer(ItemKind::Template, "note", "notes://{name}", &["name"]),
        ];
        m.on_composer_text("/re");
        let rows = m.slash_rows();
        assert!(
            rows.iter()
                .all(|r| matches!(r.logo, RowLogo::Tern | RowLogo::Server(_)))
        );
        let i = rows.iter().position(|r| r.label == "/recap").unwrap();
        assert_eq!(
            m.choose_slash(Some(i)),
            Some(SlashChosen::Prompt(
                "notes".into(),
                "recap".into(),
                Default::default()
            ))
        );
        m.on_composer_text("/review");
        assert_eq!(m.choose_slash(None), Some(SlashChosen::Form));
        assert_eq!(m.arg_form.as_ref().unwrap().purpose, FormFor::Prompt);
        m.arg_form = None;
        m.on_composer_text("/new");
        assert_eq!(
            m.choose_slash(None),
            Some(SlashChosen::Command(crate::commands::TernCommand::New))
        );

        // A prompt's text goes in the composer with a banner; never sent.
        let text = m
            .prompt_ready(Ok(tern_agent::mcp::Inserted {
                server: "notes".into(),
                prompt: "recap".into(),
                text: "Recap the week".into(),
                description: None,
                flags: Vec::new(),
            }))
            .unwrap();
        assert_eq!(text, "Recap the week");
        assert!(m.inserted.as_ref().unwrap().line().contains("notes"));
        assert!(matches!(m.phase, Phase::Ready));

        // @ offers resources; picking one reads it.
        m.on_composer_text("see @week");
        let rows = m.mention_candidates();
        assert!(matches!(rows[0].pick, Pick::Resource(_)));
        let chosen = m.choose_mention("see @week", Some(0)).unwrap();
        assert_eq!(chosen.text, "see @week-1 ");
        assert_eq!(
            chosen.fetch,
            Some(("notes".into(), "notes://week-1".into()))
        );
        assert_eq!(m.attaching, 1);
        let parts = vec![tern_agent::mcp::ResourcePart::Text {
            uri: "notes://week-1".into(),
            mime: None,
            text: "Monday: RAG.".into(),
        }];
        m.resource_ready(Ok(tern_agent::mcp::catalog::attachment(
            "notes",
            "week-1",
            "notes://week-1",
            &parts,
        )));
        assert_eq!(m.attaching, 0);
        m.toggle_role(0);
        assert_eq!(
            m.attachments[0].role,
            Role::Context,
            "a resource stays Context"
        );
        m.on_composer_text("and @note");
        let chosen = m.choose_mention("and @note", Some(0)).unwrap();
        assert_eq!(chosen.fetch, None);
        assert_eq!(m.arg_form.as_ref().unwrap().purpose, FormFor::Resource);
        let msg = m.start_turn("Summarise @prompt:week-1").unwrap();
        assert!(msg.contains(r#"name="week-1" role="context""#));
        assert!(msg.contains("<untrusted-data source=\"mcp:notes/notes://week-1\">"));
        assert!(m.inserted.is_none());
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

    #[test]
    fn badges_open_their_claim_in_the_report() {
        let mut m = Model::new();
        m.entries.push(Entry::User {
            text: "q".into(),
            chips: vec![],
            quote: None,
        });
        m.entries.push(Entry::Assistant {
            text: "Water boils at 100 C [1].".into(),
            run: Box::new(Run::new()),
            took: Duration::ZERO,
            pages: vec![],
        });
        m.browser.visible = true;
        m.show_claim(1, 1);
        assert!(m.run_panel_open);
        assert!(!m.browser.visible, "the report takes the panel's place");
        assert_eq!(m.run_panel.view, RunView::Report);
        assert_eq!(m.run_panel.claim, Some(1));
        assert_eq!(m.run_panel.pinned, Some(1));
        m.set_run_view(RunView::Summary);
        assert_eq!(m.run_panel.claim, None);
        m.show_claim(0, 2);
        assert_eq!(
            m.run_panel.view,
            RunView::Summary,
            "only answers have claims"
        );
    }

    #[test]
    fn the_composer_steers_a_running_turn() {
        let mut m = ready();
        assert!(!m.can_steer());
        m.start_turn("Compare Rust vs Go");
        assert!(!m.can_steer(), "no inbox yet");
        let inbox = tern_agent::Steering::new();
        m.steering = Some(inbox.clone());
        assert!(m.can_steer());
        assert!(!m.steer("   "));
        assert!(m.steer("drop sub-question 3"));
        assert_eq!(inbox.take()[0].text, "drop sub-question 3");
        assert!(matches!(
            m.entries.last(),
            Some(Entry::Notice(n)) if n.starts_with("Steer sent: Drop sub-question 3")
        ));
        m.on_event(&ev(
            "steer.received",
            "You steered the run: drop sub-question 3",
            json!({"text": "drop sub-question 3"}),
        ));
        m.on_event(&ev(
            "steer.applied",
            "Applied: stopping worker 3",
            json!({"text": "drop sub-question 3", "applied": true}),
        ));
        assert_eq!(m.panel_run().steers[0].applied, Some(true));
        m.request_stop();
        assert!(!m.can_steer(), "a stopping run takes no more notes");
        m.finish_turn(
            &outcome(TurnState::Cancelled, None, Some("You stopped the run.")),
            ctx(0, 0),
        );
        assert!(m.steering.is_none());
    }

    #[test]
    fn a_failed_deep_run_offers_retry_with_an_honest_status() {
        use tern_agent::checkpoint::{DeepState, RunCheckpoint, RunMode, Stage, WorkerCheckpoint};
        use tern_agent::WorkerStatus;
        let mut m = ready();
        let ck = RunCheckpoint {
            session: "ses_1".into(),
            mode: RunMode::Deep,
            message: "What is gpu".into(),
            stage: Stage::Workers,
            saved_at: "2026-10-23T16:31:00Z".into(),
            deep: Some(DeepState {
                concurrency: 3,
                workers: (1..=5)
                    .map(|n| WorkerCheckpoint {
                        n,
                        finished: Some(WorkerStatus::Failed),
                        ..WorkerCheckpoint::default()
                    })
                    .collect(),
                ..DeepState::default()
            }),
            ..RunCheckpoint::default()
        };
        m.set_resume_offer(
            Some(&ck),
            Some((TurnState::Failed, Some("No search service answered: SearXNG did not answer"))),
        );
        let o = m.resume_offer.clone().unwrap();
        assert_eq!(o.action(), "Retry failed workers");
        assert!(o.headline.starts_with("Run failed: No search service answered"));
        assert_eq!(o.progress, "Deep research · 0 of 5 done · 5 failed");
        assert!(!o.progress.contains("running") && !o.saved_at.contains('T'));
        assert!(o.about_search);
        // A run with nothing left to do offers nothing.
        let mut done = ck.clone();
        for w in &mut done.deep.as_mut().unwrap().workers {
            w.finished = Some(WorkerStatus::Done);
            w.notes = "n".into();
        }
        m.set_resume_offer(Some(&done), Some((TurnState::Failed, Some("x"))));
        assert!(m.resume_offer.is_none());
    }

    #[test]
    fn an_answer_cut_off_at_the_limit_offers_continue() {
        let mut m = ready();
        assert!(m.start_turn("Explain GPUs at length").is_some());
        m.on_event(&ev(
            "model.replied",
            "replied",
            serde_json::json!({"finish_reason": "length", "model": "m", "chars": 9000}),
        ));
        m.finish_turn(
            &outcome(TurnState::Completed, Some("A GPU is … specific part ("), None),
            ctx(0, 0),
        );
        assert!(m.continue_offer);
        assert!(matches!(m.entries.last(), Some(Entry::Notice(n)) if n.contains("length limit")));
        assert!(m.start_turn(tern_agent::CONTINUE_PROMPT).is_some());
        assert!(!m.continue_offer, "sending clears it");
        m.on_event(&ev(
            "model.replied",
            "replied",
            serde_json::json!({"finish_reason": "stop"}),
        ));
        m.finish_turn(&outcome(TurnState::Completed, Some("the rest."), None), ctx(0, 0));
        assert!(!m.continue_offer);
    }

    #[test]
    fn a_stopped_run_offers_resume_once() {
        use tern_agent::checkpoint::{RunCheckpoint, RunMode, Stage};
        let mut m = ready();
        let ck = RunCheckpoint {
            session: "ses_1".into(),
            mode: RunMode::Quick,
            message: "What is a tern?".into(),
            stage: Stage::Started,
            saved_at: "2026-10-22T10:00:00Z".into(),
            ..RunCheckpoint::default()
        };
        m.set_resume_offer(Some(&ck), Some((TurnState::Cancelled, None)));
        let offer = m.resume_offer.clone().unwrap();
        assert_eq!(offer.question, "What is a tern?");
        assert_eq!(offer.headline, "Stopped by you: What is a tern?");
        assert_eq!(offer.action(), "Resume");
        assert_eq!(offer.progress, "Quick turn · 0 tool results");
        assert!(m.start_resume());
        assert!(m.is_busy() && m.resume_offer.is_none());
        assert!(!m.start_resume(), "only one resume at a time");
        m.finish_turn(
            &outcome(TurnState::Completed, Some("Seabirds."), None),
            ctx(0, 0),
        );
        m.set_resume_offer(Some(&ck), None);
        m.new_chat();
        assert!(m.resume_offer.is_none(), "a new chat has nothing to resume");
    }

    #[test]
    fn watches_load_and_keep_the_selection_honest() {
        use tern_agent::watch::WatchList;
        let mut m = ready();
        let mut list = WatchList::default();
        let w = list.add("EU AI rules", 24, false).unwrap();
        m.watch_selected = Some(w.id.clone());
        m.watches_loaded(list.watches.clone());
        assert_eq!(m.watch_selected.as_deref(), Some(w.id.as_str()));
        assert!(m.is_watched("  eu ai   rules "));
        m.watches_loaded(Vec::new());
        assert!(m.watch_selected.is_none());
        m.start_turn("EU AI rules");
        m.finish_turn(
            &outcome(TurnState::Completed, Some("They changed [1]."), None),
            ctx(0, 0),
        );
        assert_eq!(m.question_before(1).as_deref(), Some("EU AI rules"));
        assert_eq!(RunView::Ledger.label(), "Ledger");
    }
}
