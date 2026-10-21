//! The Tern engine.
//!
//! The turn loop, policy gate, workers, and steering are planned for roadmap
//! phases 3 and 5. So far it provides the health checks behind `tern doctor` and provider setup,
//! so front ends reach the platform and store only through this crate.

#![forbid(unsafe_code)]

pub mod doctor;
pub mod providers;
