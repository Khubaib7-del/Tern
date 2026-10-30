//! The Research harness: clarify, plan, search, read, verify, write, record.
//!
//! Built so far: the verify and record steps. [`claims`] finds the cited
//! sentences of an answer, [`verdict`] checks each against the text of the
//! sources it cites (offline by word overlap, or through a model judge the
//! engine runs), and [`report`] is the per-run record written as Markdown
//! and JSON. For deep research, [`plan`] holds the
//! typed research plan (drafted by the model or split without one, with
//! its budget divided among workers) and [`deep`] the workers' notes,
//! their merge into one numbering, and the synthesis's final citations. Nothing here
//! touches the network, the store, or a provider: the engine in
//! `tern-agent` feeds it text and runs the model calls. [`scholar`] reads the
//! scholarly services' answers into papers, ranks them through the rerank
//! funnel (with "not enough evidence" when nothing supports the question),
//! cuts PDFs into passages with page and section locators, and decides
//! when citation snowballing stops. [`evidence`] is the evidence chain:
//! quotes Tern minted from what it read, citations bound to them, each
//! source's stance on each claim (Supports, Refutes, Mixed, Not enough
//! info), the agreement across sources, and counter-search planning.

#![forbid(unsafe_code)]

pub mod claims;
pub mod deep;
pub mod evidence;
pub mod ledger;
pub mod plan;
pub mod report;
pub mod scholar;
pub mod steer;
pub mod verdict;
pub mod watch;

pub use claims::{Claim, extract};
pub use deep::{Final, Merged, NoteSource, WorkerNotes, WorkerStatus};
pub use evidence::{Agreement, Chain, CiteStatus, ClaimEvidence, QuoteBook, QuoteId, Stance};
pub use ledger::{Ledger, LedgerReason, SourceFate};
pub use plan::{PlanBudget, PlanSource, ResearchPlan, SourceKind, SubQuestion, WorkerBudget};
pub use report::{ReportBudget, ReportSource, ReportStep, ReportUsage, ReportWorker, RunReport};
pub use steer::{SteerAction, parse_steer};
pub use verdict::{ClaimCheck, Evidence, Method, SourceText, Tally, Verdict, check_lexical};
pub use watch::{WhatChanged, diff};
