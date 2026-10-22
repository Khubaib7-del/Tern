//! The decision-model slot: `score(context, question, choices) -> probabilities`.
//!
//! A decision model does not write text. It is given a question and a fixed
//! set of choices and returns a probability for each, in a fraction of the
//! time and cost of asking the reasoning model. Tern asks it which pages to
//! read first, which tool fits the next step, whether a passage supports a
//! claim, and what to run now and what later.
//!
//! | Part | What it does |
//! | --- | --- |
//! | [`model`] | The contract: [`DecisionRequest`], [`Decision`], the [`DecisionModel`] trait |
//! | [`local`] | [`LocalModel`]: BM25 and heuristics, deterministic and offline |
//! | [`remote`] | BYOK adapters: TypeSafe Jev and SubQ Snap (System One wire), Voyage rerank |
//! | [`engine`] | [`Decider`]: mode, timeout and local fallback, hard checks ([`Rule`]), cache, log |
//! | [`calibrate`] | Softmax, confidence, Brier, log loss, calibration error |
//! | [`bench`] | The bundled fixture set and its scoring |
//!
//! Hard checks always override a score: a [`Rule`] rules a choice out or
//! forces one, whatever the model said. This crate holds no keys of its
//! own; `tern-agent` reads them from the keychain and hands them over.

#![forbid(unsafe_code)]

pub mod bench;
pub mod calibrate;
pub mod engine;
pub mod local;
pub mod model;
pub mod remote;
pub mod text;

pub use engine::{DEFAULT_TIMEOUT, Decider, Mode, Rule, apply_rules};
pub use local::{LOCAL_MODEL, LocalModel};
pub use model::{
    BoxFuture, Choice, Cost, Decision, DecisionError, DecisionKind, DecisionModel,
    DecisionRequest, Override,
};
pub use remote::{DECISION_CATALOG, RemoteModel, RemoteSpec, Wire, find_remote, price_of};
