//! A node of the tree, and the hashes that say whether it changed.

use sha2::{Digest, Sha256};
use std::fmt;

/// The row number of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub i64);

/// What a node is.
///
/// The order of the variants is the order of the tree: a `Repo` holds `Dir`s,
/// a `Dir` holds `File`s, and a `File` holds the definitions that the grammar
/// of its language names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Repo,
    Dir,
    File,
    Module,
    Class,
    Impl,
    Function,
}

impl NodeKind {
    /// The word that goes in the `kind` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::Dir => "dir",
            Self::File => "file",
            Self::Module => "module",
            Self::Class => "class",
            Self::Impl => "impl",
            Self::Function => "function",
        }
    }

    /// Reads the word back. An unknown word gives `None`, which is what a file
    /// written by a newer build would hold.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "repo" => Self::Repo,
            "dir" => Self::Dir,
            "file" => Self::File,
            "module" => Self::Module,
            "class" => Self::Class,
            "impl" => Self::Impl,
            "function" => Self::Function,
            _ => return None,
        })
    }

    /// Whether the hash of this node comes from the hashes of its children.
    ///
    /// A directory and the root hold no bytes of their own, so their hash can
    /// only come from below. Everything else hashes the bytes of its span.
    pub fn hashes_its_children(&self) -> bool {
        matches!(self, Self::Repo | Self::Dir)
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The hash of what a node holds.
///
/// For a file and everything below it, this is the hash of the bytes of the
/// span. For a directory and the root, it is the hash of the hashes of the
/// children, in the order that [`ContentHash::of_children`] receives them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(String);

impl ContentHash {
    /// Hashes the bytes of one node.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hex(&hasher.finalize()))
    }

    /// Hashes the children of a directory or of the root.
    ///
    /// The caller gives the children in a fixed order, so that the same tree
    /// hashes the same way on every machine and on every run.
    pub fn of_children<'a>(children: impl IntoIterator<Item = &'a ContentHash>) -> Self {
        let mut hasher = Sha256::new();
        for child in children {
            hasher.update(child.as_str().as_bytes());
            hasher.update(b"\n");
        }
        Self(hex(&hasher.finalize()))
    }

    /// Takes a hash that the database already holds.
    pub fn from_stored(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a summary is filed under.
///
/// The key is the qualified name and the content hash together. The hash alone
/// would let a body that says nothing on its own — `Self::default()`, and the
/// fifty places that hold exactly that — take the description of whichever of
/// them an agent happened to read first. The name alone would go stale the
/// moment the body changed.
///
/// The key carries no collection and no repository, and that is the point: the
/// same code in another branch, or in another checkout, answers with the
/// summary that is already written.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PoolKey(String);

impl PoolKey {
    pub fn new(qualified_name: &str, content: &ContentHash) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(qualified_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_str().as_bytes());
        Self(hex(&hasher.finalize()))
    }

    pub fn from_stored(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PoolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub ulid: String,
    pub parent_id: Option<NodeId>,
    pub kind: NodeKind,
    pub name: String,
    /// The path of the file and the chain of definitions inside it, such as
    /// `src/memory/api.rs::Memory::recall`.
    pub qualified_name: String,
    pub rel_path: String,
    pub language: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub depth: u32,
    pub content_hash: ContentHash,
    pub pool_key: PoolKey,
    /// Whether the grammar failed on the file that holds this node. A file
    /// that carries this holds no children: the index does not claim a shape
    /// that it could not read.
    pub parse_errors: bool,
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_survives_a_trip_through_its_word() {
        for kind in [
            NodeKind::Repo,
            NodeKind::Dir,
            NodeKind::File,
            NodeKind::Module,
            NodeKind::Class,
            NodeKind::Impl,
            NodeKind::Function,
        ] {
            assert_eq!(NodeKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(NodeKind::parse("closure"), None);
    }

    #[test]
    fn only_a_directory_and_the_root_hash_their_children() {
        assert!(NodeKind::Repo.hashes_its_children());
        assert!(NodeKind::Dir.hashes_its_children());
        assert!(!NodeKind::File.hashes_its_children());
        assert!(!NodeKind::Function.hashes_its_children());
    }

    #[test]
    fn the_same_bytes_hash_the_same_way_twice() {
        let one = ContentHash::of_bytes(b"fn a() {}");
        let two = ContentHash::of_bytes(b"fn a() {}");
        assert_eq!(one, two);
        assert_eq!(one.as_str().len(), 64);
    }

    #[test]
    fn one_byte_of_difference_moves_the_hash() {
        assert_ne!(
            ContentHash::of_bytes(b"fn a() {}"),
            ContentHash::of_bytes(b"fn b() {}")
        );
    }

    #[test]
    fn a_directory_follows_the_children_that_it_holds() {
        let one = ContentHash::of_bytes(b"one");
        let two = ContentHash::of_bytes(b"two");

        let before = ContentHash::of_children([&one, &two]);
        assert_eq!(before, ContentHash::of_children([&one, &two]));

        // A child that changes moves the directory.
        let other = ContentHash::of_bytes(b"other");
        assert_ne!(before, ContentHash::of_children([&one, &other]));

        // The order is part of the hash, so the caller must fix one.
        assert_ne!(before, ContentHash::of_children([&two, &one]));
    }

    #[test]
    fn an_empty_directory_still_has_a_hash() {
        let empty = ContentHash::of_children([]);
        assert_eq!(empty.as_str().len(), 64);
        assert_eq!(empty, ContentHash::of_children([]));
    }

    #[test]
    fn the_pool_key_needs_the_name_and_the_body_to_agree() {
        let body = ContentHash::of_bytes(b"Self::default()");
        let mine = PoolKey::new("src/a.rs::new", &body);

        assert_eq!(mine, PoolKey::new("src/a.rs::new", &body));
        // The same trivial body under another name is another key, so one
        // description cannot spread over fifty unrelated places.
        assert_ne!(mine, PoolKey::new("src/b.rs::new", &body));
        // The same name over another body is another key, so a summary never
        // outlives the code that it describes.
        assert_ne!(
            mine,
            PoolKey::new("src/a.rs::new", &ContentHash::of_bytes(b"Self::empty()"))
        );
    }

    #[test]
    fn the_separator_keeps_the_two_halves_apart() {
        // Without a separator, "ab" + "c" and "a" + "bc" would collide.
        let hash = ContentHash::from_stored("c");
        let other = ContentHash::from_stored("bc");
        assert_ne!(PoolKey::new("ab", &hash), PoolKey::new("a", &other));
    }
}
