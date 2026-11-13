//! Pure domain model for op-workflow.
//!
//! No I/O, no async, no Tauri. Everything here is plain data and pure functions so it
//! can be unit tested without a terminal, a network or an AI CLI.

pub mod error;
pub mod finding;
pub mod task;
pub mod view;
pub mod workflow;

pub use error::{DomainError, DomainResult};
