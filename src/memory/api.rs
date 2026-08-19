//! What the commands do.
//!
//! [`Memory`] joins the database and the guard. Each command of the tool is
//! one method here, so the command line only reads arguments and prints
//! results.

use crate::config::{Config, Paths};
use crate::embedding::{self, Embedder, Input, Provider};
use crate::error::{Error, Result};
use crate::memory::acl::{AccessFilter, Action, OWNER_KEY, Resource, Subject};
use crate::memory::db::{Database, VEC_TABLE};
use crate::memory::fact::{Fact, FactId, NewFact, OrderBy, ScoredFact, Signal};
use crate::memory::guard::Guard;
use crate::memory::path::{PathEntry, PathId, ROOT_ID, WikiPath};
use crate::memory::tag::{Tag, TagKey, TagSet, TagValue};
use crate::memory::time;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The columns that build a [`Fact`], and the tables that hold them.
const FACT_SELECT: &str = "SELECT f.id, f.ulid, f.path_id, p.full_path, f.content, \
     f.created_at, f.last_recall_at, f.recall_count, f.stability_days, \
     f.supersedes_id, f.deleted_at, f.embedding_model, f.owner \
     FROM facts f JOIN paths p ON p.id = f.path_id";

/// An open memory.
pub struct Memory {
    db: Database,
    guard: Guard,
    config: Config,
    /// What turns text into vectors. It loads its weights on the first call
    /// that needs them, and never for `ls`, `cat` or `tree`.
    embedder: Provider,
}

/// What `cat` needs to know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// How many facts go to the model at one time during a backfill.
const REINDEX_GROUP: usize = 64;

/// What `reindex` needs to know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReindexOptions {
    /// Stops after this many facts.
    pub limit: Option<usize>,
    /// Writes the vector of every fact again, not only of the facts that have
    /// none. Use this after a change of model.
    pub all: bool,
}

/// What `reindex` did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReindexReport {
    /// Whether this memory has an embedding model at all.
    ///
    /// Without one, the queue stays as it is. The count below then says how
    /// much work waits for a model.
    pub has_model: bool,
    /// How many facts had no vector.
    pub pending: usize,
    /// How many of them have one now.
    pub done: usize,
    /// The model that wrote them. `None` when nothing needed a vector.
    pub model: Option<String>,
}

/// What `tree` needs to know.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeOptions {
    /// Shows the paths that hold paths below them, and nothing else.
    pub dirs_only: bool,
}

/// One path of the tree, with everything below it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// The path that the listing is about.
    pub path: WikiPath,
    /// The children, in name order.
    pub children: Vec<PathEntry>,
    /// How many facts the path itself holds.
    pub fact_count: u64,
    /// How many visible facts the path and all paths below it hold.
    pub subtree_fact_count: u64,
}

impl Memory {
    /// Opens the memory of a home directory.
    ///
    /// The home holds the database and the weights of the embedding model, so
    /// the two arrive together.
    pub fn open(paths: &Paths, config: Config) -> Result<Self> {
        let db = Database::open(&config.database_file(paths), &config)?;
        let embedder = Provider::from_config(&config, paths)?;
        Self::with_database(db, config, embedder)
    }

    /// Opens a memory in RAM.
    ///
    /// The memory runs without vectors. The tests use this, so that they
    /// reach for no weights. Use [`Memory::with_embedder`] to give it one.
    pub fn open_in_memory(config: Config) -> Result<Self> {
        let db = Database::open_in_memory(&config)?;
        Self::with_database(db, config, Provider::off())
    }

    /// Puts an embedder in a memory that has none. The tests use this.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Provider::ready(embedder);
        self
    }

    fn with_database(db: Database, config: Config, embedder: Provider) -> Result<Self> {
        let guard = Guard::load(db.conn(), config.subject.clone())?;
        Ok(Self {
            db,
            guard,
            config,
            embedder,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn subject(&self) -> &Subject {
        self.guard.subject()
    }

    /// Points this memory at another subject.
    ///
    /// It reads the policies again, so that the guard speaks for the new
    /// subject and for nobody else. A server calls this for each request,
    /// because one memory answers many people but one request has one caller.
    pub fn set_subject(&mut self, subject: Subject) -> Result<()> {
        if self.guard.subject() == &subject {
            return Ok(());
        }
        self.guard = Guard::load(self.db.conn(), subject)?;
        Ok(())
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

        // Nobody names the owner of a fact but the memory. A writer that
        // could name it would take the facts of another subject, because the
        // access rules read that tag.
        if let Some(tag) = request
            .tags
            .iter()
            .find(|tag| tag.key.as_str() == OWNER_KEY)
        {
            return Err(Error::ReservedTag(tag.key.to_string()));
        }
        let owner = self.subject().clone();

        // The check reads the tags that the fact will hold: the ones that it
        // takes from the paths above it, and the ones that come with it.
        let mut tags = self.inherited_tags(&request.path)?;
        for tag in &request.tags {
            tags.insert(tag.clone());
        }
        // The owner goes in last, so that a tag of a path above cannot give
        // the fact away.
        tags.insert(owner.owner_tag());
        self.guard
            .require(&Resource::new(request.path.clone(), tags), Action::Write)?;

        let now = Utc::now();
        let ulid = Ulid::generate();
        let tx = self.db.conn_mut().transaction()?;

        let path_id = ensure_path(&tx, &request.path, now)?;
        tx.execute(
            "INSERT INTO facts(ulid, path_id, content, created_at, stability_days, supersedes_id, owner)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                ulid.to_string(),
                path_id.0,
                request.content.trim(),
                time::to_sql(now),
                crate::memory::fact::INITIAL_STABILITY_DAYS,
                request.supersedes_id.map(|id| id.0),
                owner.as_str(),
            ],
        )?;
        let fact_id = FactId(tx.last_insert_rowid());

        // The owner column is the truth; this row is the same name in the
        // form that the access rules read.
        let owner_tag = owner.owner_tag();
        for tag in request.tags.iter().chain(std::iter::once(&owner_tag)) {
            tx.execute(
                "INSERT INTO fact_tags(fact_id, key, value) VALUES (?, ?, ?)",
                params![fact_id.0, tag.key.as_str(), tag.value.as_str()],
            )?;
        }
        tx.commit()?;

        let mut fact = Fact {
            id: fact_id,
            ulid,
            path_id,
            path: request.path,
            content: request.content.trim().to_string(),
            owner: owner.to_string(),
            created_at: now,
            signal: Signal::new(now),
            supersedes_id: request.supersedes_id,
            deleted_at: None,
            embedding_model: None,
        };

        // The fact is already written. A model that fails must not take it
        // away, so the failure is a warning and the fact waits in the queue
        // that `reindex` reads.
        match self.embed_fact(&fact) {
            Ok(model) => fact.embedding_model = model,
            Err(err) => eprintln!("embornal: the fact is stored, but it has no vector: {err}"),
        }
        Ok(fact)
    }

    /// Writes the vector of one fact, if this memory has a model.
    ///
    /// It gives back the name of the model that wrote the vector, or `None`
    /// when the memory runs without vectors.
    fn embed_fact(&mut self, fact: &Fact) -> Result<Option<String>> {
        if self.embedder.is_off() {
            return Ok(None);
        }
        let Some(embedder) = self.embedder.get()? else {
            return Ok(None);
        };

        let vector = embedder.embed_one(Input::Document {
            path: &fact.path,
            content: &fact.content,
        })?;
        let model = embedder.model_name().to_string();
        write_embedding(self.db.conn_mut(), fact.id, &vector, &model)?;
        Ok(Some(model))
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
        let subtree_fact_count = self.visible_subtree_fact_count(path, &filter)?;

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
                subtree_fact_count: visible,
                child_count: self.child_count(child_id)?,
            });
        }

        Ok(Listing {
            path: path.clone(),
            children,
            fact_count,
            subtree_fact_count,
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
    /// With a query, two indexes answer. The keyword index finds the facts
    /// that hold the words. The vector index finds the facts that hold the
    /// sense, even when they share no word with the question. The strength of
    /// each fact then moves it up or down.
    ///
    /// With no query, the strongest facts come back.
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

    /// Asks both indexes and mixes their answers with the strength.
    fn search(
        &mut self,
        query: &str,
        filter: &AccessFilter,
        options: &RecallOptions,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScoredFact>> {
        let keyword = self.by_keyword(query, filter, options)?;
        let vector = self.by_vector(query, filter, options)?;

        let weights = &self.config.recall;
        let mut mixed: Vec<ScoredFact> = Vec::new();
        // The facts that both indexes found must appear one time, so a fact
        // that arrives again only takes the score that it was missing.
        let mut seen: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

        for (fact, keyword_score, vector_score) in keyword
            .into_iter()
            .map(|(fact, score)| (fact, Some(score), None))
            .chain(
                vector
                    .into_iter()
                    .map(|(fact, score)| (fact, None, Some(score))),
            )
        {
            match seen.get(&fact.id.0) {
                Some(&at) => {
                    let entry: &mut ScoredFact = &mut mixed[at];
                    entry.keyword_score = entry.keyword_score.or(keyword_score);
                    entry.vector_score = entry.vector_score.or(vector_score);
                }
                None => {
                    seen.insert(fact.id.0, mixed.len());
                    let strength = fact.signal.strength_at(now);
                    mixed.push(ScoredFact {
                        score: 0.0,
                        fact,
                        keyword_score,
                        vector_score,
                        signal_strength: strength,
                    });
                }
            }
        }

        // A fact that only one index found scores zero on the other one. That
        // is the point of the mix: a fact that both indexes name rises above
        // a fact that only one of them names.
        for entry in &mut mixed {
            entry.score = weights.keyword_weight * entry.keyword_score.unwrap_or(0.0)
                + weights.vector_weight * entry.vector_score.unwrap_or(0.0)
                + weights.signal_weight * entry.signal_strength;
        }
        Ok(mixed)
    }

    /// Builds the expression that the keyword index reads.
    ///
    /// A word that most of the facts hold does not tell one fact from
    /// another. A question such as "where is the data kept" would otherwise
    /// reach every fact that holds "the", and the best of those weak matches
    /// would take the top of the answer away from a fact that really answers
    /// the question. Such a word therefore leaves the expression.
    ///
    /// The count runs through the index itself, and not through a list of
    /// words. Two reasons ask for that. One memory holds more than one
    /// language, and a list would serve only the language that wrote it. And
    /// only the index knows how it folds a word, so `MEMÓRIA` and `memoria`
    /// count as one word here, exactly as they do in the search.
    fn keyword_expression(&self, query: &str) -> Result<String> {
        let terms = fts_terms(query);
        if terms.is_empty() {
            return Ok(String::new());
        }

        let facts: i64 = self
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))?;
        let ceiling = (facts as f64 * self.config.recall.keyword_ceiling).floor() as i64;

        // One lookup for each word of the question. A question holds few
        // words, and each lookup reads the index alone.
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT COUNT(*) FROM facts_fts WHERE facts_fts MATCH ?")?;
        let mut kept = Vec::with_capacity(terms.len());
        for term in &terms {
            let holders: i64 = stmt.query_row([term], |row| row.get(0))?;
            if holders <= ceiling {
                kept.push(term.clone());
            }
        }

        // A question that holds nothing but common words still asks
        // something. This also carries a memory that is too small to tell a
        // common word from a rare one.
        if kept.is_empty() {
            return Ok(terms.join(" OR "));
        }
        Ok(kept.join(" OR "))
    }

    /// Reads the keyword index.
    fn by_keyword(
        &self,
        query: &str,
        filter: &AccessFilter,
        options: &RecallOptions,
    ) -> Result<Vec<(Fact, f64)>> {
        let expression = self.keyword_expression(query)?;
        if expression.is_empty() {
            return Ok(Vec::new());
        }

        let candidates = candidate_count(options);
        let (subtree, subtree_params) = subtree_clause(options.under.as_ref());

        let sql = format!(
            "SELECT f.id, f.ulid, f.path_id, p.full_path, f.content, f.created_at,
                    f.last_recall_at, f.recall_count, f.stability_days,
                    f.supersedes_id, f.deleted_at, f.embedding_model, f.owner,
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
            let rank: f64 = row.get(13)?;
            Ok((fact, rank))
        })?;
        let hits: Vec<(Fact, f64)> = rows.collect::<rusqlite::Result<_>>()?;

        // bm25 gives a negative number, and the best match is the smallest.
        Ok(rescale(hits))
    }

    /// Reads the vector index.
    ///
    /// A memory with no model gives back nothing here, and `recall` then works
    /// exactly as it did before the model arrived.
    fn by_vector(
        &mut self,
        query: &str,
        filter: &AccessFilter,
        options: &RecallOptions,
    ) -> Result<Vec<(Fact, f64)>> {
        if self.embedder.is_off() {
            return Ok(Vec::new());
        }
        let Some(embedder) = self.embedder.get()? else {
            return Ok(Vec::new());
        };
        let vector = embedder.embed_one(Input::Query(query))?;
        let blob = embedding::to_blob(&vector);

        let candidates = candidate_count(options);
        let (subtree, subtree_params) = subtree_clause(options.under.as_ref());

        // The vector index answers on its own: it takes no join and no other
        // condition beside the width. The facts that the reader may not see
        // therefore fall away below, and they use up places of `k`. Asking for
        // more places than the caller wants pays for that.
        let sql = format!(
            "WITH knn AS (
                 SELECT fact_id, distance FROM {VEC_TABLE}
                 WHERE embedding MATCH ? AND k = ?
             )
             SELECT f.id, f.ulid, f.path_id, p.full_path, f.content, f.created_at,
                    f.last_recall_at, f.recall_count, f.stability_days,
                    f.supersedes_id, f.deleted_at, f.embedding_model, f.owner,
                    knn.distance AS rank
             FROM knn
             JOIN facts f ON f.id = knn.fact_id
             JOIN paths p ON p.id = f.path_id
             WHERE f.deleted_at IS NULL AND ({}){subtree}
             ORDER BY rank",
            filter.sql()
        );

        let mut bound: Vec<&dyn ToSql> = vec![&blob, &candidates];
        for value in filter.params() {
            bound.push(value);
        }
        for value in &subtree_params {
            bound.push(value);
        }

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), |row| {
            let fact = fact_from_row(row)?;
            let distance: f64 = row.get(13)?;
            Ok((fact, distance))
        })?;

        let mut hits: Vec<(Fact, f64)> = rows
            .collect::<rusqlite::Result<Vec<(Fact, f64)>>>()?
            .into_iter()
            .map(|(fact, distance)| (fact, similarity(distance)))
            .collect();

        // The index gives the nearest facts even when none of them is near.
        // Without a cut, a question that the memory cannot answer would still
        // come back full.
        let weights = &self.config.recall;
        let best = hits
            .iter()
            .map(|(_, score)| *score)
            .fold(f64::MIN, f64::max);
        let cut = weights.vector_floor.max(best * weights.vector_share);
        hits.retain(|(_, score)| *score >= cut);
        Ok(hits)
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
    // reindex
    // -----------------------------------------------------------------------

    /// Gives a vector to each fact that has none.
    ///
    /// A fact reaches the queue when it was written before the memory had a
    /// model, or when the model failed at the moment of writing. With
    /// [`ReindexOptions::all`], every fact goes back into the queue first,
    /// which is how a memory moves to another model.
    ///
    /// A fact that the reader may not see stays where it is.
    pub fn reindex(&mut self, options: ReindexOptions) -> Result<ReindexReport> {
        let has_model = !self.embedder.is_off();

        // Throwing the vectors away without a model to write new ones would
        // leave the memory worse than it was.
        if options.all && has_model {
            let tx = self.db.conn_mut().transaction()?;
            tx.execute(&format!("DELETE FROM {VEC_TABLE}"), [])?;
            tx.execute(
                "UPDATE facts SET embedding = NULL, embedding_model = NULL
                 WHERE embedding IS NOT NULL",
                [],
            )?;
            tx.commit()?;
        }

        let filter = self.guard.filter(Action::Read);
        if filter.is_empty_set() {
            return Ok(ReindexReport {
                has_model,
                ..ReindexReport::default()
            });
        }

        let sql = format!(
            "{FACT_SELECT} WHERE f.embedding IS NULL AND f.deleted_at IS NULL AND ({})
             ORDER BY f.id",
            filter.sql()
        );
        let mut bound: Vec<&dyn ToSql> = Vec::new();
        for value in filter.params() {
            bound.push(value);
        }

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), fact_from_row)?;
        let mut pending: Vec<Fact> = rows.collect::<rusqlite::Result<_>>()?;
        drop(stmt);

        if let Some(limit) = options.limit {
            pending.truncate(limit);
        }
        let mut report = ReindexReport {
            has_model,
            pending: pending.len(),
            done: 0,
            model: None,
        };
        // A memory with no model still counts the queue, so that the reader
        // learns how much work a model would find here.
        if pending.is_empty() || !has_model {
            return Ok(report);
        }

        // The facts go in groups, so that a failure in the middle keeps the
        // work that came before it.
        for group in pending.chunks(REINDEX_GROUP) {
            let Some(embedder) = self.embedder.get()? else {
                return Ok(report);
            };
            let inputs: Vec<Input<'_>> = group
                .iter()
                .map(|fact| Input::Document {
                    path: &fact.path,
                    content: &fact.content,
                })
                .collect();
            let vectors = embedder.embed(&inputs)?;
            let model = embedder.model_name().to_string();

            for (fact, vector) in group.iter().zip(vectors) {
                write_embedding(self.db.conn_mut(), fact.id, &vector, &model)?;
                report.done += 1;
            }
            report.model = Some(model);
        }
        Ok(report)
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

/// How many facts one index gives back before the mix.
///
/// It is more than the caller wants, because the mix with the other index and
/// with the strength changes the order.
fn candidate_count(options: &RecallOptions) -> i64 {
    (options.limit * 4).max(50) as i64
}

/// Turns the distance of the vector index into a score.
///
/// The vectors have a length of one, and the index measures the straight
/// distance `d` between two of them. For such vectors the angle gives
/// `cos = 1 - d² / 2`, which is 1.0 for two texts that say the same, 0.0 for
/// two texts with nothing in common, and -1.0 for two texts that say the
/// opposite.
///
/// This is an absolute scale, so it needs no other hit to make sense of it.
/// The keyword index has no such scale, which is why [`rescale`] exists for
/// that one alone.
fn similarity(distance: f64) -> f64 {
    1.0 - (distance * distance) / 2.0
}

/// Turns the rank of the keyword index into a number from 0.0 to 1.0.
///
/// bm25 gives a negative number, and the best match is the smallest. The
/// number says nothing on its own: it depends on the words of the question
/// and on the whole memory. This therefore maps the best hit of the list to
/// 1.0 and the worst to 0.0.
///
/// A list of one hit, or a list where every hit ties, scores 1.0 throughout:
/// with no spread there is nothing to tell the hits apart.
fn rescale(hits: Vec<(Fact, f64)>) -> Vec<(Fact, f64)> {
    let best = hits.iter().map(|(_, rank)| *rank).fold(f64::MAX, f64::min);
    let worst = hits.iter().map(|(_, rank)| *rank).fold(f64::MIN, f64::max);
    let spread = (worst - best).abs();

    hits.into_iter()
        .map(|(fact, rank)| {
            let score = if spread < f64::EPSILON {
                1.0
            } else {
                (worst - rank) / spread
            };
            (fact, score)
        })
        .collect()
}

/// Writes the vector of one fact, in the table and in the vector index.
///
/// The two writes go together: a row of the index that names a fact with no
/// vector would make the two disagree.
fn write_embedding(conn: &mut Connection, fact: FactId, vector: &[f32], model: &str) -> Result<()> {
    let blob = embedding::to_blob(vector);
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE facts SET embedding = ?, embedding_model = ? WHERE id = ?",
        params![blob, model, fact.0],
    )?;
    // A fact that already had a vector gets a new one, so the old row goes.
    tx.execute(
        &format!("DELETE FROM {VEC_TABLE} WHERE fact_id = ?"),
        params![fact.0],
    )?;
    tx.execute(
        &format!("INSERT INTO {VEC_TABLE}(fact_id, embedding) VALUES (?, ?)"),
        params![fact.0, blob],
    )?;
    tx.commit()?;
    Ok(())
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
        owner: row.get(12)?,
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

/// Cuts what a user typed into the terms of an FTS5 expression.
///
/// Each word becomes a quoted term. The quotes stop the FTS5 syntax from
/// reading a word such as `NOT` or `a-b` as an operator.
pub fn fts_terms(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect()
}

/// Turns what a user typed into an FTS5 expression.
pub fn fts_query(input: &str) -> String {
    fts_terms(input).join(" OR ")
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

    // -- the owner of a fact ------------------------------------------------

    /// Reads the owner column and the owner tag of one fact.
    fn owner_of(memory: &Memory, fact: FactId) -> (Option<String>, Option<String>) {
        memory
            .database()
            .conn()
            .query_row(
                "SELECT f.owner, t.value
                   FROM facts f
                   LEFT JOIN fact_tags t ON t.fact_id = f.id AND t.key = 'owner'
                  WHERE f.id = ?",
                [fact.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    /// Gives `name` the rules that a new user of a server gets: it writes
    /// anywhere, and it reads what it wrote.
    ///
    /// It also joins the role that reads the facts about the memory itself.
    fn enrol(memory: &Memory, name: &str) {
        let conn = memory.database().conn();
        for action in [Action::Write, Action::Delete] {
            conn.execute(
                "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3) VALUES ('p', ?, 'path:/*', ?, 'allow')",
                params![name, action.as_str()],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3) VALUES ('p', ?, ?, 'read', 'allow')",
            params![name, format!("tag:owner={name}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO casbin_rule(ptype, v0, v1) VALUES ('g', ?, 'everyone')",
            [name],
        )
        .unwrap();
    }

    /// Speaks for another subject on the same memory.
    fn as_subject(memory: &mut Memory, name: &str) {
        memory.set_subject(Subject::parse(name).unwrap()).unwrap();
    }

    #[test]
    fn a_stored_fact_says_who_wrote_it() {
        let mut memory = memory();
        let fact = write(&mut memory, "/notes", "one");

        // The column is the truth, and the tag is the same name in the form
        // that the access rules read.
        assert_eq!(
            owner_of(&memory, fact.id),
            (Some("cli".to_string()), Some("cli".to_string()))
        );
    }

    #[test]
    fn nobody_but_the_memory_names_the_owner_of_a_fact() {
        let mut memory = memory();
        let refused = memory.store(NewFact {
            path: path("/notes"),
            content: "mine now".to_string(),
            tags: vec![Tag::parse("owner=somebody-else").unwrap()],
            supersedes_id: None,
        });
        assert!(matches!(refused, Err(Error::ReservedTag(_))), "{refused:?}");

        // The fact did not reach the memory either.
        assert!(memory.path_id(&path("/notes")).unwrap().is_none());
    }

    #[test]
    fn a_tag_of_a_path_cannot_take_a_fact_from_its_writer() {
        let mut memory = memory();
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
                 VALUES ('01ABCDEFGHIJKLMNOPQRSTUVWX', 1, 'notes', '/notes', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        let path_id: i64 = memory
            .database()
            .conn()
            .query_row("SELECT id FROM paths WHERE full_path = '/notes'", [], |r| {
                r.get(0)
            })
            .unwrap();
        memory
            .database()
            .conn()
            .execute(
                "INSERT INTO path_tags(path_id, key, value) VALUES (?, 'owner', 'somebody-else')",
                [path_id],
            )
            .unwrap();

        let fact = write(&mut memory, "/notes", "still mine");

        // The tag of the fact wins over the tag that it takes from the path.
        assert_eq!(
            memory
                .effective_tags(fact.id)
                .unwrap()
                .get(&TagKey::parse("owner").unwrap())
                .map(|v| v.to_string()),
            Some("cli".to_string())
        );
    }

    #[test]
    fn a_subject_reads_its_own_facts_and_not_the_facts_of_another() {
        // Two subjects share one memory, each with the rules that a new user
        // of a server gets.
        let mut memory = memory();
        enrol(&memory, "alice");
        enrol(&memory, "bob");

        as_subject(&mut memory, "alice");
        write(&mut memory, "/notes", "alice wrote this");

        as_subject(&mut memory, "bob");
        write(&mut memory, "/notes", "bob wrote this");

        let seen: Vec<String> = memory
            .cat(&path("/notes"), CatOptions::default())
            .unwrap()
            .iter()
            .map(|fact| fact.content.clone())
            .collect();
        assert_eq!(seen, ["bob wrote this"]);

        let found = memory
            .recall(Some("wrote"), RecallOptions::default())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fact.content, "bob wrote this");

        // The path shows one fact to each of them, not two.
        assert_eq!(memory.ls(&path("/notes")).unwrap().fact_count, 1);
        as_subject(&mut memory, "alice");
        assert_eq!(memory.ls(&path("/notes")).unwrap().fact_count, 1);
    }

    #[test]
    fn every_subject_reads_the_facts_of_the_memory_itself() {
        let mut memory = memory();
        enrol(&memory, "alice");
        as_subject(&mut memory, "alice");

        // The memory carries its own instructions, and a new subject needs
        // them before it can use anything else.
        let facts = memory.cat(&path("/memory"), CatOptions::default()).unwrap();
        assert_eq!(facts.len(), crate::memory::db::MEMORY_SEED_LEN);
    }

    #[test]
    fn the_command_line_keeps_the_whole_memory_to_itself() {
        // A memory of one machine gives its subject everything, so the split
        // by owner changes nothing for it.
        let mut memory = memory();
        write(&mut memory, "/notes", "one");
        let other = Subject::parse("someone-else").unwrap();
        memory
            .database()
            .conn()
            .execute(
                "UPDATE facts SET owner = ? WHERE content = 'one'",
                [other.as_str()],
            )
            .unwrap();

        assert_eq!(
            memory
                .cat(&path("/notes"), CatOptions::default())
                .unwrap()
                .len(),
            1
        );
    }

    // -- the fake model -----------------------------------------------------

    /// How many topics [`Topics`] knows. It is also the width of its vectors.
    const TOPICS: usize = 4;

    /// The words that put a text into a topic. The last topic holds every
    /// text that no word of the lists above reaches.
    const TOPIC_WORDS: [&[&str]; TOPICS] = [
        &["sqlite", "database", "row", "table"],
        &["rust", "cargo", "crate"],
        &["signal", "strength", "forget"],
        &[],
    ];

    /// A model that puts a text into one topic and gives back the corner of
    /// the space that the topic owns.
    ///
    /// Two texts of one topic sit in the same place, so a test can say what
    /// the vector index must find without a 300 MB file of weights.
    struct Topics;

    impl Embedder for Topics {
        fn embed(&mut self, inputs: &[Input<'_>]) -> Result<Vec<Vec<f32>>> {
            inputs
                .iter()
                .map(|input| {
                    let text = input.prompt().to_lowercase();
                    let topic = TOPIC_WORDS
                        .iter()
                        .position(|words| words.iter().any(|word| text.contains(word)))
                        .unwrap_or(TOPICS - 1);
                    let mut vector = vec![0.0f32; TOPICS];
                    vector[topic] = 1.0;
                    embedding::shape(vector, TOPICS)
                })
                .collect()
        }

        fn dimensions(&self) -> usize {
            TOPICS
        }

        fn model_name(&self) -> &str {
            "topics"
        }
    }

    /// A model that never answers.
    struct Broken;

    impl Embedder for Broken {
        fn embed(&mut self, _inputs: &[Input<'_>]) -> Result<Vec<Vec<f32>>> {
            Err(Error::Embedding("this model is broken".to_string()))
        }

        fn dimensions(&self) -> usize {
            TOPICS
        }

        fn model_name(&self) -> &str {
            "broken"
        }
    }

    /// A memory whose vector index is as wide as the fake model.
    fn narrow_memory() -> Memory {
        let mut config = Config::default();
        config.embedding.dimensions = TOPICS;
        Memory::open_in_memory(config).unwrap()
    }

    /// A memory that embeds each fact with [`Topics`].
    fn memory_with_vectors() -> Memory {
        narrow_memory().with_embedder(Box::new(Topics))
    }

    /// Reads the vector that a fact holds, if it holds one.
    fn stored_vector(memory: &Memory, fact: FactId) -> Option<(Vec<u8>, String)> {
        memory
            .database()
            .conn()
            .query_row(
                "SELECT embedding, embedding_model FROM facts WHERE id = ?",
                [fact.0],
                |row| {
                    Ok(row
                        .get::<_, Option<Vec<u8>>>(0)?
                        .zip(row.get::<_, Option<String>>(1)?))
                },
            )
            .unwrap()
    }

    /// How many rows the vector index holds for a fact.
    fn indexed(memory: &Memory, fact: FactId) -> i64 {
        memory
            .database()
            .conn()
            .query_row(
                &format!("SELECT COUNT(*) FROM {VEC_TABLE} WHERE fact_id = ?"),
                [fact.0],
                |row| row.get(0),
            )
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
        assert_eq!(entry.subtree_fact_count, 2);
        assert_eq!(entry.child_count, 1);
    }

    #[test]
    fn ls_reports_the_visible_facts_in_its_full_subtree() {
        let mut memory = memory();
        write(&mut memory, "/a", "on this path");
        write(&mut memory, "/a/b", "one level below");
        write(&mut memory, "/a/b/c", "two levels below");

        let listing = memory.ls(&path("/a")).unwrap();
        assert_eq!(listing.fact_count, 1);
        assert_eq!(listing.subtree_fact_count, 3);
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

    // -- the vectors --------------------------------------------------------

    #[test]
    fn a_store_writes_the_vector_of_the_fact() {
        let mut memory = memory_with_vectors();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");

        assert_eq!(fact.embedding_model.as_deref(), Some("topics"));
        let (blob, model) = stored_vector(&memory, fact.id).expect("the fact holds a vector");
        // Four numbers of four bytes each.
        assert_eq!(blob.len(), TOPICS * 4);
        assert_eq!(model, "topics");
        assert_eq!(indexed(&memory, fact.id), 1);
    }

    #[test]
    fn a_memory_with_no_model_leaves_the_vector_empty() {
        let mut memory = narrow_memory();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");

        assert_eq!(fact.embedding_model, None);
        assert!(stored_vector(&memory, fact.id).is_none());
        assert_eq!(indexed(&memory, fact.id), 0);
    }

    #[test]
    fn a_model_that_fails_does_not_lose_the_fact() {
        let mut memory = narrow_memory().with_embedder(Box::new(Broken));
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");

        // The fact is written, and it waits for the backfill.
        assert_eq!(fact.embedding_model, None);
        let found = memory.cat(&path("/db"), CatOptions::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(stored_vector(&memory, fact.id).is_none());
    }

    #[test]
    fn recall_finds_a_fact_that_shares_no_word_with_the_question() {
        let mut memory = memory_with_vectors();
        write(&mut memory, "/db", "The memory uses SQLite.");
        write(&mut memory, "/lang", "The tool is written in Rust.");

        // "database" appears in neither fact, so the keyword index says
        // nothing. Only the vector index can answer.
        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();

        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].fact.content, "The memory uses SQLite.");
        assert!(hits[0].vector_score.is_some());
        assert!(hits[0].keyword_score.is_none());
    }

    #[test]
    fn without_a_model_that_same_question_finds_nothing() {
        // This is the other half of the test above: it shows that the vector
        // index, and not something else, gave the answer.
        let mut memory = narrow_memory();
        write(&mut memory, "/db", "The memory uses SQLite.");

        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn a_fact_that_both_indexes_find_rises_above_one_that_only_one_finds() {
        let mut memory = memory_with_vectors();
        write(&mut memory, "/a", "The database holds every row.");
        write(&mut memory, "/b", "One table for each topic.");

        // Both facts sit in the same topic, so the vector index gives them
        // the same place. Only the first holds the word.
        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();

        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].fact.content, "The database holds every row.");
        assert!(hits[0].keyword_score.is_some());
        assert!(hits[0].vector_score.is_some());
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn a_deleted_fact_stays_out_of_the_vector_answer() {
        let mut memory = memory_with_vectors();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");
        memory
            .database()
            .conn()
            .execute(
                "UPDATE facts SET deleted_at = '2026-01-01T00:00:00.000000Z' WHERE id = ?",
                [fact.id.0],
            )
            .unwrap();

        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn the_floor_holds_back_an_answer_where_nothing_is_near() {
        let far = |floor: f64| {
            let mut config = Config::default();
            config.embedding.dimensions = TOPICS;
            config.recall.vector_floor = floor;
            let mut memory = Memory::open_in_memory(config)
                .unwrap()
                .with_embedder(Box::new(Topics));
            write(&mut memory, "/lang", "The tool is written in Rust.");
            memory
                .recall(Some("database"), RecallOptions::default())
                .unwrap()
                .len()
        };

        // The fact sits in another topic, so it has nothing in common with
        // the question and scores 0.0. The whole answer is bad, and the share
        // alone would still give the best of it.
        assert_eq!(far(0.15), 0);
        // A floor below that lets it in, which shows that the floor, and not
        // something else, held it back.
        assert_eq!(far(-1.0), 1);
    }

    #[test]
    fn the_share_drops_the_tail_of_a_good_answer() {
        let mut config = Config::default();
        config.embedding.dimensions = TOPICS;
        config.recall.keyword_weight = 0.0;
        config.recall.signal_weight = 0.0;
        // The floor lets everything through, so the share alone decides.
        config.recall.vector_floor = -1.0;
        config.recall.vector_share = 0.5;
        let mut memory = Memory::open_in_memory(config)
            .unwrap()
            .with_embedder(Box::new(Topics));

        write(&mut memory, "/a", "The database holds every row.");
        write(&mut memory, "/b", "The tool is written in Rust.");

        // The first fact shares the topic of the question and scores 1.0.
        // The second is in another topic and scores 0.0, which is below half
        // of the best.
        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].fact.content, "The database holds every row.");
    }

    #[test]
    fn the_distance_of_the_index_becomes_an_angle() {
        // Two vectors that point the same way.
        assert!((similarity(0.0) - 1.0).abs() < 1e-9);
        // Two vectors at a right angle. Their distance is the root of two.
        assert!(similarity(2.0f64.sqrt()).abs() < 1e-9);
        // Two vectors that point against each other.
        assert!((similarity(2.0) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_holds_the_weights_of_the_configuration() {
        let mut config = Config::default();
        config.embedding.dimensions = TOPICS;
        // The vector index alone decides the order.
        config.recall.keyword_weight = 0.0;
        config.recall.signal_weight = 0.0;
        config.recall.vector_weight = 1.0;
        let mut memory = Memory::open_in_memory(config)
            .unwrap()
            .with_embedder(Box::new(Topics));

        write(&mut memory, "/a", "The database holds every row.");
        let hits = memory
            .recall(Some("database"), RecallOptions::default())
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, hits[0].vector_score.unwrap());
    }

    // -- reindex ------------------------------------------------------------

    #[test]
    fn reindex_fills_the_facts_that_wait_in_the_queue() {
        // The facts arrive before the model does.
        let mut memory = narrow_memory();
        let first = write(&mut memory, "/db", "The memory uses SQLite.");
        let second = write(&mut memory, "/lang", "The tool is written in Rust.");
        assert!(stored_vector(&memory, first.id).is_none());

        let mut memory = memory.with_embedder(Box::new(Topics));
        let report = memory.reindex(ReindexOptions::default()).unwrap();

        // The seeded facts of /memory wait in the queue as well.
        assert!(report.pending >= 2);
        assert_eq!(report.done, report.pending);
        assert_eq!(report.model.as_deref(), Some("topics"));
        for fact in [first.id, second.id] {
            assert!(stored_vector(&memory, fact).is_some());
            assert_eq!(indexed(&memory, fact), 1);
        }
    }

    #[test]
    fn a_second_reindex_has_nothing_to_do() {
        let mut memory = memory_with_vectors();
        write(&mut memory, "/db", "The memory uses SQLite.");
        memory.reindex(ReindexOptions::default()).unwrap();

        let report = memory.reindex(ReindexOptions::default()).unwrap();
        assert_eq!(report.pending, 0);
        assert_eq!(report.done, 0);
        assert_eq!(report.model, None);
    }

    #[test]
    fn reindex_stops_at_the_limit() {
        let mut memory = narrow_memory();
        write(&mut memory, "/a", "one");
        write(&mut memory, "/b", "two");

        let mut memory = memory.with_embedder(Box::new(Topics));
        let report = memory
            .reindex(ReindexOptions {
                limit: Some(1),
                all: false,
            })
            .unwrap();
        assert_eq!(report.pending, 1);
        assert_eq!(report.done, 1);
    }

    #[test]
    fn reindex_with_all_writes_every_vector_again() {
        let mut memory = memory_with_vectors();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");
        memory.reindex(ReindexOptions::default()).unwrap();

        let report = memory
            .reindex(ReindexOptions {
                limit: None,
                all: true,
            })
            .unwrap();

        // Every fact went back into the queue, the seeded ones included.
        assert!(report.pending > 1);
        assert_eq!(report.done, report.pending);
        assert_eq!(indexed(&memory, fact.id), 1);
    }

    #[test]
    fn reindex_counts_the_queue_but_writes_nothing_without_a_model() {
        let mut memory = narrow_memory();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");

        let report = memory.reindex(ReindexOptions::default()).unwrap();
        assert!(!report.has_model);
        assert!(report.pending > 0);
        assert_eq!(report.done, 0);
        assert_eq!(report.model, None);
        assert!(stored_vector(&memory, fact.id).is_none());
    }

    #[test]
    fn reindex_with_all_keeps_the_vectors_when_there_is_no_model() {
        // The facts get their vectors, and then the model goes away.
        let mut memory = memory_with_vectors();
        let fact = write(&mut memory, "/db", "The memory uses SQLite.");
        let before = stored_vector(&memory, fact.id).expect("the fact holds a vector");

        let mut memory = Memory {
            embedder: Provider::off(),
            ..memory
        };
        let report = memory
            .reindex(ReindexOptions {
                limit: None,
                all: true,
            })
            .unwrap();

        assert!(!report.has_model);
        // Throwing the vectors away with nothing to write new ones would
        // leave the memory worse than it was.
        assert_eq!(stored_vector(&memory, fact.id), Some(before));
        assert_eq!(indexed(&memory, fact.id), 1);
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
    fn a_word_that_most_facts_hold_leaves_the_question() {
        let mut memory = memory();
        for at in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            write(&mut memory, at, "the tool holds the word");
        }
        write(&mut memory, "/rare", "the tool holds zarquon");

        // "the" and "tool" reach every fact, so only "zarquon" is left and
        // one fact comes back.
        let hits = memory
            .recall(Some("the tool zarquon"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].fact.content.contains("zarquon"));
    }

    #[test]
    fn a_question_of_common_words_alone_still_asks_something() {
        let mut memory = memory();
        for at in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            write(&mut memory, at, "the tool holds the word");
        }

        // Every word of the question is common. Dropping all of them would
        // turn the question into silence, so they all stay.
        let hits = memory
            .recall(Some("the tool"), RecallOptions::default())
            .unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn a_ceiling_of_one_keeps_every_word() {
        let mut config = Config::default();
        config.recall.keyword_ceiling = 1.0;
        let mut memory = Memory::open_in_memory(config).unwrap();
        for at in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            write(&mut memory, at, "the tool holds the word");
        }
        write(&mut memory, "/rare", "the tool holds zarquon");

        // With no ceiling, "the" reaches every fact again.
        let hits = memory
            .recall(Some("the tool zarquon"), RecallOptions::default())
            .unwrap();
        assert!(hits.len() > 1, "{hits:?}");
    }

    #[test]
    fn a_common_word_is_folded_the_way_the_index_folds_it() {
        let mut memory = memory();
        for at in ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"] {
            write(&mut memory, at, "a memória do sistema");
        }
        write(&mut memory, "/rare", "a memória guarda zarquon");

        // The question writes MEMORIA without the accent. The count must
        // still see it as the word that every fact holds, because that is how
        // the index reads it.
        let hits = memory
            .recall(Some("MEMORIA zarquon"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].fact.content.contains("zarquon"));
    }

    #[test]
    fn a_small_memory_keeps_its_words() {
        // With one fact of its own, every word looks common. The question
        // must still find it.
        let mut memory = memory();
        write(&mut memory, "/db", "zarquon");

        let hits = memory
            .recall(Some("zarquon"), RecallOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
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
