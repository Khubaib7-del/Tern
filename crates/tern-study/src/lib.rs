//! The Study harness: the pure part.
//!
//! Study sessions are grounded in the user's own materials: attached PDFs
//! and notes, vault notes, links, and research reports. This crate splits
//! them into chunks with source anchors (file and page), groups the chunks
//! into topics with learning objectives ([`topics`]), keeps a small,
//! explainable mastery model per topic ([`mastery`]), and holds the prompts,
//! lenient parsers, and deterministic fallbacks behind the tutor
//! ([`tutor`]), quizzes and their grading ([`quiz`], [`grade`]), and viva
//! practice ([`viva`]).
//!
//! It does no I/O and calls no model: `tern-agent` reads the files, talks to
//! the provider, and saves the [`library::StudyLibrary`] in the data
//! directory. Every model step has a deterministic fallback, so the harness
//! works offline and its tests need no network.
//!
//! Flashcards ([`cards`]) are written from the same passages, by the model
//! or deterministically, each with its source anchor, and scheduled with
//! SM-2 ([`srs`]); reviews nudge the topic's mastery. [`cards_io`] reads
//! and writes them as CSV and Anki-ready TSV. Explain in 3D
//! ([`explain3d`]) turns a concept into a typed isometric scene with
//! ordered, cited steps, and projects it for the desktop renderer and the
//! SVG snapshot.

#![forbid(unsafe_code)]

pub mod camera;
pub mod cards;
pub mod cards_io;
pub mod explain3d;
pub mod explain_answer;
pub mod grade;
pub mod labels;
pub mod library;
pub mod mastery;
pub mod material;
pub mod quiz;
pub mod srs;
pub mod text;
pub mod topics;
pub mod tutor;
pub mod viva;

pub use cards::{Card, CardKind, CardSpec, Deck, DeckStats, ReviewLog};
pub use explain3d::{Scene, Shape};
pub use grade::{Grade, GradeMethod, Mark};
pub use library::{QuizSummary, StudyLibrary, VivaSummary};
pub use mastery::{Level, Mastery};
pub use material::{Anchor, Chunk, Material, MaterialKind, MaterialText, Source};
pub use quiz::{Difficulty, Question, QuestionKind, Quiz, QuizAttempt, QuizRecord, QuizSpec};
pub use srs::{CardState, Rating, Sched};
pub use topics::Topic;
pub use tutor::TutorMode;
pub use viva::{Rubric, Viva, VivaReport, VivaTurn};
