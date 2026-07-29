//! What the commands do.
//!
//! [`Memory`] joins the database and the guard. Each command of the tool is
//! one method here, so the command line only reads arguments and prints
//! results.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::memory::acl::{AccessFilter, Action, Resource, Subject};
use crate::memory::db::Database;
use crate::memory::fact::{Fact, FactId, NewFact, OrderBy, ScoredFact, Signal};
use crate::memory::guard::Guard;
use crate::memory::path::{PathEntry, PathId, ROOT_ID, WikiPath};
use crate::memory::tag::{Tag, TagKey, TagSet, TagValue};
use crate::memory::time;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use std::path::Path;
use ulid::Ulid;

/// The columns that build a [`Fact`], and the tables that hold them.
const FACT_SELECT: &str = "SELECT f.id, f.ulid, f.path_id, p.full_path, f.content, \
     f.created_at, f.last_recall_at, f.recall_count, f.stability_days, \
     f.supersedes_id, f.deleted_at, f.embedding_model \
     FROM facts f JOIN paths p ON p.id = f.path_id";

/// An open memory.
pub struct Memory {
    db: Database,
    guard: Guard,
    config: Config,
}

/// What `cat` needs to know.
#[derive(Debug, Clone, PartialEq)]
pub struct CatOptions {
    pub order_by: OrderBy,
    pub limit: Option<usize>,
    /// Whether reading the document counts as a recall.
    ///
    /// It does not by default: `cat` hands over each fact of a path at once,
    /// so it says nothing about which fact was useful.
    pub reinforce: bool,
}

impl Default for CatOptions {
    fn default() -> Self {
        Self {
            order_by: OrderBy::Date,
            limit: None,
            reinforce: false,
        }
    }
}

/// What `recall` needs to know.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallOptions {
    pub limit: usize,
    /// Search below this path only.
    pub under: Option<WikiPath>,
    /// Whether a hit counts as a recall. It does, because a hit is the memory
    /// giving an answer that somebody asked for.
    pub reinforce: bool,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            under: None,
            reinforce: true,
        }
    }
}

/// What `tree` needs to know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeOptions {
    /// Shows the paths that hold paths below them, and nothing else.
    pub dirs_only: bool,
}

/// One path of the tree, with everything below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub path: WikiPath,
    /// How many facts the path itself holds.
    pub fact_count: u64,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// Returns the number of paths in the tree, this one counted.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(TreeNode::count).sum::<usize>()
    }

    /// Returns how deep the tree goes. One path alone has depth 0.
    pub fn depth(&self) -> usize {
        self.children
            .iter()
            .map(|child| 1 + child.depth())
            .max()
            .unwrap_or(0)
    }
}

/// What `ls` gives back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The path that the listing is about.
    pub path: WikiPath,
    /// The children, in name order.
    pub children: Vec<PathEntry>,
    /// How many facts the path itself holds.
    pub fact_count: u64,
}

impl Memory {
    /// Opens the memory that the configuration names.
    pub fn open(file: &Path, config: Config) -> Result<Self> {
        let db = Database::open(file, &config)?;
        Self::with_database(db, config)
    }

    /// Opens a memory in RAM. The tests use this.
    pub fn open_in_memory(config: Config) -> Result<Self> {
        let db = Database::open_in_memory(&config)?;
        Self::with_database(db, config)
    }

    fn with_database(db: Database, config: Config) -> Result<Self> {
        let guard = Guard::load(db.conn(), config.subject.clone())?;
        Ok(Self { db, guard, config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn subject(&self) -> &Subject {
        self.guard.subject()
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    // -----------------------------------------------------------------------
    // store
    // -----------------------------------------------------------------------

    /// Writes one fact, and creates the paths that it needs.
    pub fn store(&mut self, request: NewFact) -> Result<Fact> {
        if request.path.is_root() {
            return Err(Error::RootHoldsNoFacts);
        }
        if request.content.trim().is_empty() {
            return Err(Error::EmptyContent);
        }

        // The check reads the tags that the fact will hold: the ones that it
        // takes from the paths above it, and the ones that come with it.
        let mut tags = self.inherited_tags(&request.path)?;
        for tag in &request.tags {
            tags.insert(tag.clone());
        }
        self.guard
            .require(&Resource::new(request.path.clone(), tags), Action::Write)?;

        let now = Utc::now();
        let ulid = Ulid::generate();
        let tx = self.db.conn_mut().transaction()?;

        let path_id = ensure_path(&tx, &request.path, now)?;
        tx.execute(
            "INSERT INTO facts(ulid, path_id, content, created_at, stability_days, supersedes_id)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                ulid.to_string(),
                path_id.0,
                request.content.trim(),
                time::to_sql(now),
                crate::memory::fact::INITIAL_STABILITY_DAYS,
                request.supersedes_id.map(|id| id.0),
            ],
        )?;
        let fact_id = FactId(tx.last_insert_rowid());

        for tag in &request.tags {
            tx.execute(
                "INSERT INTO fact_tags(fact_id, key, value) VALUES (?, ?, ?)",
                params![fact_id.0, tag.key.as_str(), tag.value.as_str()],
            )?;
        }
        tx.commit()?;

        Ok(Fact {
            id: fact_id,
            ulid,
            path_id,
            path: request.path,
            content: request.content.trim().to_string(),
            created_at: now,
            signal: Signal::new(now),
            supersedes_id: request.supersedes_id,
            deleted_at: None,
            embedding_model: None,
        })
    }

    // -----------------------------------------------------------------------
    // ls
    // -----------------------------------------------------------------------

    /// Lists one level below `path`, the way `ls` lists a directory.
    ///
    /// A child that holds facts which the subject may not read does not
    /// appear. A child that holds no fact at all does appear, because it hides
    /// nothing.
    pub fn ls(&self, path: &WikiPath) -> Result<Listing> {
        let path_id = self
            .path_id(path)?
            .ok_or_else(|| Error::PathNotFound(path.to_string()))?;
        let filter = self.guard.filter(Action::Read);

        let fact_count = self.visible_fact_count(path_id, &filter)?;

        let mut children = Vec::new();
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT id, full_path FROM paths WHERE parent_id = ? ORDER BY segment")?;
        let rows = stmt.query_map([path_id.0], |row| {
            Ok((PathId(row.get(0)?), row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (child_id, full_path) = row?;
            let child_path = WikiPath::parse(&full_path)?;

            let visible = self.visible_subtree_fact_count(&child_path, &filter)?;
            let total = self.subtree_fact_count(&child_path)?;
            if visible == 0 && total > 0 {
                continue;
            }

            children.push(PathEntry {
                path: child_path,
                fact_count: self.visible_fact_count(child_id, &filter)?,
                child_count: self.child_count(child_id)?,
            });
        }

        Ok(Listing {
            path: path.clone(),
            children,
            fact_count,
        })
    }

    // -----------------------------------------------------------------------
    // tree
    // -----------------------------------------------------------------------

    /// Reads the whole tree below `path`.
    ///
    /// The walk uses [`Memory::ls`] at each step, so a path that the subject
    /// may not read stays out of the tree, together with everything below it.
    pub fn tree(&self, path: &WikiPath, options: TreeOptions) -> Result<TreeNode> {
        let mut tree = self.walk(path)?;
        if options.dirs_only {
            prune_leaves(&mut tree);
        }
        Ok(tree)
    }

    /// Reads each path below `path`, with no filter.
    fn walk(&self, path: &WikiPath) -> Result<TreeNode> {
        let listing = self.ls(path)?;
        let mut node = TreeNode {
            path: path.clone(),
            fact_count: listing.fact_count,
            children: Vec::with_capacity(listing.children.len()),
        };
        for entry in &listing.children {
            node.children.push(self.walk(&entry.path)?);
        }
        Ok(node)
    }

    // -----------------------------------------------------------------------
    // cat
    // -----------------------------------------------------------------------

    /// Builds the document of one path.
    pub fn cat(&mut self, path: &WikiPath, options: CatOptions) -> Result<Vec<Fact>> {
        let path_id = self
            .path_id(path)?
            .ok_or_else(|| Error::PathNotFound(path.to_string()))?;
        let filter = self.guard.filter(Action::Read);
        if filter.is_empty_set() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "{FACT_SELECT} WHERE f.path_id = ? AND f.deleted_at IS NULL AND ({})
             ORDER BY f.created_at, f.id",
            filter.sql()
        );
        let mut bound: Vec<&dyn ToSql> = vec![&path_id.0];
        for value in filter.params() {
            bound.push(value);
        }

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), fact_from_row)?;
        let mut facts: Vec<Fact> = rows.collect::<rusqlite::Result<_>>()?;

        let now = Utc::now();
        if options.order_by == OrderBy::Signal {
            facts.sort_by(|a, b| {
                b.signal
                    .strength_at(now)
                    .total_cmp(&a.signal.strength_at(now))
                    .then_with(|| b.created_at.cmp(&a.created_at))
            });
        }
        if let Some(limit) = options.limit {
            facts.truncate(limit);
        }

        if options.reinforce {
            drop(stmt);
            self.reinforce(&mut facts, now)?;
        }
        Ok(facts)
    }

    // -----------------------------------------------------------------------
    // recall
    // -----------------------------------------------------------------------

    /// Searches the memory.
    ///
    /// With a query, the keyword index answers and the strength of each fact
    /// moves it up or down. With no query, the strongest facts come back.
    pub fn recall(
        &mut self,
        query: Option<&str>,
        options: RecallOptions,
    ) -> Result<Vec<ScoredFact>> {
        let filter = self.guard.filter(Action::Read);
        if filter.is_empty_set() {
            return Ok(Vec::new());
        }

        let now = Utc::now();
        let mut scored = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(text) => self.search(text, &filter, &options, now)?,
            None => self.strongest(&filter, &options, now)?,
        };

        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(options.limit);

        if options.reinforce {
            let mut facts: Vec<Fact> = scored.iter().map(|s| s.fact.clone()).collect();
            self.reinforce(&mut facts, now)?;
            for (entry, fact) in scored.iter_mut().zip(facts) {
                entry.fact = fact;
            }
        }
        Ok(scored)
    }

    /// Runs the keyword index and mixes its answer with the strength.
    fn search(
        &self,
        query: &str,
        filter: &AccessFilter,
        options: &RecallOptions,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScoredFact>> {
        let expression = fts_query(query);
        if expression.is_empty() {
            return Ok(Vec::new());
        }

        // Take more candidates than the caller wants, because the mix with the
        // strength changes the order.
        let candidates = (options.limit * 4).max(50) as i64;
        let (subtree, subtree_params) = subtree_clause(options.under.as_ref());

        let sql = format!(
            "SELECT f.id, f.ulid, f.path_id, p.full_path, f.content, f.created_at,
                    f.last_recall_at, f.recall_count, f.stability_days,
                    f.supersedes_id, f.deleted_at, f.embedding_model,
                    bm25(facts_fts) AS rank
             FROM facts_fts
             JOIN facts f ON f.id = facts_fts.rowid
             JOIN paths p ON p.id = f.path_id
             WHERE facts_fts MATCH ? AND f.deleted_at IS NULL AND ({}){subtree}
             ORDER BY rank
             LIMIT ?",
            filter.sql()
        );

        let mut bound: Vec<&dyn ToSql> = vec![&expression];
        for value in filter.params() {
            bound.push(value);
        }
        for value in &subtree_params {
            bound.push(value);
        }
        bound.push(&candidates);

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), |row| {
            let fact = fact_from_row(row)?;
            let rank: f64 = row.get(12)?;
            Ok((fact, rank))
        })?;

        // bm25 gives a negative number, and the best match is the smallest.
        let hits: Vec<(Fact, f64)> = rows.collect::<rusqlite::Result<_>>()?;
        let best = hits.iter().map(|(_, rank)| *rank).fold(f64::MAX, f64::min);
        let worst = hits.iter().map(|(_, rank)| *rank).fold(f64::MIN, f64::max);
        let spread = (worst - best).abs();

        let weights = &self.config.recall;
        Ok(hits
            .into_iter()
            .map(|(fact, rank)| {
                // Map the best match to 1.0 and the worst to 0.0.
                let keyword = if spread < f64::EPSILON {
                    1.0
                } else {
                    (worst - rank) / spread
                };
                let strength = fact.signal.strength_at(now);
                ScoredFact {
                    score: weights.keyword_weight * keyword + weights.signal_weight * strength,
                    fact,
                    keyword_score: Some(keyword),
                    vector_score: None,
                    signal_strength: strength,
                }
            })
            .collect())
    }

    /// Returns the facts that are still strongest.
    fn strongest(
        &self,
        filter: &AccessFilter,
        options: &RecallOptions,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScoredFact>> {
        let (subtree, subtree_params) = subtree_clause(options.under.as_ref());
        let sql = format!(
            "{FACT_SELECT} WHERE f.deleted_at IS NULL AND ({}){subtree}",
            filter.sql()
        );

        let mut bound: Vec<&dyn ToSql> = Vec::new();
        for value in filter.params() {
            bound.push(value);
        }
        for value in &subtree_params {
            bound.push(value);
        }

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), fact_from_row)?;
        let facts: Vec<Fact> = rows.collect::<rusqlite::Result<_>>()?;

        Ok(facts
            .into_iter()
            .map(|fact| {
                let strength = fact.signal.strength_at(now);
                ScoredFact {
                    score: strength,
                    fact,
                    keyword_score: None,
                    vector_score: None,
                    signal_strength: strength,
                }
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // Shared work
    // -----------------------------------------------------------------------

    /// Writes one recall on each fact and updates the copies in memory.
    fn reinforce(&mut self, facts: &mut [Fact], now: DateTime<Utc>) -> Result<()> {
        if facts.is_empty() {
            return Ok(());
        }
        let tx = self.db.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE facts SET last_recall_at = ?, recall_count = ?, stability_days = ?
                 WHERE id = ?",
            )?;
            for fact in facts.iter_mut() {
                let signal = fact.signal.reinforce(now);
                stmt.execute(params![
                    time::to_sql(now),
                    signal.recall_count,
                    signal.stability_days,
                    fact.id.0
                ])?;
                fact.signal = signal;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns the tags that a fact of `path` takes from the paths above it.
    pub fn inherited_tags(&self, path: &WikiPath) -> Result<TagSet> {
        let mut tags = TagSet::new();
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT key, value FROM path_tags WHERE path_id = ?")?;

        // The chain runs from the root down, so a deeper path overwrites the
        // value that it inherits.
        for ancestor in path.ancestry() {
            let Some(id) = self.path_id(&ancestor)? else {
                continue;
            };
            let rows = stmt.query_map([id.0], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (key, value) = row?;
                tags.insert(Tag::new(TagKey::parse(&key)?, TagValue::parse(&value)?));
            }
        }
        Ok(tags)
    }

    /// Returns the tags that decide access for one fact.
    pub fn effective_tags(&self, fact: FactId) -> Result<TagSet> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT key, value FROM effective_fact_tags WHERE fact_id = ?")?;
        let rows = stmt.query_map([fact.0], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut tags = TagSet::new();
        for row in rows {
            let (key, value) = row?;
            tags.insert(Tag::new(TagKey::parse(&key)?, TagValue::parse(&value)?));
        }
        Ok(tags)
    }

    /// Returns the row id of a path, if the path exists.
    pub fn path_id(&self, path: &WikiPath) -> Result<Option<PathId>> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT id FROM paths WHERE full_path = ?",
                [path.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(PathId))
    }

    fn child_count(&self, path_id: PathId) -> Result<u64> {
        Ok(self.db.conn().query_row(
            "SELECT COUNT(*) FROM paths WHERE parent_id = ?",
            [path_id.0],
            |row| row.get::<_, i64>(0),
        )? as u64)
    }

    fn visible_fact_count(&self, path_id: PathId, filter: &AccessFilter) -> Result<u64> {
        if filter.is_empty_set() {
            return Ok(0);
        }
        let sql = format!(
            "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id
             WHERE f.path_id = ? AND f.deleted_at IS NULL AND ({})",
            filter.sql()
        );
        let mut bound: Vec<&dyn ToSql> = vec![&path_id.0];
        for value in filter.params() {
            bound.push(value);
        }
        Ok(self
            .db
            .conn()
            .query_row(&sql, bound.as_slice(), |row| row.get::<_, i64>(0))? as u64)
    }

    fn visible_subtree_fact_count(&self, path: &WikiPath, filter: &AccessFilter) -> Result<u64> {
        if filter.is_empty_set() {
            return Ok(0);
        }
        let sql = format!(
            "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id
             WHERE f.deleted_at IS NULL AND (p.full_path = ? OR p.full_path GLOB ?) AND ({})",
            filter.sql()
        );
        let full = path.to_string();
        let glob = path.subtree_glob();
        let mut bound: Vec<&dyn ToSql> = vec![&full, &glob];
        for value in filter.params() {
            bound.push(value);
        }
        Ok(self
            .db
            .conn()
            .query_row(&sql, bound.as_slice(), |row| row.get::<_, i64>(0))? as u64)
    }

    fn subtree_fact_count(&self, path: &WikiPath) -> Result<u64> {
        Ok(self.db.conn().query_row(
            "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id
             WHERE f.deleted_at IS NULL AND (p.full_path = ? OR p.full_path GLOB ?)",
            params![path.as_str(), path.subtree_glob()],
            |row| row.get::<_, i64>(0),
        )? as u64)
    }
}

/// Drops each path that holds no path below it.
///
/// The test reads the tree as it was read from the database. A branch whose
/// only child is a leaf therefore stays: it holds a path, and the fact that
/// this path leaves the tree does not turn its parent into a leaf.
fn prune_leaves(node: &mut TreeNode) {
    node.children.retain(|child| !child.children.is_empty());
    for child in &mut node.children {
        prune_leaves(child);
    }
}

/// Creates the path and every path above it that is still absent.
fn ensure_path(tx: &Connection, path: &WikiPath, now: DateTime<Utc>) -> Result<PathId> {
    let mut parent = ROOT_ID;
    let stamp = time::to_sql(now);

    for step in path.ancestry() {
        if step.is_root() {
            continue;
        }
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM paths WHERE full_path = ?",
                [step.as_str()],
                |row| row.get(0),
            )
            .optional()?;

        parent = match existing {
            Some(id) => PathId(id),
            None => {
                tx.execute(
                    "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
                     VALUES (?, ?, ?, ?, ?)",
                    params![
                        Ulid::generate().to_string(),
                        parent.0,
                        step.segment().expect("the step is not the root"),
                        step.as_str(),
                        stamp
                    ],
                )?;
                PathId(tx.last_insert_rowid())
            }
        };
    }
    Ok(parent)
}

/// Builds a [`Fact`] out of the columns of [`FACT_SELECT`].
fn fact_from_row(row: &Row<'_>) -> rusqlite::Result<Fact> {
    let created_at = read_time(row, 5)?;
    let last_recall_at = read_optional_time(row, 6)?;
    Ok(Fact {
        id: FactId(row.get(0)?),
        ulid: row
            .get::<_, String>(1)?
            .parse()
            .unwrap_or_else(|_| Ulid::nil()),
        path_id: PathId(row.get(2)?),
        path: WikiPath::parse(&row.get::<_, String>(3)?).map_err(to_sql_error)?,
        content: row.get(4)?,
        created_at,
        signal: Signal::from_parts(created_at, last_recall_at, row.get(7)?, row.get(8)?),
        supersedes_id: row.get::<_, Option<i64>>(9)?.map(FactId),
        deleted_at: read_optional_time(row, 10)?,
        embedding_model: row.get(11)?,
    })
}

fn read_time(row: &Row<'_>, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let text: String = row.get(index)?;
    time::from_sql(&text).map_err(to_sql_error)
}

fn read_optional_time(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    match row.get::<_, Option<String>>(index)? {
        Some(text) => Ok(Some(time::from_sql(&text).map_err(to_sql_error)?)),
        None => Ok(None),
    }
}

fn to_sql_error<E: std::error::Error + Send + Sync + 'static>(err: E) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
}

/// Builds the clause that holds a search inside one subtree.
fn subtree_clause(under: Option<&WikiPath>) -> (String, Vec<String>) {
    match under {
        Some(path) if path.is_root() => (String::new(), Vec::new()),
        Some(path) => (
            " AND (p.full_path = ? OR p.full_path GLOB ?)".to_string(),
            vec![path.to_string(), path.subtree_glob()],
        ),
        None => (String::new(), Vec::new()),
    }
}

/// Turns what a user typed into an FTS5 expression.
///
/// Each word becomes a quoted term, and the terms join with `OR`. The quotes
/// stop the FTS5 syntax from reading a word such as `NOT` or `a-b` as an
/// operator.
pub fn fts_query(input: &str) -> String {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::path::MEMORY_PATH;

    fn path(s: &str) -> WikiPath {
        WikiPath::parse(s).unwrap()
    }

    fn memory() -> Memory {
        Memory::open_in_memory(Config::default()).unwrap()
    }

    fn write(memory: &mut Memory, at: &str, content: &str) -> Fact {
        memory
            .store(NewFact {
                path: path(at),
                content: content.to_string(),
                tags: Vec::new(),
                supersedes_id: None,
            })
            .unwrap()
    }

    // -- store --------------------------------------------------------------

    #[test]
    fn a_store_creates_the_missing_paths() {
        let mut memory = memory();
        let fact = write(&mut memory, "/projects/embornal/design", "It uses SQLite.");

        assert_eq!(fact.path.as_str(), "/projects/embornal/design");
        for step in [
            "/projects",
            "/projects/embornal",
            "/projects/embornal/design",
        ] {
            assert!(memory.path_id(&path(step)).unwrap().is_some(), "{step}");
        }
    }

    #[test]
    fn a_second_store_reuses_the_paths() {
        let mut memory = memory();
        write(&mut memory, "/a/b", "one");
        write(&mut memory, "/a/c", "two");

        let count: i64 = memory
            .database()
            .conn()
            .query_row("SELECT COUNT(*) FROM paths", [], |row| row.get(0))
            .unwrap();
        // root, /memory, /a, /a/b, /a/c
        assert_eq!(count, 5);
    }

    #[test]
    fn a_store_folds_the_path() {
        let mut memory = memory();
        let fact = write(&mut memory, "/Projects/Embornal", "one");
        assert_eq!(fact.path.as_str(), "/projects/embornal");
    }

    #[test]
    fn the_root_holds_no_facts() {
        let mut memory = memory();
        let result = memory.store(NewFact {
            path: WikiPath::root(),
            content: "nowhere".to_string(),
            tags: Vec::new(),
            supersedes_id: None,
        });
        assert!(matches!(result, Err(Error::RootHoldsNoFacts)));
    }

    #[test]
    fn a_fact_needs_content() {
        let mut memory = memory();
        let result = memory.store(NewFact {
            path: path("/a"),
            content: "   ".to_string(),
            tags: Vec::new(),
            supersedes_id: None,
        });
        assert!(matches!(result, Err(Error::EmptyContent)));
    }

    #[test]
    fn a_store_writes_the_tags() {
        let mut memory = memory();
        let fact = memory
            .store(NewFact {
                path: path("/work"),
                content: "one".to_string(),
                tags: vec![Tag::parse("visibility=private").unwrap()],
                supersedes_id: None,
            })
            .unwrap();

        let tags = memory.effective_tags(fact.id).unwrap();
        assert!(tags.matches(&Tag::parse("visibility=private").unwrap()));
    }

    #[test]
    fn a_write_that_the_policy_refuses_stops() {
        let mut memory = memory();
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
                 VALUES ('p', 'cli', 'path:/locked/*', 'write', 'deny')",
                [],
            )
            .unwrap();
        memory.guard = Guard::load(memory.db.conn(), Subject::cli()).unwrap();

        let result = memory.store(NewFact {
            path: path("/locked/notes"),
            content: "one".to_string(),
            tags: Vec::new(),
            supersedes_id: None,
        });
        assert!(matches!(result, Err(Error::Denied { .. })));
        // Nothing was written, not even the path.
        assert!(memory.path_id(&path("/locked")).unwrap().is_none());
    }

    // -- ls -----------------------------------------------------------------

    #[test]
    fn ls_shows_one_level_only() {
        let mut memory = memory();
        write(&mut memory, "/a/b/c", "deep");
        write(&mut memory, "/a/d", "shallow");

        let listing = memory.ls(&path("/a")).unwrap();
        let names: Vec<String> = listing
            .children
            .iter()
            .map(|entry| entry.path.to_string())
            .collect();
        assert_eq!(names, ["/a/b", "/a/d"]);
    }

    #[test]
    fn ls_reports_content_and_children() {
        let mut memory = memory();
        write(&mut memory, "/a", "the path itself holds this");
        write(&mut memory, "/a/b", "one");

        let listing = memory.ls(&WikiPath::root()).unwrap();
        let entry = listing
            .children
            .iter()
            .find(|entry| entry.path.as_str() == "/a")
            .unwrap();
        assert!(entry.has_content());
        assert_eq!(entry.fact_count, 1);
        assert_eq!(entry.child_count, 1);
    }

    #[test]
    fn ls_of_the_root_finds_the_memory_path() {
        let memory = memory();
        let listing = memory.ls(&WikiPath::root()).unwrap();
        assert!(
            listing
                .children
                .iter()
                .any(|entry| entry.path.as_str() == MEMORY_PATH)
        );
    }

    #[test]
    fn ls_of_a_path_that_is_absent_says_so() {
        let memory = memory();
        assert!(matches!(
            memory.ls(&path("/nowhere")),
            Err(Error::PathNotFound(_))
        ));
    }

    #[test]
    fn ls_hides_a_child_whose_facts_are_all_refused() {
        let mut memory = memory();
        write(&mut memory, "/open/a", "visible");
        write(&mut memory, "/secret/a", "hidden");
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
                 VALUES ('p', 'cli', 'path:/secret/*', 'read', 'deny')",
                [],
            )
            .unwrap();
        memory.guard = Guard::load(memory.db.conn(), Subject::cli()).unwrap();

        let names: Vec<String> = memory
            .ls(&WikiPath::root())
            .unwrap()
            .children
            .iter()
            .map(|entry| entry.path.to_string())
            .collect();
        assert!(names.contains(&"/open".to_string()));
        assert!(!names.contains(&"/secret".to_string()));
    }

    // -- tree ---------------------------------------------------------------

    #[test]
    fn a_tree_holds_every_level() {
        let mut memory = memory();
        write(&mut memory, "/a/b/c", "deep");
        write(&mut memory, "/a/d", "shallow");

        let tree = memory.tree(&path("/a"), TreeOptions::default()).unwrap();
        assert_eq!(tree.path.as_str(), "/a");
        assert_eq!(tree.depth(), 2);

        let names: Vec<String> = tree.children.iter().map(|c| c.path.to_string()).collect();
        assert_eq!(names, ["/a/b", "/a/d"]);
        assert_eq!(tree.children[0].children[0].path.as_str(), "/a/b/c");
    }

    #[test]
    fn a_tree_carries_the_number_of_facts() {
        let mut memory = memory();
        write(&mut memory, "/a", "one");
        write(&mut memory, "/a", "two");
        write(&mut memory, "/a/b", "three");

        let tree = memory.tree(&path("/a"), TreeOptions::default()).unwrap();
        assert_eq!(tree.fact_count, 2);
        assert_eq!(tree.children[0].fact_count, 1);
    }

    #[test]
    fn a_tree_of_a_leaf_holds_the_leaf_alone() {
        let mut memory = memory();
        write(&mut memory, "/a", "one");

        let tree = memory.tree(&path("/a"), TreeOptions::default()).unwrap();
        assert!(tree.children.is_empty());
        assert_eq!(tree.count(), 1);
        assert_eq!(tree.depth(), 0);
    }

    #[test]
    fn dirs_only_drops_the_paths_that_hold_no_path() {
        let mut memory = memory();
        write(&mut memory, "/a/branch/leaf", "deep");
        write(&mut memory, "/a/leaf", "shallow");

        let tree = memory
            .tree(&path("/a"), TreeOptions { dirs_only: true })
            .unwrap();
        let names: Vec<String> = tree.children.iter().map(|c| c.path.to_string()).collect();
        // /a/branch stays because it holds a path; /a/leaf goes.
        assert_eq!(names, ["/a/branch"]);
        // The leaf below the branch goes as well.
        assert!(tree.children[0].children.is_empty());
    }

    #[test]
    fn dirs_only_keeps_a_deep_branch() {
        let mut memory = memory();
        write(&mut memory, "/a/b/c/d", "deep");

        let tree = memory
            .tree(&path("/a"), TreeOptions { dirs_only: true })
            .unwrap();
        // /a -> /a/b -> /a/b/c, and /a/b/c/d is the leaf that goes.
        assert_eq!(tree.depth(), 2);
        assert_eq!(tree.count(), 3);
    }

    #[test]
    fn a_tree_of_a_path_that_is_absent_says_so() {
        let memory = memory();
        assert!(matches!(
            memory.tree(&path("/nowhere"), TreeOptions::default()),
            Err(Error::PathNotFound(_))
        ));
    }

    #[test]
    fn a_tree_hides_what_the_policy_refuses() {
        let mut memory = memory();
        write(&mut memory, "/a/open", "visible");
        write(&mut memory, "/a/secret/deep", "hidden");
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
                 VALUES ('p', 'cli', 'path:/a/secret/*', 'read', 'deny')",
                [],
            )
            .unwrap();
        memory.guard = Guard::load(memory.db.conn(), Subject::cli()).unwrap();

        let tree = memory.tree(&path("/a"), TreeOptions::default()).unwrap();
        let names: Vec<String> = tree.children.iter().map(|c| c.path.to_string()).collect();
        assert_eq!(names, ["/a/open"]);
    }

    // -- cat ----------------------------------------------------------------

    #[test]
    fn cat_reads_the_facts_oldest_first() {
        let mut memory = memory();
        for content in ["first", "second", "third"] {
            write(&mut memory, "/notes", content);
        }
        let facts = memory.cat(&path("/notes"), CatOptions::default()).unwrap();
        let text: Vec<&str> = facts.iter().map(|f| f.content.as_str()).collect();
        assert_eq!(text, ["first", "second", "third"]);
    }

    #[test]
    fn cat_limits_the_document() {
        let mut memory = memory();
        for content in ["a", "b", "c"] {
            write(&mut memory, "/notes", content);
        }
        let facts = memory
            .cat(
                &path("/notes"),
                CatOptions {
                    limit: Some(2),
                    ..CatOptions::default()
                },
            )
            .unwrap();
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn cat_sorts_by_signal_when_it_is_asked_to() {
        let mut memory = memory();
        write(&mut memory, "/notes", "weak");
        let strong = write(&mut memory, "/notes", "strong");

        // A recall lifts the second fact above the first.
        memory
            .database()
            .conn()
            .execute(
                "UPDATE facts SET stability_days = 500 WHERE id = ?",
                [strong.id.0],
            )
            .unwrap();
        memory
            .database()
            .conn()
            .execute(
                "UPDATE facts SET created_at = '2020-01-01T00:00:00.000000Z'",
                [],
            )
            .unwrap();

        let facts = memory
            .cat(
                &path("/notes"),
                CatOptions {
                    order_by: OrderBy::Signal,
                    ..CatOptions::default()
                },
            )
            .unwrap();
        assert_eq!(facts[0].content, "strong");
    }

    #[test]
    fn cat_does_not_count_as_a_recall() {
        let mut memory = memory();
        let fact = write(&mut memory, "/notes", "one");
        memory.cat(&path("/notes"), CatOptions::default()).unwrap();

        let count: i64 = memory
            .database()
            .conn()
            .query_row(
                "SELECT recall_count FROM facts WHERE id = ?",
                [fact.id.0],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn cat_counts_as_a_recall_when_it_is_asked_to() {
        let mut memory = memory();
        let fact = write(&mut memory, "/notes", "one");
        memory
            .cat(
                &path("/notes"),
                CatOptions {
                    reinforce: true,
                    ..CatOptions::default()
                },
            )
            .unwrap();

        let count: i64 = memory
            .database()
            .conn()
            .query_row(
                "SELECT recall_count FROM facts WHERE id = ?",
                [fact.id.0],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // -- recall -------------------------------------------------------------

    #[test]
    fn recall_finds_a_fact_by_a_word() {
        let mut memory = memory();
        write(&mut memory, "/db", "The memory uses SQLite for storage.");
        write(&mut memory, "/lang", "The tool is written in Rust.");

        let hits = memory
            .recall(Some("sqlite"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].fact.content.contains("SQLite"));
        assert!(hits[0].keyword_score.is_some());
    }

    #[test]
    fn recall_ignores_the_case_and_the_accents() {
        let mut memory = memory();
        write(&mut memory, "/pt", "A memória usa índices");

        for query in ["memória", "memoria", "MEMORIA"] {
            let hits = memory
                .recall(Some(query), RecallOptions::default())
                .unwrap();
            assert_eq!(hits.len(), 1, "{query}");
        }
    }

    #[test]
    fn recall_reads_more_than_one_word() {
        let mut memory = memory();
        write(&mut memory, "/a", "alpha beta");
        write(&mut memory, "/b", "beta gamma");
        write(&mut memory, "/c", "nothing here");

        let hits = memory
            .recall(Some("alpha gamma"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn recall_survives_the_fts_operators() {
        let mut memory = memory();
        write(&mut memory, "/a", "a fact about NEAR and OR");

        for query in ["NEAR", "OR", "\"", "a AND b", "*", "(unbalanced"] {
            assert!(
                memory.recall(Some(query), RecallOptions::default()).is_ok(),
                "{query}"
            );
        }
    }

    #[test]
    fn a_recall_with_no_query_gives_the_strongest_facts() {
        let mut memory = memory();
        let hits = memory.recall(None, RecallOptions::default()).unwrap();
        // The seeded facts of /memory come back.
        assert!(!hits.is_empty());
        assert!(hits[0].keyword_score.is_none());
        // The order runs from strong to weak.
        for pair in hits.windows(2) {
            assert!(pair[0].score >= pair[1].score);
        }
    }

    #[test]
    fn a_recall_counts_as_a_recall() {
        let mut memory = memory();
        let fact = write(&mut memory, "/db", "SQLite holds the memory.");
        memory
            .recall(Some("sqlite"), RecallOptions::default())
            .unwrap();

        let (count, stability): (i64, f64) = memory
            .database()
            .conn()
            .query_row(
                "SELECT recall_count, stability_days FROM facts WHERE id = ?",
                [fact.id.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert!(stability >= crate::memory::fact::INITIAL_STABILITY_DAYS);
    }

    #[test]
    fn recall_stays_under_the_path_that_it_is_given() {
        let mut memory = memory();
        write(&mut memory, "/work/notes", "the same word here");
        write(&mut memory, "/home/notes", "the same word here");

        let hits = memory
            .recall(
                Some("word"),
                RecallOptions {
                    under: Some(path("/work")),
                    ..RecallOptions::default()
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].fact.path.is_under(&path("/work")));
    }

    #[test]
    fn recall_holds_the_limit() {
        let mut memory = memory();
        for i in 0..10 {
            write(&mut memory, "/many", &format!("fact number {i} about rust"));
        }
        let hits = memory
            .recall(
                Some("rust"),
                RecallOptions {
                    limit: 3,
                    ..RecallOptions::default()
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn recall_does_not_return_what_the_policy_refuses() {
        let mut memory = memory();
        write(&mut memory, "/open", "rust is the language");
        write(&mut memory, "/secret", "rust is the language");
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
                 VALUES ('p', 'cli', 'path:/secret/*', 'read', 'deny')",
                [],
            )
            .unwrap();
        memory.guard = Guard::load(memory.db.conn(), Subject::cli()).unwrap();

        let hits = memory
            .recall(Some("rust"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].fact.path.as_str(), "/open");
    }

    #[test]
    fn a_deleted_fact_does_not_come_back() {
        let mut memory = memory();
        let fact = write(&mut memory, "/notes", "a fact about rust");
        memory
            .database()
            .conn()
            .execute(
                "UPDATE facts SET deleted_at = '2026-01-01T00:00:00.000000Z' WHERE id = ?",
                [fact.id.0],
            )
            .unwrap();

        assert!(
            memory
                .recall(Some("rust"), RecallOptions::default())
                .unwrap()
                .is_empty()
        );
        assert!(
            memory
                .cat(&path("/notes"), CatOptions::default())
                .unwrap()
                .is_empty()
        );
    }

    // -- the FTS expression -------------------------------------------------

    #[test]
    fn builds_a_safe_fts_expression() {
        assert_eq!(fts_query("hello world"), "\"hello\" OR \"world\"");
        assert_eq!(fts_query("a-b"), "\"a\" OR \"b\"");
        assert_eq!(fts_query("NEAR"), "\"NEAR\"");
        assert_eq!(fts_query("  "), "");
        assert_eq!(fts_query("\"quoted\""), "\"quoted\"");
    }
}
