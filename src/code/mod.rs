//! The code index.
//!
//! The index holds a tree of one repository: the directories, the files, and
//! the definitions that tree-sitter finds inside each file. Beside the tree it
//! holds what an agent wrote about each of those, so that a later question
//! reaches the right function without reading the whole repository.
//!
//! Embornal writes no summary itself. It says which nodes have none, an
//! outside agent writes them, and `describe` takes them back. Nothing here
//! talks to a model.
//!
//! The modules are:
//!
//! - [`api`]: what a command calls.
//! - [`db`]: the SQLite file, its schema and its indexes.
//! - [`index`]: the pass that brings the index up to date with the files.
//! - [`lang`]: the grammars and the queries that list definitions.
//! - [`node`]: what a node is, and the hashes that say whether it changed.
//! - [`parse`]: what tree-sitter finds in one file.
//! - [`queue`]: what still waits for a summary, and what comes back.
//! - [`repo`]: which repository a command works on.
//! - [`tree`]: what holds what, read off the spans, and the hashes.
//! - [`walk`]: which files of a repository the index reads.

pub mod api;
pub mod db;
pub mod index;
pub mod lang;
pub mod node;
pub mod parse;
pub mod queue;
pub mod repo;
pub mod tree;
pub mod walk;

pub use api::CodeIndex;
pub use db::Database;
pub use node::{Node, NodeId, NodeKind, PoolKey};
