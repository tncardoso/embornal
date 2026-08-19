//! Where the work happens.
//!
//! A command of the tool does not know whether the memory is a file on this
//! machine or a server on another one. It asks a [`MemoryApi`], and the
//! [`Backend`] decides which of the two answers.
//!
//! The server runs the same [`Memory`] that a memory on one machine runs, so
//! the two cannot drift apart: there is one place where a fact is written and
//! one place where the access rules are read.
//!
//! `reindex` and the wiki are not here. They need the file itself, so they
//! run on the machine that holds it.

use crate::config::RecallConfig;
use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::api::{CatOptions, Listing, Memory, RecallOptions, TreeNode, TreeOptions};
use crate::memory::fact::{Fact, FactId, NewFact, ScoredFact};
use crate::memory::path::WikiPath;
use crate::memory::tag::TagSet;

/// What every command of `embornal memory` needs.
///
/// The methods are the commands. A caller that holds one of these knows
/// nothing about SQLite, about HTTP, or about who is answering.
pub trait MemoryApi {
    /// Writes one fact and gives it back as it was written.
    fn store(&mut self, request: NewFact) -> Result<Fact>;

    /// Lists one level below a path.
    fn ls(&mut self, path: &WikiPath) -> Result<Listing>;

    /// Shows the whole tree below a path.
    fn tree(&mut self, path: &WikiPath, options: TreeOptions) -> Result<TreeNode>;

    /// Reads the facts of one path.
    fn cat(&mut self, path: &WikiPath, options: CatOptions) -> Result<Vec<Fact>>;

    /// Searches the memory.
    fn recall(&mut self, query: Option<&str>, options: RecallOptions) -> Result<Vec<ScoredFact>>;

    /// Reads the tags of one fact, inheritance resolved.
    fn effective_tags(&mut self, fact: FactId) -> Result<TagSet>;

    /// Who the memory believes is asking.
    ///
    /// It does not share the name of [`Memory::subject`]: a trait method that
    /// takes `&mut self` wins over an inherent one that takes `&self`, so two
    /// methods of one name here would send a caller somewhere it did not mean
    /// to go.
    fn whoami(&mut self) -> Result<Subject>;

    /// The values that a command uses when its flags say nothing.
    ///
    /// A memory on this machine reads them from its own configuration. A
    /// client reads them from the server, so that both ends of one memory
    /// agree on what a recall gives back.
    fn recall_defaults(&mut self) -> Result<RecallConfig>;
}

/// A memory on this machine.
///
/// `Memory` keeps its own methods, so the wiki and the tests reach it the way
/// they always did. This carries them into the trait and nothing more.
impl MemoryApi for Memory {
    fn store(&mut self, request: NewFact) -> Result<Fact> {
        Memory::store(self, request)
    }

    fn ls(&mut self, path: &WikiPath) -> Result<Listing> {
        Memory::ls(self, path)
    }

    fn tree(&mut self, path: &WikiPath, options: TreeOptions) -> Result<TreeNode> {
        Memory::tree(self, path, options)
    }

    fn cat(&mut self, path: &WikiPath, options: CatOptions) -> Result<Vec<Fact>> {
        Memory::cat(self, path, options)
    }

    fn recall(&mut self, query: Option<&str>, options: RecallOptions) -> Result<Vec<ScoredFact>> {
        Memory::recall(self, query, options)
    }

    fn effective_tags(&mut self, fact: FactId) -> Result<TagSet> {
        Memory::effective_tags(self, fact)
    }

    fn whoami(&mut self) -> Result<Subject> {
        Ok(Memory::subject(self).clone())
    }

    fn recall_defaults(&mut self) -> Result<RecallConfig> {
        Ok(self.config().recall.clone())
    }
}

/// The memory that a command works on.
///
/// The configuration decides: a `server` section makes this a client, and no
/// such section keeps the memory on this machine.
pub enum Backend {
    /// The file of this machine.
    Local(Box<Memory>),
    /// A memory on a server.
    Remote(Box<crate::client::Client>),
}

impl Backend {
    /// Gives back the memory of this machine, or says why there is none.
    ///
    /// `reindex`, the wiki and the tokens work on the file itself. A client
    /// has no file, so those belong on the machine that holds the memory.
    pub fn into_local(self, command: &str) -> Result<Memory> {
        match self {
            Self::Local(memory) => Ok(*memory),
            Self::Remote(_) => Err(Error::BadArgument(format!(
                "`{command}` works on the memory itself, and this one is on a \
                 server. Run it on the machine that holds the memory."
            ))),
        }
    }
}

/// Sends each call to the memory that this backend holds.
macro_rules! dispatch {
    ($self:ident, $method:ident $(, $arg:expr)*) => {
        match $self {
            Backend::Local(memory) => MemoryApi::$method(memory.as_mut() $(, $arg)*),
            Backend::Remote(client) => MemoryApi::$method(client.as_mut() $(, $arg)*),
        }
    };
}

impl MemoryApi for Backend {
    fn store(&mut self, request: NewFact) -> Result<Fact> {
        dispatch!(self, store, request)
    }

    fn ls(&mut self, path: &WikiPath) -> Result<Listing> {
        dispatch!(self, ls, path)
    }

    fn tree(&mut self, path: &WikiPath, options: TreeOptions) -> Result<TreeNode> {
        dispatch!(self, tree, path, options)
    }

    fn cat(&mut self, path: &WikiPath, options: CatOptions) -> Result<Vec<Fact>> {
        dispatch!(self, cat, path, options)
    }

    fn recall(&mut self, query: Option<&str>, options: RecallOptions) -> Result<Vec<ScoredFact>> {
        dispatch!(self, recall, query, options)
    }

    fn effective_tags(&mut self, fact: FactId) -> Result<TagSet> {
        dispatch!(self, effective_tags, fact)
    }

    fn whoami(&mut self) -> Result<Subject> {
        dispatch!(self, whoami)
    }

    fn recall_defaults(&mut self) -> Result<RecallConfig> {
        dispatch!(self, recall_defaults)
    }
}
