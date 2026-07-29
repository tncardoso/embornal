//! Access control.
//!
//! The memory asks Casbin, not its own code, whether a subject can touch a
//! fact. Casbin holds the model and the policies; this module holds the types
//! that connect the policies to the tables.
//!
//! A policy object names either a place in the tree or an attribute:
//!
//! ```text
//! p, cli,    path:/work/acme/*,     read,   allow
//! p, cli,    tag:visibility=public, read,   allow
//! p, cli,    path:/secrets/*,       read,   deny
//! g, cli,    reader
//! ```
//!
//! Both forms translate into SQL. A read first asks Casbin for the implicit
//! permissions of the subject, turns them into one `WHERE` fragment and lets
//! the database drop what the subject must not see. This costs one query
//! instead of one check per fact.

use crate::error::PolicyError;
use crate::memory::path::WikiPath;
use crate::memory::tag::{Tag, TagSet};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The Casbin model. It is built into the binary because it changes with the
/// code, not with the data.
pub const MODEL: &str = r#"
[request_definition]
r = sub, obj, act

[policy_definition]
p = sub, obj, act, eft

[role_definition]
g = _, _

[policy_effect]
e = some(where (p.eft == allow)) && !some(where (p.eft == deny))

[matchers]
m = g(r.sub, p.sub) && objMatch(r.obj, p.obj) && r.act == p.act
"#;

/// The name of the matching function that the model calls.
///
/// The enforcer registers it, and it must behave exactly like
/// [`PolicyObject::matches`].
pub const OBJ_MATCH_FN: &str = "objMatch";

/// The subject that the command line uses until real identities exist.
pub const DEFAULT_SUBJECT: &str = "cli";

/// Who asks. For now the command line always sends `cli`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Subject(String);

impl Subject {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn cli() -> Self {
        Self(DEFAULT_SUBJECT.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Subject {
    fn default() -> Self {
        Self::cli()
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the subject wants to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Write,
    Delete,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Read, Action::Write, Action::Delete];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Action {
    type Err = PolicyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "delete" => Ok(Self::Delete),
            other => Err(PolicyError::UnknownAction(other.to_string())),
        }
    }
}

/// The effect of a policy. A deny beats every allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    #[default]
    Allow,
    Deny,
}

impl Effect {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Effect {
    type Err = PolicyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // An empty effect column means allow, which is how Casbin reads a
            // three column policy.
            "allow" | "" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => Err(PolicyError::UnknownEffect(other.to_string())),
        }
    }
}

/// Which paths a policy covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPattern {
    /// One path and nothing else.
    Exact(WikiPath),
    /// One path and everything below it.
    Subtree(WikiPath),
}

impl PathPattern {
    /// Reads `/work/acme` or `/work/acme/*`.
    pub fn parse(input: &str) -> Result<Self, PolicyError> {
        let to_err = |source| PolicyError::Pattern {
            pattern: input.to_string(),
            source,
        };
        match input.strip_suffix("/*") {
            Some(head) => {
                let base = if head.is_empty() {
                    WikiPath::root()
                } else {
                    WikiPath::parse(head).map_err(to_err)?
                };
                Ok(Self::Subtree(base))
            }
            None => Ok(Self::Exact(WikiPath::parse(input).map_err(to_err)?)),
        }
    }

    /// Returns `true` if `path` falls in the pattern.
    pub fn matches(&self, path: &WikiPath) -> bool {
        match self {
            Self::Exact(target) => path == target,
            // A subtree holds its own root: `path:/work/*` covers `/work`.
            Self::Subtree(base) => path.is_under(base),
        }
    }
}

impl fmt::Display for PathPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(path) => write!(f, "{path}"),
            Self::Subtree(base) if base.is_root() => f.write_str("/*"),
            Self::Subtree(base) => write!(f, "{base}/*"),
        }
    }
}

/// What a policy points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyObject {
    Path(PathPattern),
    Tag(Tag),
}

impl PolicyObject {
    pub const PATH_PREFIX: &'static str = "path:";
    pub const TAG_PREFIX: &'static str = "tag:";

    /// Reads `path:/work/*` or `tag:visibility=public`.
    pub fn parse(input: &str) -> Result<Self, PolicyError> {
        if let Some(rest) = input.strip_prefix(Self::PATH_PREFIX) {
            Ok(Self::Path(PathPattern::parse(rest)?))
        } else if let Some(rest) = input.strip_prefix(Self::TAG_PREFIX) {
            Ok(Self::Tag(Tag::parse(rest).map_err(PolicyError::Tag)?))
        } else {
            Err(PolicyError::UnknownPrefix)
        }
    }

    /// Returns `true` if the object covers `resource`.
    ///
    /// The enforcer registers this behaviour under [`OBJ_MATCH_FN`], so the
    /// filter and the single check always agree.
    pub fn matches(&self, resource: &Resource) -> bool {
        match self {
            Self::Path(pattern) => pattern.matches(&resource.path),
            Self::Tag(tag) => resource.tags.matches(tag),
        }
    }
}

impl fmt::Display for PolicyObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(pattern) => write!(f, "{}{pattern}", Self::PATH_PREFIX),
            Self::Tag(tag) => write!(f, "{}{tag}", Self::TAG_PREFIX),
        }
    }
}

impl std::str::FromStr for PolicyObject {
    type Err = PolicyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// What the subject wants to touch.
///
/// The tags are the resolved ones: the tags of the fact merged on top of the
/// tags that it takes from the paths above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub path: WikiPath,
    pub tags: TagSet,
}

impl Resource {
    pub fn new(path: WikiPath, tags: TagSet) -> Self {
        Self { path, tags }
    }

    /// Builds a resource for an operation that touches a path only, such as
    /// the creation of a node.
    pub fn path_only(path: WikiPath) -> Self {
        Self {
            path,
            tags: TagSet::new(),
        }
    }

    /// Writes the resource as the one string that the matcher receives.
    ///
    /// Casbin gives the matcher plain strings, so the path and the tags travel
    /// together in one field. The separator is the ASCII unit separator, which
    /// no path and no tag can hold.
    pub fn to_request(&self) -> String {
        let mut request = format!("{}{}", Self::PATH_FIELD, self.path);
        for tag in self.tags.iter() {
            request.push(Self::FIELD_SEPARATOR);
            request.push_str(Self::TAG_FIELD);
            request.push_str(&tag.to_string());
        }
        request
    }

    /// Reads back what [`Resource::to_request`] wrote.
    pub fn from_request(request: &str) -> Result<Self, PolicyError> {
        let mut path = None;
        let mut tags = TagSet::new();

        for field in request.split(Self::FIELD_SEPARATOR) {
            if let Some(rest) = field.strip_prefix(Self::PATH_FIELD) {
                path = Some(
                    WikiPath::parse(rest).map_err(|source| PolicyError::Pattern {
                        pattern: rest.to_string(),
                        source,
                    })?,
                );
            } else if let Some(rest) = field.strip_prefix(Self::TAG_FIELD) {
                tags.insert(Tag::parse(rest)?);
            } else {
                return Err(PolicyError::UnknownPrefix);
            }
        }

        Ok(Self {
            path: path.ok_or(PolicyError::UnknownPrefix)?,
            tags,
        })
    }

    const FIELD_SEPARATOR: char = '\u{1f}';
    const PATH_FIELD: &'static str = "path:";
    const TAG_FIELD: &'static str = "tag:";
}

/// One row of the policy, in the shape that Casbin returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRule {
    pub subject: Subject,
    pub object: PolicyObject,
    pub action: Action,
    pub effect: Effect,
}

impl PolicyRule {
    /// Reads the `v0..v3` columns that Casbin gives back.
    pub fn from_casbin(fields: &[String]) -> Result<Self, PolicyError> {
        let get = |i: usize| fields.get(i).map(String::as_str).unwrap_or_default();
        Ok(Self {
            subject: Subject::new(get(0)),
            object: PolicyObject::parse(get(1))?,
            action: get(2).parse()?,
            effect: get(3).parse()?,
        })
    }

    /// Writes the rule back in the Casbin column order.
    pub fn to_casbin(&self) -> [String; 4] {
        [
            self.subject.to_string(),
            self.object.to_string(),
            self.action.to_string(),
            self.effect.to_string(),
        ]
    }
}

/// A `WHERE` fragment that keeps only the facts that the subject can touch.
///
/// The fragment reads two aliases from the query around it: `f` for `facts`
/// and `p` for `paths`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessFilter {
    sql: String,
    params: Vec<String>,
}

impl AccessFilter {
    /// The alias that the caller must give to the `facts` table.
    pub const FACT_ALIAS: &'static str = "f";
    /// The alias that the caller must give to the `paths` table.
    pub const PATH_ALIAS: &'static str = "p";

    /// Builds the filter from the permissions of one subject for one action.
    ///
    /// Pass the rules that Casbin resolved, roles included. Rules for another
    /// action are ignored here, so the caller can hand over the whole set.
    pub fn build(rules: &[PolicyRule], action: Action) -> Self {
        let mut allow = Vec::new();
        let mut deny = Vec::new();
        let mut params = Vec::new();

        for rule in rules.iter().filter(|r| r.action == action) {
            let (fragment, mut fragment_params) = predicate(&rule.object);
            match rule.effect {
                Effect::Allow => allow.push(fragment),
                Effect::Deny => deny.push(fragment),
            }
            params.append(&mut fragment_params);
        }

        // No allow means no access. Default deny is the whole point.
        let mut sql = if allow.is_empty() {
            "0".to_string()
        } else if allow.iter().any(|f| f == "1") {
            "1".to_string()
        } else {
            format!("({})", allow.join(" OR "))
        };

        if !deny.is_empty() {
            sql = format!("{sql} AND NOT ({})", deny.join(" OR "));
        }

        Self { sql, params }
    }

    /// Builds the filter of a subject that can do everything.
    pub fn allow_all() -> Self {
        Self {
            sql: "1".to_string(),
            params: Vec::new(),
        }
    }

    /// Builds the filter of a subject that can do nothing.
    pub fn deny_all() -> Self {
        Self {
            sql: "0".to_string(),
            params: Vec::new(),
        }
    }

    /// Returns the SQL of the fragment.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Returns the values that bind to the `?` marks of the fragment, in
    /// order.
    pub fn params(&self) -> &[String] {
        &self.params
    }

    /// Returns `true` if the filter drops every row. The caller can then skip
    /// the query.
    pub fn is_empty_set(&self) -> bool {
        self.sql == "0"
    }

    /// Returns `true` if the filter keeps every row.
    pub fn is_unrestricted(&self) -> bool {
        self.sql == "1"
    }
}

/// Decides whether a request field matches a policy field.
///
/// This is the body of the [`OBJ_MATCH_FN`] function that the model calls. It
/// reads both sides back into types and asks [`PolicyObject::matches`], so the
/// matcher and the SQL filter cannot drift apart. Anything that does not read
/// back is a no match: a rule that nobody can parse must not grant access.
pub fn object_matches(request: &str, policy: &str) -> bool {
    match (Resource::from_request(request), PolicyObject::parse(policy)) {
        (Ok(resource), Ok(object)) => object.matches(&resource),
        _ => false,
    }
}

/// Builds the SQL predicate of one policy object.
fn predicate(object: &PolicyObject) -> (String, Vec<String>) {
    match object {
        PolicyObject::Path(PathPattern::Exact(path)) => (
            format!("{}.full_path = ?", AccessFilter::PATH_ALIAS),
            vec![path.to_string()],
        ),
        PolicyObject::Path(PathPattern::Subtree(base)) if base.is_root() => {
            ("1".to_string(), Vec::new())
        }
        PolicyObject::Path(PathPattern::Subtree(base)) => (
            format!(
                "({alias}.full_path = ? OR {alias}.full_path GLOB ?)",
                alias = AccessFilter::PATH_ALIAS
            ),
            vec![base.to_string(), base.subtree_glob()],
        ),
        PolicyObject::Tag(tag) => (
            format!(
                "EXISTS (SELECT 1 FROM effective_fact_tags eft \
                 WHERE eft.fact_id = {}.id AND eft.key = ? AND eft.value = ?)",
                AccessFilter::FACT_ALIAS
            ),
            vec![tag.key.to_string(), tag.value.to_string()],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> WikiPath {
        WikiPath::parse(s).unwrap()
    }

    fn rule(obj: &str, act: Action, eft: Effect) -> PolicyRule {
        PolicyRule {
            subject: Subject::cli(),
            object: PolicyObject::parse(obj).unwrap(),
            action: act,
            effect: eft,
        }
    }

    #[test]
    fn reads_both_object_forms() {
        assert_eq!(
            PolicyObject::parse("path:/work/*").unwrap(),
            PolicyObject::Path(PathPattern::Subtree(path("/work")))
        );
        assert_eq!(
            PolicyObject::parse("path:/work").unwrap(),
            PolicyObject::Path(PathPattern::Exact(path("/work")))
        );
        assert_eq!(
            PolicyObject::parse("path:/*").unwrap(),
            PolicyObject::Path(PathPattern::Subtree(WikiPath::root()))
        );
        assert_eq!(
            PolicyObject::parse("tag:visibility=public").unwrap(),
            PolicyObject::Tag(Tag::parse("visibility=public").unwrap())
        );
        assert_eq!(
            PolicyObject::parse("/work/*"),
            Err(PolicyError::UnknownPrefix)
        );
    }

    #[test]
    fn prints_back_what_it_read() {
        for text in [
            "path:/work/*",
            "path:/work",
            "path:/*",
            "tag:visibility=public",
        ] {
            assert_eq!(PolicyObject::parse(text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn a_subtree_holds_its_own_root() {
        let object = PolicyObject::parse("path:/work/*").unwrap();
        assert!(object.matches(&Resource::path_only(path("/work"))));
        assert!(object.matches(&Resource::path_only(path("/work/acme"))));
        assert!(object.matches(&Resource::path_only(path("/work/acme/notes"))));
        assert!(!object.matches(&Resource::path_only(path("/workshop"))));
        assert!(!object.matches(&Resource::path_only(path("/home"))));
    }

    #[test]
    fn an_exact_object_holds_one_path_only() {
        let object = PolicyObject::parse("path:/work").unwrap();
        assert!(object.matches(&Resource::path_only(path("/work"))));
        assert!(!object.matches(&Resource::path_only(path("/work/acme"))));
    }

    #[test]
    fn a_tag_object_reads_the_resolved_tags() {
        let object = PolicyObject::parse("tag:visibility=public").unwrap();
        let public = Resource::new(
            path("/a"),
            [Tag::parse("visibility=public").unwrap()]
                .into_iter()
                .collect(),
        );
        let private = Resource::new(
            path("/a"),
            [Tag::parse("visibility=private").unwrap()]
                .into_iter()
                .collect(),
        );
        assert!(object.matches(&public));
        assert!(!object.matches(&private));
        assert!(!object.matches(&Resource::path_only(path("/a"))));
    }

    #[test]
    fn no_policy_means_no_access() {
        let filter = AccessFilter::build(&[], Action::Read);
        assert!(filter.is_empty_set());
        assert_eq!(filter.sql(), "0");
        assert!(filter.params().is_empty());
    }

    #[test]
    fn the_root_subtree_needs_no_test() {
        let filter = AccessFilter::build(
            &[rule("path:/*", Action::Read, Effect::Allow)],
            Action::Read,
        );
        assert!(filter.is_unrestricted());
        assert!(filter.params().is_empty());
    }

    #[test]
    fn other_actions_do_not_leak_in() {
        let rules = [
            rule("path:/work/*", Action::Write, Effect::Allow),
            rule("path:/notes/*", Action::Read, Effect::Allow),
        ];
        let filter = AccessFilter::build(&rules, Action::Read);
        assert_eq!(filter.params(), ["/notes", "/notes/*"]);
    }

    #[test]
    fn allows_join_with_or() {
        let rules = [
            rule("path:/notes/*", Action::Read, Effect::Allow),
            rule("tag:visibility=public", Action::Read, Effect::Allow),
        ];
        let filter = AccessFilter::build(&rules, Action::Read);
        assert_eq!(
            filter.sql(),
            "((p.full_path = ? OR p.full_path GLOB ?) OR EXISTS (SELECT 1 FROM effective_fact_tags eft \
             WHERE eft.fact_id = f.id AND eft.key = ? AND eft.value = ?))"
        );
        assert_eq!(
            filter.params(),
            ["/notes", "/notes/*", "visibility", "public"]
        );
    }

    #[test]
    fn a_deny_cuts_a_hole_in_a_wide_allow() {
        let rules = [
            rule("path:/*", Action::Read, Effect::Allow),
            rule("path:/secrets/*", Action::Read, Effect::Deny),
        ];
        let filter = AccessFilter::build(&rules, Action::Read);
        assert_eq!(
            filter.sql(),
            "1 AND NOT ((p.full_path = ? OR p.full_path GLOB ?))"
        );
        assert_eq!(filter.params(), ["/secrets", "/secrets/*"]);
        assert!(!filter.is_unrestricted());
        assert!(!filter.is_empty_set());
    }

    #[test]
    fn a_deny_without_an_allow_still_shows_nothing() {
        let filter = AccessFilter::build(
            &[rule("path:/secrets/*", Action::Read, Effect::Deny)],
            Action::Read,
        );
        assert!(filter.sql().starts_with('0'));
    }

    #[test]
    fn params_follow_the_order_of_the_marks() {
        let rules = [
            rule("tag:a=1", Action::Read, Effect::Allow),
            rule("path:/x/*", Action::Read, Effect::Allow),
            rule("tag:b=2", Action::Read, Effect::Deny),
        ];
        let filter = AccessFilter::build(&rules, Action::Read);
        assert_eq!(filter.sql().matches('?').count(), filter.params().len());
        assert_eq!(filter.params(), ["a", "1", "/x", "/x/*", "b", "2"]);
    }

    #[test]
    fn a_resource_survives_the_trip_through_the_matcher() {
        let resource = Resource::new(
            path("/work/acme"),
            [
                Tag::parse("visibility=private").unwrap(),
                Tag::parse("client=ACME Corp").unwrap(),
            ]
            .into_iter()
            .collect(),
        );
        let request = resource.to_request();
        assert_eq!(Resource::from_request(&request).unwrap(), resource);
    }

    #[test]
    fn a_resource_without_tags_survives_as_well() {
        let resource = Resource::path_only(path("/a/b"));
        assert_eq!(
            Resource::from_request(&resource.to_request()).unwrap(),
            resource
        );
    }

    #[test]
    fn the_matcher_function_agrees_with_the_type() {
        let resource = Resource::new(
            path("/work/acme"),
            [Tag::parse("visibility=public").unwrap()]
                .into_iter()
                .collect(),
        );
        let request = resource.to_request();
        for policy in [
            "path:/work/*",
            "path:/work/acme",
            "path:/home/*",
            "tag:visibility=public",
            "tag:visibility=private",
        ] {
            let object = PolicyObject::parse(policy).unwrap();
            assert_eq!(
                object_matches(&request, policy),
                object.matches(&resource),
                "{policy}"
            );
        }
    }

    #[test]
    fn the_matcher_function_refuses_what_it_cannot_read() {
        assert!(!object_matches("nonsense", "path:/*"));
        assert!(!object_matches(
            &Resource::path_only(path("/a")).to_request(),
            "nonsense"
        ));
    }

    #[test]
    fn reads_and_writes_the_casbin_columns() {
        let fields: Vec<String> = ["cli", "path:/work/*", "read", "allow"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rule = PolicyRule::from_casbin(&fields).unwrap();
        assert_eq!(rule.subject.as_str(), "cli");
        assert_eq!(rule.effect, Effect::Allow);
        assert_eq!(rule.to_casbin().to_vec(), fields);
    }

    #[test]
    fn an_empty_effect_column_means_allow() {
        let fields: Vec<String> = ["cli", "path:/*", "read"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            PolicyRule::from_casbin(&fields).unwrap().effect,
            Effect::Allow
        );
    }
}
