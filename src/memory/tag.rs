//! ABAC tags.
//!
//! A tag is a `key=value` pair. Tags sit on facts and on paths. A fact takes
//! the tags of every path above it, and its own tags win over the inherited
//! ones. This lets one mark `/work/acme` as `client=acme` once, instead of
//! marking each fact.

use crate::error::TagError;
use crate::memory::path::WikiPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The maximum length of a tag value.
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The key of a tag. Lowercase, and it starts with a letter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TagKey(String);

impl TagKey {
    pub fn parse(input: &str) -> Result<Self, TagError> {
        let lowered = input.trim().to_lowercase();
        let mut chars = lowered.chars();
        let Some(first) = chars.next() else {
            return Err(TagError::InvalidKey(input.to_string()));
        };
        if !first.is_ascii_alphabetic() {
            return Err(TagError::InvalidKey(input.to_string()));
        }
        for c in chars {
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(TagError::InvalidKey(input.to_string()));
            }
        }
        Ok(Self(lowered))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TagKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for TagKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The value of a tag. The memory keeps the case that the writer used.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TagValue(String);

impl TagValue {
    pub fn parse(input: &str) -> Result<Self, TagError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(TagError::EmptyValue);
        }
        if trimmed.chars().count() > MAX_TAG_VALUE_LEN {
            return Err(TagError::ValueTooLong(trimmed.to_string()));
        }
        // The access check writes the tags of a fact into one string, with a
        // control character between the fields. A value that holds such a
        // character could write a field of its own, and so claim a tag that
        // the fact does not carry.
        if trimmed.chars().any(char::is_control) {
            return Err(TagError::ControlCharacter(trimmed.to_string()));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TagValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for TagValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `key=value` pair.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag {
    pub key: TagKey,
    pub value: TagValue,
}

/// A tag travels as the `key=value` text that a person writes, which is what
/// [`Tag::parse`] reads back. The two must agree, or a tag that goes out
/// cannot come home.
impl Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Tag {
    pub fn new(key: TagKey, value: TagValue) -> Self {
        Self { key, value }
    }

    /// Parses the `key=value` form that the command line uses.
    ///
    /// The value can hold `=`, so only the first one separates.
    pub fn parse(input: &str) -> Result<Self, TagError> {
        let (key, value) = input.split_once('=').ok_or(TagError::MissingSeparator)?;
        Ok(Self {
            key: TagKey::parse(key)?,
            value: TagValue::parse(value)?,
        })
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.key, self.value)
    }
}

impl std::str::FromStr for Tag {
    type Err = TagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// The tags that apply to one fact, after the inheritance is resolved.
///
/// One key holds one value. The set is ordered by key, so two equal sets
/// always print the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSet(BTreeMap<TagKey, TagValue>);

/// A set of tags travels as a list of `key=value` texts, in the order of the
/// keys, because that is how a person reads it and how a fact carries it.
impl Serialize for TagSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for TagSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Vec::<Tag>::deserialize(deserializer)?.into_iter().collect())
    }
}

impl TagSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a tag and returns the value that it replaced.
    pub fn insert(&mut self, tag: Tag) -> Option<TagValue> {
        self.0.insert(tag.key, tag.value)
    }

    pub fn get(&self, key: &TagKey) -> Option<&TagValue> {
        self.0.get(key)
    }

    /// Returns `true` if the set holds this exact pair.
    pub fn matches(&self, tag: &Tag) -> bool {
        self.0.get(&tag.key) == Some(&tag.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = Tag> + '_ {
        self.0.iter().map(|(k, v)| Tag::new(k.clone(), v.clone()))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Merges `other` on top of `self`. The values of `other` win.
    ///
    /// The inheritance walks from the root down to the fact, so each step
    /// overrides the step above it.
    pub fn overlay(&mut self, other: &TagSet) {
        for (key, value) in &other.0 {
            self.0.insert(key.clone(), value.clone());
        }
    }
}

impl FromIterator<Tag> for TagSet {
    fn from_iter<I: IntoIterator<Item = Tag>>(iter: I) -> Self {
        let mut set = Self::new();
        for tag in iter {
            set.insert(tag);
        }
        set
    }
}

impl fmt::Display for TagSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for tag in self.iter() {
            if !first {
                f.write_str(" ")?;
            }
            write!(f, "{tag}")?;
            first = false;
        }
        Ok(())
    }
}

/// The tags of one path, before the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTags {
    pub path: WikiPath,
    pub tags: TagSet,
}

/// Merges the tags of a chain of paths and of a fact into one set.
///
/// `ancestry` must run from the root down to the path of the fact.
pub fn resolve(ancestry: &[PathTags], own: &TagSet) -> TagSet {
    let mut resolved = TagSet::new();
    for level in ancestry {
        resolved.overlay(&level.tags);
    }
    resolved.overlay(own);
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> Tag {
        Tag::parse(s).unwrap()
    }

    #[test]
    fn a_value_cannot_carry_a_field_of_its_own_into_the_access_check() {
        // The access check writes the tags of a fact into one string, with
        // this character between the fields. A value that held it could name
        // a tag that the fact does not carry, and so read what it must not.
        assert!(Tag::parse("a=b\u{1f}tag:visibility=public").is_err());
        assert!(TagValue::parse("one\ntwo").is_err());
        assert!(TagValue::parse("one\ttwo").is_err());
        // A space is not a control character, and a value may hold one.
        assert!(TagValue::parse("one two").is_ok());
    }

    #[test]
    fn parses_the_pair_form() {
        let t = tag("visibility=private");
        assert_eq!(t.key.as_str(), "visibility");
        assert_eq!(t.value.as_str(), "private");
    }

    #[test]
    fn only_the_first_equals_separates() {
        let t = tag("query=a=b");
        assert_eq!(t.key.as_str(), "query");
        assert_eq!(t.value.as_str(), "a=b");
    }

    #[test]
    fn folds_the_key_but_keeps_the_value() {
        let t = tag("Client=ACME Corp");
        assert_eq!(t.key.as_str(), "client");
        assert_eq!(t.value.as_str(), "ACME Corp");
    }

    #[test]
    fn rejects_bad_tags() {
        assert_eq!(Tag::parse("novalue"), Err(TagError::MissingSeparator));
        assert_eq!(Tag::parse("key="), Err(TagError::EmptyValue));
        assert_eq!(
            Tag::parse("1key=v"),
            Err(TagError::InvalidKey("1key".into()))
        );
        assert_eq!(
            Tag::parse("bad key=v"),
            Err(TagError::InvalidKey("bad key".into()))
        );
    }

    #[test]
    fn one_key_holds_one_value() {
        let mut set = TagSet::new();
        set.insert(tag("visibility=public"));
        let old = set.insert(tag("visibility=private"));
        assert_eq!(old.unwrap().as_str(), "public");
        assert_eq!(set.len(), 1);
        assert!(set.matches(&tag("visibility=private")));
        assert!(!set.matches(&tag("visibility=public")));
    }

    #[test]
    fn the_deeper_path_wins() {
        let ancestry = vec![
            PathTags {
                path: WikiPath::root(),
                tags: [tag("visibility=public"), tag("owner=thiago")]
                    .into_iter()
                    .collect(),
            },
            PathTags {
                path: WikiPath::parse("/work").unwrap(),
                tags: [tag("visibility=private")].into_iter().collect(),
            },
        ];
        let own: TagSet = [tag("visibility=secret")].into_iter().collect();

        let resolved = resolve(&ancestry, &own);
        assert!(resolved.matches(&tag("visibility=secret")));
        assert!(resolved.matches(&tag("owner=thiago")));
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn the_path_tags_apply_when_the_fact_is_silent() {
        let ancestry = vec![PathTags {
            path: WikiPath::parse("/work").unwrap(),
            tags: [tag("client=acme")].into_iter().collect(),
        }];
        let resolved = resolve(&ancestry, &TagSet::new());
        assert!(resolved.matches(&tag("client=acme")));
    }

    #[test]
    fn prints_in_a_stable_order() {
        let set: TagSet = [tag("z=1"), tag("a=2")].into_iter().collect();
        assert_eq!(set.to_string(), "a=2 z=1");
    }
}
