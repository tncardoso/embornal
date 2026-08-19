//! The access guard.
//!
//! The guard holds a Casbin enforcer that reads its policies from the
//! `casbin_rule` table. It answers two questions:
//!
//! - May this subject touch this one resource? The enforcer answers.
//! - Which facts may this subject see? The guard turns the permissions of the
//!   subject into one SQL fragment, so the database drops the rest.
//!
//! The two answers come from the same matcher, [`acl::object_matches`], so
//! they cannot disagree about what a policy covers.

use crate::error::{Error, Result};
use crate::memory::acl::{
    AccessFilter, Action, MODEL, OBJ_MATCH_FN, PolicyRule, Resource, Subject, object_matches,
};
use casbin::function_map::{OperatorFunction, dynamic_to_str};
use casbin::{CoreApi, DefaultModel, Enforcer, MemoryAdapter, MgmtApi, RbacApi};
use rusqlite::Connection;

/// The access guard of one subject.
pub struct Guard {
    subject: Subject,
    enforcer: Enforcer,
    /// The permissions of the subject, roles resolved.
    rules: Vec<PolicyRule>,
}

impl Guard {
    /// Builds the guard from the policies in the database.
    ///
    /// A rule that this build cannot read is dropped with no other effect. An
    /// unreadable rule must not open a door, and it must not shut the tool
    /// down either.
    pub fn load(conn: &Connection, subject: Subject) -> Result<Self> {
        let policies = read_rules(conn, "p")?;
        let groups = read_rules(conn, "g")?;

        let enforcer = block_on(async {
            let model = DefaultModel::from_str(MODEL).await?;
            let mut enforcer = Enforcer::new(model, MemoryAdapter::default()).await?;
            enforcer.add_function(
                OBJ_MATCH_FN,
                OperatorFunction::Arg2(|request, policy| {
                    object_matches(&dynamic_to_str(&request), &dynamic_to_str(&policy)).into()
                }),
            );
            if !policies.is_empty() {
                enforcer.add_named_policies("p", policies).await?;
            }
            if !groups.is_empty() {
                enforcer.add_named_grouping_policies("g", groups).await?;
            }
            Ok::<_, casbin::Error>(enforcer)
        })?;

        let rules = enforcer
            .get_implicit_permissions_for_user(subject.as_str(), None)
            .iter()
            .filter_map(|fields| PolicyRule::from_casbin(fields).ok())
            .collect();

        Ok(Self {
            subject,
            enforcer,
            rules,
        })
    }

    /// Returns the subject that the guard speaks for.
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    /// Returns `true` if the subject may do `action` on `resource`.
    pub fn allows(&self, resource: &Resource, action: Action) -> bool {
        self.enforcer
            .enforce((
                self.subject.as_str(),
                resource.to_request(),
                action.as_str(),
            ))
            .unwrap_or(false)
    }

    /// Returns an error if the subject may not do `action` on `resource`.
    pub fn require(&self, resource: &Resource, action: Action) -> Result<()> {
        if self.allows(resource, action) {
            Ok(())
        } else {
            Err(Error::Denied {
                subject: self.subject.to_string(),
                action,
                path: resource.path.to_string(),
            })
        }
    }

    /// Returns the `WHERE` fragment that keeps the facts that the subject may
    /// touch with `action`.
    pub fn filter(&self, action: Action) -> AccessFilter {
        AccessFilter::build(&self.rules, action)
    }

    /// Returns the permissions of the subject, roles resolved.
    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }
}

/// Reads one kind of rule out of the policy table.
fn read_rules(conn: &Connection, ptype: &str) -> Result<Vec<Vec<String>>> {
    let mut stmt =
        conn.prepare("SELECT v0, v1, v2, v3, v4, v5 FROM casbin_rule WHERE ptype = ? ORDER BY id")?;
    let rows = stmt.query_map([ptype], |row| {
        let mut fields = Vec::with_capacity(6);
        for i in 0..6 {
            let value: String = row.get(i)?;
            fields.push(value);
        }
        // Casbin counts the columns that carry a value, so the empty tail
        // must go.
        while fields.last().is_some_and(String::is_empty) {
            fields.pop();
        }
        Ok(fields)
    })?;

    let mut rules = Vec::new();
    for row in rows {
        rules.push(row?);
    }
    Ok(rules)
}

/// Runs one future to its end on a small runtime.
///
/// Only the setup of the enforcer is asynchronous. `enforce` itself is not, so
/// the rest of the tool stays synchronous.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current thread runtime always builds")
        .block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::memory::Database;
    use crate::memory::path::WikiPath;
    use crate::memory::tag::Tag;

    fn path(s: &str) -> WikiPath {
        WikiPath::parse(s).unwrap()
    }

    fn add(conn: &Connection, fields: &[&str]) {
        let ptype = fields[0];
        let mut values: Vec<String> = fields[1..].iter().map(|s| s.to_string()).collect();
        values.resize(6, String::new());
        conn.execute(
            "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3, v4, v5)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                ptype, values[0], values[1], values[2], values[3], values[4], values[5]
            ],
        )
        .unwrap();
    }

    fn guard_of(db: &Database) -> Guard {
        Guard::load(db.conn(), Subject::cli()).unwrap()
    }

    fn fresh() -> Database {
        Database::open_in_memory(&Config::default()).unwrap()
    }

    #[test]
    fn the_seeded_policy_opens_the_whole_tree() {
        let db = fresh();
        let guard = guard_of(&db);
        for action in Action::ALL {
            assert!(guard.allows(&Resource::path_only(path("/anywhere")), action));
            assert!(guard.filter(action).is_unrestricted());
        }
    }

    #[test]
    fn a_deny_beats_the_seeded_allow() {
        let db = fresh();
        add(
            db.conn(),
            &["p", "default", "path:/secrets/*", "read", "deny"],
        );
        let guard = guard_of(&db);

        assert!(guard.allows(&Resource::path_only(path("/notes")), Action::Read));
        assert!(!guard.allows(&Resource::path_only(path("/secrets/keys")), Action::Read));
        // The deny is about reading only.
        assert!(guard.allows(&Resource::path_only(path("/secrets/keys")), Action::Write));
    }

    #[test]
    fn a_role_carries_its_permissions_to_its_members() {
        let db = fresh();
        db.conn().execute("DELETE FROM casbin_rule", []).unwrap();
        add(
            db.conn(),
            &["p", "reader", "path:/notes/*", "read", "allow"],
        );
        add(db.conn(), &["g", "default", "reader"]);
        let guard = guard_of(&db);

        assert!(guard.allows(&Resource::path_only(path("/notes/a")), Action::Read));
        assert!(!guard.allows(&Resource::path_only(path("/other")), Action::Read));
        assert!(!guard.allows(&Resource::path_only(path("/notes/a")), Action::Write));

        // The filter sees the same permissions as the single check.
        let filter = guard.filter(Action::Read);
        assert_eq!(filter.params(), ["/notes", "/notes/*"]);
    }

    #[test]
    fn an_empty_policy_table_shuts_every_door() {
        let db = fresh();
        db.conn().execute("DELETE FROM casbin_rule", []).unwrap();
        let guard = guard_of(&db);

        assert!(!guard.allows(&Resource::path_only(path("/anything")), Action::Read));
        assert!(guard.filter(Action::Read).is_empty_set());
    }

    #[test]
    fn a_tag_policy_reaches_the_enforcer() {
        let db = fresh();
        db.conn().execute("DELETE FROM casbin_rule", []).unwrap();
        add(
            db.conn(),
            &["p", "default", "tag:visibility=public", "read", "allow"],
        );
        let guard = guard_of(&db);

        let public = Resource::new(
            path("/a"),
            [Tag::parse("visibility=public").unwrap()]
                .into_iter()
                .collect(),
        );
        assert!(guard.allows(&public, Action::Read));
        assert!(!guard.allows(&Resource::path_only(path("/a")), Action::Read));
    }

    #[test]
    fn a_rule_that_nobody_can_read_grants_nothing() {
        let db = fresh();
        db.conn().execute("DELETE FROM casbin_rule", []).unwrap();
        add(
            db.conn(),
            &["p", "default", "nonsense", "read", "allow"],
        );
        let guard = guard_of(&db);

        assert!(!guard.allows(&Resource::path_only(path("/a")), Action::Read));
        assert!(guard.filter(Action::Read).is_empty_set());
    }

    #[test]
    fn the_filter_and_the_single_check_agree() {
        let db = fresh();
        db.conn().execute("DELETE FROM casbin_rule", []).unwrap();
        add(
            db.conn(),
            &["p", "default", "path:/work/*", "read", "allow"],
        );
        add(
            db.conn(),
            &["p", "default", "path:/work/acme/*", "read", "deny"],
        );
        let guard = guard_of(&db);
        let filter = guard.filter(Action::Read);

        // The same question, asked of the enforcer and of the database. The
        // two must never differ, because a difference means that a fact that
        // the guard refuses one by one still comes back from a listing.
        for candidate in [
            "/work",
            "/work/other",
            "/work/acme",
            "/work/acme/deep",
            "/home",
        ] {
            let single = guard.allows(&Resource::path_only(path(candidate)), Action::Read);
            let in_sql = filter_says(&db, &filter, candidate);
            assert_eq!(single, in_sql, "{candidate}");
        }
    }

    /// Asks the database what the fragment says about one path.
    fn filter_says(db: &Database, filter: &AccessFilter, candidate: &str) -> bool {
        // The fragment reads `p.full_path`, so the query gives it a `paths`
        // row that holds the candidate.
        let query = format!(
            "SELECT EXISTS (
                 SELECT 1 FROM (SELECT ? AS full_path, 0 AS id) p
                 WHERE {}
             )",
            filter.sql()
        );
        let mut bound: Vec<&dyn rusqlite::ToSql> = vec![&candidate];
        for value in filter.params() {
            bound.push(value);
        }
        db.conn()
            .query_row(&query, bound.as_slice(), |row| row.get::<_, bool>(0))
            .unwrap()
    }
}
