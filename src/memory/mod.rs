//! The memory.
//!
//! The memory is a wiki that an agent writes one small fact at a time. Each
//! fact belongs to a path such as `/projects/embornal`. The memory finds a
//! fact again by keyword, by meaning, or by how strong the fact still is.
//!
//! The modules are:
//!
//! - [`path`]: the tree of the wiki and the rules of a path.
//! - [`fact`]: a fact and how its strength falls with time.
//! - [`tag`]: the `key=value` attributes that control access.
//! - [`acl`]: the types that connect Casbin to the tables.
//! - [`backend`]: the one shape that a memory has, here or on a server.
//! - [`db`]: the SQLite file, its schema and its indexes.
//! - [`token`]: the secrets that let a client reach a server.
//! - [`time`]: how a moment goes into a column.

pub mod acl;
pub mod api;
pub mod backend;
pub mod db;
pub mod fact;
pub mod guard;
pub mod link;
pub mod path;
pub mod tag;
pub mod time;
pub mod token;

pub use acl::{AccessFilter, Action, Effect, PolicyObject, PolicyRule, Resource, Subject};
pub use api::{CatOptions, Listing, Memory, RecallOptions, TreeNode, TreeOptions};
pub use backend::{Backend, MemoryApi};
pub use db::Database;
pub use fact::{Fact, FactId, NewFact, OrderBy, ScoredFact, Signal};
pub use guard::Guard;
pub use path::{PathEntry, PathId, PathRecord, WikiPath};
pub use tag::{Tag, TagKey, TagSet, TagValue};
pub use token::Token;
