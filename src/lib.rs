//! Embornal is a toolkit for agents.
//!
//! This crate holds the memory: a wiki of small facts that an agent writes
//! and reads through the `embornal memory` commands.

pub mod cli;
pub mod config;
pub mod embedding;
pub mod error;
pub mod memory;
pub mod server;

pub use config::{Config, Paths};
pub use error::{Error, Result};
pub use memory::Memory;
