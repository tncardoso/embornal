//! Embornal is a toolkit for agents.
//!
//! This crate holds the memory: a wiki of small facts that an agent writes
//! and reads through the `embornal memory` commands.

pub mod api;
pub mod cli;
pub mod client;
pub mod code;
pub mod common;
pub mod config;
pub mod embedding;
pub mod error;
pub mod memory;
pub mod wiki;

pub use config::{Config, Paths};
pub use error::{Error, Result};
pub use memory::Memory;
