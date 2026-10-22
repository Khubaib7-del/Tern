//! Runs chat turns on a tokio runtime and reports back over one channel.
//!
//! The window never calls a provider or the store itself: it hands a
//! [`ChatSession`] to the bridge, which gives it back with the outcome.

use tern_agent::providers::{self as setup, ModelInfo, SecretStore};
use tern_agent::search;
use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, CancelFlag, ChatSession, ContextUse, PastTurn,
    Preferences, SessionSummary, TurnOutcome,
};
use tern_core::SecretString;
use tern_core::SessionId;
use tokio::sync::{mpsc, oneshot};

use crate::providers::{Kind, ServiceRow, rows_from};

/// Everything the background side tells the window.
pub enum Msg {
    Connected(Box<ChatSession>, Vec<SessionSummary>),
    /// A past chat reopened, with its turns for the transcript.
    Resumed(Box<ChatSession>, Vec<PastTurn>, Vec<SessionSummary>),
    /// A plain note for the transcript (a chat that would not reopen).
    Notice(String),
    /// A site's favicon on disk, or `None` when it could not be had.
    Icon(String, Option<std::path::PathBuf>),
    /// A file, folder, or archive read for the composer, or why not.
    Attached(Result<tern_agent::Attachment, String>),
    /// A page for the built-in browser: the load's number and the page.
    Browsed(u64, Result<tern_agent::browse::BrowsedPage, String>),
    /// A git operation finished: the repository as it now is, and what
    /// to tell the user.
    Git(crate::git::GitSnapshot, Result<Option<String>, String>),
    /// The chat list after archiving one.
    Sessions(Vec<SessionSummary>),
    ConnectFailed(String),
    /// An event from the running turn, as logged.
    Event(Box<tern_core::EventEnvelope>),
    /// Reply text streaming in: the model call's number and the new text.
    Text(u32, String),
    Approval(ApprovalRequest, oneshot::Sender<ApprovalAnswer>),
    Done(
        Box<ChatSession>,
        TurnOutcome,
        ContextUse,
        Vec<SessionSummary>,
    ),
    /// The Providers screen's rows, the saved choices, and a keychain error.
    Providers(Vec<ServiceRow>, Preferences, Option<String>),
    /// A key (new or saved) was tried against its service.
    Checked(Kind, &'static str, Result<String, String>),
    Removed(Kind, &'static str, Result<bool, String>),
    Models(&'static str, Result<Vec<ModelInfo>, String>),
    /// Saving the choices failed; the chat still reconnects with them.
    PrefsFailed(String),
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected(..) => f.write_str("Connected"),
            Self::Attached(r) => write!(f, "Attached(ok: {})", r.is_ok()),
            Self::Resumed(_, turns, _) => write!(f, "Resumed({} turns)", turns.len()),
            Self::Notice(e) => write!(f, "Notice({e})"),
            Self::Git(s, r) => write!(f, "Git(repo: {}, ok: {})", s.status.is_repo, r.is_ok()),
            Self::Browsed(r, p) => write!(f, "Browsed({r}, ok: {})", p.is_ok()),
            Self::Sessions(s) => write!(f, "Sessions({})", s.len()),
            Self::ConnectFailed(e) => write!(f, "ConnectFailed({e})"),
            Self::Event(e) => write!(f, "Event({})", e.kind),
            Self::Icon(site, path) => write!(f, "Icon({site}, {})", path.is_some()),
            Self::Text(call, t) => write!(f, "Text({call}, {} chars)", t.len()),
            Self::Approval(r, _) => write!(f, "Approval({})", r.tool),
            Self::Done(_, out, ..) => write!(f, "Done({:?})", out.state),
            Self::Providers(rows, ..) => write!(f, "Providers({})", rows.len()),
            Self::Checked(k, id, r) => write!(f, "Checked({k:?}, {id}, {})", r.is_ok()),
            Self::Removed(k, id, r) => write!(f, "Removed({k:?}, {id}, {})", r.is_ok()),
            Self::Models(id, r) => write!(f, "Models({id}, {})", r.is_ok()),
            Self::PrefsFailed(e) => write!(f, "PrefsFailed({e})"),
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

    /// Opens a new chat session with the user's saved choices.
    pub fn connect(&self) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            match ChatSession::open(prefs.chat_options()).await {
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

    /// Reopens a past chat with the saved provider choices.
    pub fn resume(&self, id: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            match ChatSession::resume(prefs.chat_options(), id).await {
                Ok((s, turns)) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Resumed(Box::new(s), turns, recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!("Tern could not reopen that chat: {e}")));
                }
            }
        });
    }

    /// Makes a branch of chat `from` keeping its first `turns` turns, then
    /// opens the branch.
    pub fn branch(&self, from: SessionId, turns: usize) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let made = tokio::task::spawn_blocking(move || {
                tern_agent::branch::branch_session(from, turns)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let id = match made {
                Ok(id) => id,
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!("Tern could not branch that chat: {e}")));
                    return;
                }
            };
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            match ChatSession::resume(prefs.chat_options(), id).await {
                Ok((s, turns)) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Resumed(Box::new(s), turns, recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!("Tern could not open the branch: {e}")));
                }
            }
        });
    }

    /// Saves a chat's new title, then sends the list again.
    pub fn rename(&self, id: SessionId, title: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = tern_agent::rename_session(id, &title) {
                let _ = tx.send(Msg::Notice(format!("Tern could not rename that chat: {e}")));
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Fetches a site's favicon into the icon cache (once per site), through
    /// the same network guard as page reads.
    pub fn favicon(&self, site: String, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let found = tern_agent::sources::cached_favicon(&site, &url).await;
            let _ = tx.send(Msg::Icon(site, found));
        });
    }

    /// Hides a chat from the list, then sends the list again.
    pub fn archive(&self, id: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = tern_agent::archive_session(id) {
                let _ = tx.send(Msg::Notice(format!(
                    "Tern could not archive that chat: {e}"
                )));
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Reads files, folders, or archives off the UI thread, one message each.
    pub fn attach(&self, paths: Vec<std::path::PathBuf>) {
        for path in paths {
            let tx = self.tx.clone();
            self.rt.spawn(async move {
                let result =
                    tokio::task::spawn_blocking(move || tern_agent::Attachment::load(&path))
                        .await
                        .unwrap_or_else(|e| Err(format!("Tern could not read that file: {e}")));
                let _ = tx.send(Msg::Attached(result));
            });
        }
    }

    /// Runs one turn and hands the session back when it ends.
    pub fn send(&self, mut session: Box<ChatSession>, text: String, cancel: CancelFlag) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let approver = UiApprover(tx.clone());
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event(Box::new(e.clone())));
            };
            let texts = tx.clone();
            let on_text = move |call: u32, t: &str| {
                let _ = texts.send(Msg::Text(call, t.to_owned()));
            };
            let out = session
                .send_streaming(&text, &approver, cancel, &observer, Some(&on_text))
                .await;
            let usage = session.context();
            let recent = session.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Done(session, out, usage, recent));
        });
    }

    /// Saves the choices, then opens a chat with them.
    pub fn save_and_connect(&self, prefs: Preferences) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let saved = {
                let p = prefs.clone();
                tokio::task::spawn_blocking(move || p.save())
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            };
            if let Err(e) = saved {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
            match ChatSession::open(prefs.chat_options()).await {
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

    /// Reads the catalog with key status and the saved choices.
    /// Runs a git operation in `dir`, then reads the repository again.
    pub fn git(&self, dir: std::path::PathBuf, op: crate::git::GitOp) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let repo = tern_agent::git::Repo::at(dir);
            let prefs = Preferences::load();
            let owner = prefs.git_name.clone().unwrap_or_default();
            let year = tern_agent::git::this_year();
            let result = crate::git::run(&repo, &op, &prefs.git_identity(), &owner, year);
            let snapshot = crate::git::GitSnapshot::read(&repo);
            let _ = tx.send(Msg::Git(snapshot, result));
        });
    }

    /// Saves the choices without reconnecting the chat.
    pub fn save_prefs(&self, prefs: Preferences) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = prefs.save() {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
        });
    }

    /// Reads a page for the built-in browser.
    pub fn browse(&self, request: u64, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let page = tern_agent::browse::open(&url).await;
            let _ = tx.send(Msg::Browsed(request, page));
        });
    }

    pub fn load_providers(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let rows = rows_from(setup::list(secrets), search::list(secrets));
            let _ = tx.send(Msg::Providers(rows, Preferences::load(), keychain.err()));
        });
    }

    /// Tests a new key and saves it to the keychain only if it works.
    pub fn add_key(&self, kind: Kind, id: &'static str, key: SecretString) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = match setup::keychain() {
                Err(e) => Err(format!("The keychain is not available: {e}")),
                Ok(store) => match kind {
                    Kind::Model => setup::add(id, key, store.as_ref(), None)
                        .await
                        .map(|r| model_report(r.chat_models, r.models, r.latency_ms))
                        .map_err(|e| e.to_string()),
                    Kind::Search => search::add(id, key, store.as_ref(), None)
                        .await
                        .map(|_| "Key works and is saved".to_owned()),
                },
            };
            let _ = tx.send(Msg::Checked(kind, id, result));
        });
    }

    /// Tries the saved key, or the local server.
    pub fn test(&self, kind: Kind, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let result = match kind {
                Kind::Model => setup::test(id, secrets, None)
                    .await
                    .map(|r| model_report(r.chat_models, r.models, r.latency_ms))
                    .map_err(|e| e.to_string()),
                Kind::Search => search::test(id, secrets, None)
                    .await
                    .map(|n| format!("Works, {n} result{}", if n == 1 { "" } else { "s" })),
            };
            let _ = tx.send(Msg::Checked(kind, id, result));
        });
    }

    pub fn remove(&self, kind: Kind, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = setup::keychain()
                .map_err(|e| format!("The keychain is not available: {e}"))
                .and_then(|store| match kind {
                    Kind::Model => setup::remove(id, store.as_ref()).map_err(|e| e.to_string()),
                    Kind::Search => search::remove(id, store.as_ref()),
                });
            let _ = tx.send(Msg::Removed(kind, id, result));
        });
    }

    /// Lists a provider's models for the picker.
    pub fn models(&self, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let result = setup::models(id, secrets, None)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Models(id, result));
        });
    }
}

/// "Works, 12 chat models, 180 ms".
fn model_report(chat: usize, all: usize, ms: u128) -> String {
    let n = if chat > 0 { chat } else { all };
    let what = if chat > 0 { "chat model" } else { "model" };
    format!(
        "Works, {n} {what}{}, {ms} ms",
        if n == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::model_report;

    #[test]
    fn reports_read_plainly() {
        assert_eq!(model_report(12, 40, 180), "Works, 12 chat models, 180 ms");
        assert_eq!(model_report(0, 1, 9), "Works, 1 model, 9 ms");
    }
}
