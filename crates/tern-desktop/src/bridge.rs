//! Runs chat turns on a tokio runtime and reports back over one channel.
//!
//! The window never calls a provider or the store itself: it hands a
//! [`ChatSession`] to the bridge, which gives it back with the outcome.

use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, CancelFlag, ChatOptions, ChatSession,
    SessionSummary, TurnOutcome, Usage,
};
use tokio::sync::{mpsc, oneshot};

/// Everything the background side tells the window.
pub enum Msg {
    Connected(Box<ChatSession>, Vec<SessionSummary>),
    ConnectFailed(String),
    Event { kind: String, summary: String },
    Approval(ApprovalRequest, oneshot::Sender<ApprovalAnswer>),
    Done(Box<ChatSession>, TurnOutcome, Usage, Vec<SessionSummary>),
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected(..) => f.write_str("Connected"),
            Self::ConnectFailed(e) => write!(f, "ConnectFailed({e})"),
            Self::Event { kind, .. } => write!(f, "Event({kind})"),
            Self::Approval(r, _) => write!(f, "Approval({})", r.tool),
            Self::Done(_, out, ..) => write!(f, "Done({:?})", out.state),
        }
    }
}

/// How many chats the sidebar lists.
pub const RECENT: usize = 30;

/// Sends approval requests to the window and waits for the button pressed.
struct UiApprover(mpsc::UnboundedSender<Msg>);

impl Approver for UiApprover {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            if self.0.send(Msg::Approval(req.clone(), tx)).is_err() {
                return ApprovalAnswer::Deny;
            }
            // A closed window counts as a refusal.
            rx.await.unwrap_or(ApprovalAnswer::Deny)
        })
    }
}

/// The tokio side of the app.
#[derive(Debug)]
pub struct Bridge {
    rt: tokio::runtime::Runtime,
    tx: mpsc::UnboundedSender<Msg>,
}

impl Bridge {
    pub fn new() -> std::io::Result<(Self, mpsc::UnboundedReceiver<Msg>)> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("tern-desktop-rt")
            .build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok((Self { rt, tx }, rx))
    }

    /// Opens a new chat session with the user's saved defaults.
    pub fn connect(&self) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            match ChatSession::open(ChatOptions::default()).await {
                Ok(s) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Connected(Box::new(s), recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnectFailed(e));
                }
            }
        });
    }

    /// Runs one turn and hands the session back when it ends.
    pub fn send(&self, mut session: Box<ChatSession>, text: String, cancel: CancelFlag) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let approver = UiApprover(tx.clone());
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event {
                    kind: e.kind.clone(),
                    summary: e.user_summary.clone(),
                });
            };
            let out = session.send(&text, &approver, cancel, &observer).await;
            let usage = session.usage();
            let recent = session.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Done(session, out, usage, recent));
        });
    }
}
