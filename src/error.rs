//! Error types shared by the memory subsystem.

use crate::memory::acl::MAX_SUBJECT_LEN;
use crate::memory::path::{MAX_PATH_LEN, MAX_SEGMENT_LEN};
use crate::memory::tag::MAX_TAG_VALUE_LEN;
use std::path::PathBuf;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid wiki path: {0}")]
    Path(#[from] PathError),

    #[error("invalid tag: {0}")]
    Tag(#[from] TagError),

    #[error("invalid policy object: {0}")]
    Policy(#[from] PolicyError),

    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("failed to read {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error("cannot locate the home directory")]
    NoHome,

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The configured embedding geometry does not agree with the one recorded
    /// in the database. Vector indexes have a fixed width, so this is fatal.
    #[error(
        "embedding dimensions mismatch: the database uses {stored}, the configuration asks for {configured}"
    )]
    EmbeddingDimensionsMismatch { stored: usize, configured: usize },

    #[error("the database schema version is {found}, but this build supports up to {supported}")]
    SchemaTooNew { found: i64, supported: i64 },

    #[error("embedding has {got} dimensions, the database expects {want}")]
    EmbeddingWidth { want: usize, got: usize },

    #[error("unknown embedding provider '{0}': use 'gguf' or 'none'")]
    UnknownProvider(String),

    /// The provider is named in the configuration, but this build cannot give
    /// it. The message says how to go on.
    #[error(
        "this build has no '{0}' embedding provider: build with the 'gguf' feature, or set embedding.provider to 'none'"
    )]
    ProviderNotBuilt(String),

    #[error("the embedding model failed: {0}")]
    Embedding(String),

    #[error("cannot fetch the embedding model {file} from {repo}: {reason}")]
    Download {
        repo: String,
        file: String,
        reason: String,
    },

    #[error("path not found: {0}")]
    PathNotFound(String),

    #[error("there is no node {0} in this collection")]
    NoSuchNode(String),

    #[error("there is no collection {0}: run `embornal code index` first")]
    NoSuchCollection(String),

    /// A file that the walk reached and that no grammar of this build reads.
    #[error("no grammar for '{0}'")]
    UnsupportedLanguage(String),

    #[error("access control error: {0}")]
    Casbin(#[from] casbin::Error),

    #[error("{subject} may not {action} {path}")]
    Denied {
        subject: String,
        action: crate::memory::acl::Action,
        path: String,
    },

    #[error("the root path holds no facts: name a path such as /projects/embornal")]
    RootHoldsNoFacts,

    #[error("a fact needs content")]
    EmptyContent,

    /// The memory writes this tag itself, so nobody else may send it.
    #[error("the tag '{0}' belongs to the memory: it names the writer of a fact")]
    ReservedTag(String),

    #[error("the server failed: {0}")]
    Serve(String),

    /// An argument of the command line that the tool cannot read.
    #[error("{0}")]
    BadArgument(String),

    /// The caller gave no token, or a token that opens nothing.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("there is no token {0}")]
    NoSuchToken(String),

    #[error("{0}")]
    Token(String),

    /// The server did not answer at all. The commands stop here: they never
    /// fall back to a memory on this machine.
    #[error("the memory at {url} does not answer: {reason}")]
    ServerUnreachable { url: String, reason: String },

    #[error("the memory at {url} refused the request ({status}): {message}")]
    Server {
        url: String,
        status: u16,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("a path must start with '/'")]
    NoLeadingSlash,

    #[error("a path must not contain an empty segment")]
    EmptySegment,

    #[error("'{0}' is a relative segment and is not allowed")]
    RelativeSegment(String),

    #[error(
        "segment '{0}' is invalid: use lowercase letters, digits, '.', '_' and '-', and start with a letter or a digit"
    )]
    InvalidSegment(String),

    #[error("segment '{0}' is longer than {MAX_SEGMENT_LEN} characters")]
    SegmentTooLong(String),

    #[error("the path is longer than {MAX_PATH_LEN} characters")]
    PathTooLong,

    #[error("the root path holds no facts")]
    RootHoldsNoFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    #[error("a tag must have the form key=value")]
    MissingSeparator,

    #[error(
        "tag key '{0}' is invalid: use lowercase letters, digits, '_' and '-', and start with a letter"
    )]
    InvalidKey(String),

    #[error("a tag value must not be empty")]
    EmptyValue,

    #[error("tag value '{0}' is longer than {MAX_TAG_VALUE_LEN} characters")]
    ValueTooLong(String),

    #[error("a tag value must not hold a control character")]
    ControlCharacter(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("a policy object must start with 'path:' or 'tag:'")]
    UnknownPrefix,

    #[error("invalid path pattern '{pattern}': {source}")]
    Pattern {
        pattern: String,
        #[source]
        source: PathError,
    },

    #[error("invalid tag in policy object: {0}")]
    Tag(#[from] TagError),

    #[error("'{0}' is not a known action")]
    UnknownAction(String),

    #[error("'{0}' is not a known effect")]
    UnknownEffect(String),

    #[error("a subject needs a name")]
    EmptySubject,

    #[error(
        "subject '{0}' is invalid: use no space, no '=' and no ',', because the name becomes an access tag"
    )]
    InvalidSubject(String),

    #[error("subject '{0}' is longer than {MAX_SUBJECT_LEN} characters")]
    SubjectTooLong(String),
}
