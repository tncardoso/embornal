//! Wiki paths.
//!
//! A path looks like `/projects/embornal`. Paths are canonical: the memory
//! folds them to lowercase and rejects anything that two different writers
//! could spell in two different ways. Without this rule `/Projects` and
//! `/projects` become two nodes that hold half of the same knowledge each.

use crate::error::PathError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

/// The maximum length of one segment.
pub const MAX_SEGMENT_LEN: usize = 128;

/// The maximum length of a full path.
pub const MAX_PATH_LEN: usize = 1024;

/// The rowid of the root path. The migration writes this row.
pub const ROOT_ID: PathId = PathId(1);

/// The path that holds the facts about the memory itself.
pub const MEMORY_PATH: &str = "/memory";

/// The primary key of a row in `paths`.
///
/// This value is internal. It is stable inside one database file only, so it
/// must not appear in output that a user or an agent keeps. Use [`Ulid`] for
/// that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PathId(pub i64);

impl fmt::Display for PathId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A canonical wiki path.
///
/// The inner string always starts with `/`. It never ends with `/`, unless it
/// is the root, which is exactly `/`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct WikiPath(String);

impl WikiPath {
    /// The root path `/`.
    pub fn root() -> Self {
        Self("/".to_string())
    }

    /// Parses and canonicalizes `input`.
    ///
    /// The function folds the path to lowercase, removes the trailing slash
    /// and checks every segment.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return Err(PathError::NoLeadingSlash);
        }

        let body = trimmed.trim_end_matches('/');
        if body.is_empty() {
            return Ok(Self::root());
        }

        let mut canonical = String::with_capacity(body.len());
        for segment in body.split('/').skip(1) {
            let segment = validate_segment(segment)?;
            canonical.push('/');
            canonical.push_str(&segment);
        }

        if canonical.len() > MAX_PATH_LEN {
            return Err(PathError::PathTooLong);
        }
        Ok(Self(canonical))
    }

    /// Returns the canonical text of the path.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if this is the root path.
    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// Returns the last segment, or `None` for the root.
    pub fn segment(&self) -> Option<&str> {
        if self.is_root() {
            None
        } else {
            self.0.rsplit('/').next()
        }
    }

    /// Returns the parent path, or `None` for the root.
    pub fn parent(&self) -> Option<WikiPath> {
        if self.is_root() {
            return None;
        }
        let cut = self.0.rfind('/').expect("a non-root path holds a slash");
        if cut == 0 {
            Some(Self::root())
        } else {
            Some(Self(self.0[..cut].to_string()))
        }
    }

    /// Returns every segment, from the first to the last.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').skip(1).filter(|s| !s.is_empty())
    }

    /// Returns the chain of paths from the root down to this path, inclusive.
    ///
    /// `/a/b` gives `[/, /a, /a/b]`. The store operation uses this chain to
    /// create the missing nodes, and the ABAC layer uses it to collect the
    /// inherited tags.
    pub fn ancestry(&self) -> Vec<WikiPath> {
        let mut chain = vec![Self::root()];
        let mut current = String::new();
        for segment in self.segments() {
            current.push('/');
            current.push_str(segment);
            chain.push(Self(current.clone()));
        }
        chain
    }

    /// Returns `true` if `self` is at or below `other`.
    ///
    /// The comparison is aware of segment borders: `/foobar` is not below
    /// `/foo`.
    pub fn is_under(&self, other: &WikiPath) -> bool {
        if other.is_root() {
            return true;
        }
        if self.0 == other.0 {
            return true;
        }
        self.0.starts_with(&other.0) && self.0.as_bytes().get(other.0.len()) == Some(&b'/')
    }

    /// Returns the number of segments. The root has depth 0.
    pub fn depth(&self) -> usize {
        self.segments().count()
    }

    /// Appends `segment` to the path.
    pub fn join(&self, segment: &str) -> Result<WikiPath, PathError> {
        let segment = validate_segment(segment)?;
        let mut next = if self.is_root() {
            String::from("/")
        } else {
            let mut s = self.0.clone();
            s.push('/');
            s
        };
        next.push_str(&segment);
        if next.len() > MAX_PATH_LEN {
            return Err(PathError::PathTooLong);
        }
        Ok(Self(next))
    }

    /// Returns the SQL `GLOB` pattern that matches this path and everything
    /// below it.
    pub fn subtree_glob(&self) -> String {
        if self.is_root() {
            "/*".to_string()
        } else {
            format!("{}/*", self.0)
        }
    }
}

impl fmt::Display for WikiPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for WikiPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A path travels as the text that it holds, and it comes back through
/// [`WikiPath::parse`]. A path that arrives from another machine is thus held
/// to the same rules as one that a person wrote, so no query and no page ever
/// meets a path that broke them.
impl<'de> Deserialize<'de> for WikiPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Checks one segment and returns its canonical form.
fn validate_segment(segment: &str) -> Result<String, PathError> {
    if segment.is_empty() {
        return Err(PathError::EmptySegment);
    }
    if segment == "." || segment == ".." {
        return Err(PathError::RelativeSegment(segment.to_string()));
    }
    if segment.chars().count() > MAX_SEGMENT_LEN {
        return Err(PathError::SegmentTooLong(segment.to_string()));
    }

    let lowered = segment.to_lowercase();
    let mut chars = lowered.chars();
    let first = chars.next().expect("the segment is not empty");
    if !first.is_ascii_alphanumeric() {
        return Err(PathError::InvalidSegment(segment.to_string()));
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return Err(PathError::InvalidSegment(segment.to_string()));
        }
    }
    Ok(lowered)
}

/// A row of the `paths` table.
///
/// The record holds structure only. Everything that a reader would call
/// content, the description of the path included, lives in facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRecord {
    pub id: PathId,
    pub ulid: Ulid,
    pub parent_id: Option<PathId>,
    pub segment: String,
    pub full_path: WikiPath,
    pub created_at: DateTime<Utc>,
}

/// One entry of a `memory ls` listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathEntry {
    pub path: WikiPath,
    /// The number of live facts that the path holds directly.
    pub fact_count: u64,
    /// The number of direct children.
    pub child_count: u64,
}

impl PathEntry {
    /// Returns `true` if the path holds facts of its own.
    ///
    /// A path can be a prefix and hold content at the same time, so this is
    /// independent of `child_count`.
    pub fn has_content(&self) -> bool {
        self.fact_count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_folds_case() {
        let path = WikiPath::parse("/Projects/Embornal").unwrap();
        assert_eq!(path.as_str(), "/projects/embornal");
    }

    #[test]
    fn removes_the_trailing_slash() {
        assert_eq!(
            WikiPath::parse("/projects/embornal/").unwrap().as_str(),
            "/projects/embornal"
        );
    }

    #[test]
    fn reads_the_root_in_every_spelling() {
        assert!(WikiPath::parse("/").unwrap().is_root());
        assert!(WikiPath::parse("//").unwrap().is_root());
        assert!(WikiPath::parse("  /  ").unwrap().is_root());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(WikiPath::parse("projects"), Err(PathError::NoLeadingSlash));
        assert_eq!(WikiPath::parse("/a//b"), Err(PathError::EmptySegment));
        assert_eq!(
            WikiPath::parse("/a/../b"),
            Err(PathError::RelativeSegment("..".into()))
        );
        assert_eq!(
            WikiPath::parse("/-leading-dash"),
            Err(PathError::InvalidSegment("-leading-dash".into()))
        );
        assert_eq!(
            WikiPath::parse("/with space"),
            Err(PathError::InvalidSegment("with space".into()))
        );
    }

    #[test]
    fn accepts_the_allowed_alphabet() {
        for good in [
            "/a",
            "/a1",
            "/dot.name",
            "/snake_case",
            "/kebab-case",
            "/9lives",
        ] {
            assert!(WikiPath::parse(good).is_ok(), "{good} must parse");
        }
    }

    #[test]
    fn walks_up_the_tree() {
        let path = WikiPath::parse("/a/b/c").unwrap();
        assert_eq!(path.parent().unwrap().as_str(), "/a/b");
        assert_eq!(path.parent().unwrap().parent().unwrap().as_str(), "/a");
        assert!(
            path.parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .is_root()
        );
        assert_eq!(WikiPath::root().parent(), None);
    }

    #[test]
    fn lists_the_ancestry_from_the_root() {
        let chain: Vec<String> = WikiPath::parse("/a/b")
            .unwrap()
            .ancestry()
            .iter()
            .map(|p| p.to_string())
            .collect();
        assert_eq!(chain, vec!["/", "/a", "/a/b"]);
        assert_eq!(
            WikiPath::root()
                .ancestry()
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
            vec!["/"]
        );
    }

    #[test]
    fn respects_segment_borders() {
        let foo = WikiPath::parse("/foo").unwrap();
        assert!(WikiPath::parse("/foo/bar").unwrap().is_under(&foo));
        assert!(WikiPath::parse("/foo").unwrap().is_under(&foo));
        assert!(!WikiPath::parse("/foobar").unwrap().is_under(&foo));
        assert!(
            WikiPath::parse("/foobar")
                .unwrap()
                .is_under(&WikiPath::root())
        );
    }

    #[test]
    fn reports_the_segment_and_the_depth() {
        let path = WikiPath::parse("/a/b/c").unwrap();
        assert_eq!(path.segment(), Some("c"));
        assert_eq!(path.depth(), 3);
        assert_eq!(WikiPath::root().segment(), None);
        assert_eq!(WikiPath::root().depth(), 0);
    }

    #[test]
    fn joins_segments() {
        let root = WikiPath::root();
        assert_eq!(root.join("a").unwrap().as_str(), "/a");
        assert_eq!(root.join("a").unwrap().join("B").unwrap().as_str(), "/a/b");
        assert!(root.join("bad segment").is_err());
    }

    #[test]
    fn builds_the_subtree_glob() {
        assert_eq!(WikiPath::root().subtree_glob(), "/*");
        assert_eq!(WikiPath::parse("/a").unwrap().subtree_glob(), "/a/*");
    }
}
