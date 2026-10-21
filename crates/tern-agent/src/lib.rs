//! The Tern engine.
//!
//! The turn loop, policy gate, workers, and steering are planned for roadmap
//! phases 3 and 5. Phase 2 provides the health checks behind `tern doctor`,
//! so front ends reach the platform and store only through this crate.

#![forbid(unsafe_code)]

pub mod doctor;
