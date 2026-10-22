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
//! Flashcards and Explain in 3D build on the same pieces: a topic's chunks
//! ([`library::StudyLibrary::topic`], [`material::Chunk`]) and its Leitner
//! box and due date ([`mastery::Mastery::due_at`]).

#![forbid(unsafe_code)]

pub mod grade;
pub mod library;
pub mod mastery;
pub mod material;
pub mod quiz;
pub mod text;
pub mod topics;
pub mod tutor;
pub mod viva;

pub use grade::{Grade, GradeMethod, Mark};
pub use library::{QuizSummary, StudyLibrary, VivaSummary};
pub use mastery::{Level, Mastery};
pub use material::{Anchor, Chunk, Material, MaterialKind, MaterialText, Source};
pub use quiz::{Difficulty, Question, QuestionKind, Quiz, QuizAttempt, QuizRecord, QuizSpec};
pub use topics::Topic;
pub use tutor::TutorMode;
pub use viva::{Rubric, Viva, VivaReport, VivaTurn};
