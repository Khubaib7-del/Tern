//! The interactive TUI: `tern` or `tern chat`.
//!
//! The terminal loop runs on the main thread; each turn runs on the tokio
//! runtime and reports back over a channel (events, approvals, the outcome).
//! The UI never calls a provider or the store; it only drives a
//! [`ChatSession`] from `tern-agent`.

pub mod app;
pub mod view;

use std::io;
use std::time::Duration;

use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use ratatui::crossterm::execute;
use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, CancelFlag, ChatOptions, ChatSession, TurnOutcome,
    Usage,
};
use tokio::sync::{mpsc, oneshot};

use app::{Action, App};

enum Msg {
    Event { kind: String, summary: String },
    Approval(ApprovalRequest, oneshot::Sender<ApprovalAnswer>),
    Done(Box<ChatSession>, TurnOutcome, Usage),
}

/// Sends approval requests to the UI and waits for the key the user presses.
struct UiApprover(mpsc::UnboundedSender<Msg>);

impl Approver for UiApprover {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            if self.0.send(Msg::Approval(req.clone(), tx)).is_err() {
                return ApprovalAnswer::Deny;
            }
            // A closed UI counts as a refusal.
            rx.await.unwrap_or(ApprovalAnswer::Deny)
        })
    }
}

/// Opens the session, then runs the TUI until the user quits.
pub fn run(rt: &tokio::runtime::Runtime, opts: ChatOptions) -> Result<(), String> {
    eprintln!("Connecting…");
    let session = rt.block_on(ChatSession::open(opts))?;
    let mut app = App::new(session.provider_name(), session.model());
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = event_loop(rt, &mut terminal, &mut app, session);
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn event_loop(
    rt: &tokio::runtime::Runtime,
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    session: ChatSession,
) -> Result<(), String> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut session = Some(session);
    let mut cancel = CancelFlag::default();
    let mut pending: Option<oneshot::Sender<ApprovalAnswer>> = None;

    loop {
        terminal
            .draw(|f| view::draw(f, app))
            .map_err(|e| format!("Could not draw the screen: {e}"))?;

        // Animate only while something runs; otherwise wait for input.
        let wait = if app.is_busy() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };
        if event::poll(wait).map_err(|e| e.to_string())? {
            let action = match event::read().map_err(|e| e.to_string())? {
                Event::Key(k) => app.on_key(k),
                Event::Paste(p) => {
                    app.on_paste(&p);
                    Action::None
                }
                _ => Action::None,
            };
            match action {
                Action::None => {}
                Action::Quit => {
                    cancel.cancel();
                    if let Some(p) = pending.take() {
                        let _ = p.send(ApprovalAnswer::Deny);
                    }
                    return Ok(());
                }
                Action::Cancel => {
                    cancel.cancel();
                    if let Some(p) = pending.take() {
                        let _ = p.send(ApprovalAnswer::Deny);
                        app.approval_answered();
                    }
                }
                Action::Approve(answer) => {
                    if let Some(p) = pending.take() {
                        let _ = p.send(answer);
                    }
                    app.approval_answered();
                }
                Action::Send(text) => {
                    if let Some(mut s) = session.take() {
                        app.start_turn(text.clone());
                        cancel = CancelFlag::default();
                        let flag = cancel.clone();
                        let tx = tx.clone();
                        rt.spawn(async move {
                            let approver = UiApprover(tx.clone());
                            let events = tx.clone();
                            let observer = move |e: &tern_core::EventEnvelope| {
                                let _ = events.send(Msg::Event {
                                    kind: e.kind.clone(),
                                    summary: e.user_summary.clone(),
                                });
                            };
                            let out = s.send(&text, &approver, flag, &observer).await;
                            let usage = s.usage();
                            let _ = tx.send(Msg::Done(Box::new(s), out, usage));
                        });
                    }
                }
            }
        } else if app.is_busy() {
            app.tick = app.tick.wrapping_add(1);
        }

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Event { kind, summary } => app.on_event(&kind, &summary),
                Msg::Approval(req, reply) => {
                    if cancel.is_cancelled() {
                        let _ = reply.send(ApprovalAnswer::Deny);
                    } else {
                        pending = Some(reply);
                        app.ask_approval(req);
                    }
                }
                Msg::Done(s, out, usage) => {
                    app.finish_turn(&out, usage);
                    session = Some(*s);
                }
            }
        }
    }
}
