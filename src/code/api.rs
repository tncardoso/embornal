//! What a command calls.
//!
//! [`CodeIndex`] holds the file, the embedding model and the configuration,
//! and it is the only thing that the command line talks to.

use crate::code::db::{Database, VEC_TABLE};
use crate::code::index::{self, IndexReport};
use crate::code::node::NodeKind;
use crate::code::queue::{self, Batch, Written};
use crate::common::score;
use crate::config::{CodeConfig, Config, Paths};
use crate::embedding::{self, Embedder, Input, Provider};
use crate::error::{Error, Result};
use rusqlite::{OptionalExtension, params};
use std::path::Path;

/// One answer of [`CodeIndex::recall`].
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub qualified_name: String,
    pub kind: String,
    pub rel_path: String,
    pub start_line: Option<u32>,
    pub summary: String,
    pub description: String,
    pub score: f64,
    pub keyword_score: Option<f64>,
    pub vector_score: Option<f64>,
}

/// One node, with what an agent wrote about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    pub qualified_name: String,
    pub kind: String,
    pub rel_path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub written_at: Option<String>,
    pub author: Option<String>,
}

/// One node of the tree that `code tree` draws.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub rel_path: String,
    pub kind: String,
    /// Whether a summary is written for this node. The tree marks the nodes
    /// that carry none, so that it says where the work still is.
    pub described: bool,
    pub children: Vec<TreeNode>,
}

/// Draws the directories and files of one collection.
///
/// The tree stops at the files. What is inside a file is what `cat` and
/// `recall` answer with, and drawing a thousand functions would say less than
/// it shows.
pub fn tree(db: &Database, collection: &str, under: &str, depth: Option<u32>) -> Result<TreeNode> {
    let collection_id = queue::collection_id(db, collection)?;

    let mut stmt = db.conn().prepare(
        "SELECT n.rel_path, n.name, n.kind, s.id IS NOT NULL
         FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ? AND n.kind IN ('repo', 'dir', 'file')
         ORDER BY n.rel_path",
    )?;
    let rows: Vec<(String, String, String, bool)> = stmt
        .query_map([collection_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let top = rows
        .iter()
        .find(|(rel_path, _, _, _)| rel_path == under)
        .ok_or_else(|| Error::NoSuchNode(under.to_string()))?;

    Ok(grow(&rows, top, under, depth))
}

/// Puts the children of one node below it.
fn grow(
    rows: &[(String, String, String, bool)],
    node: &(String, String, String, bool),
    under: &str,
    depth: Option<u32>,
) -> TreeNode {
    let children = match depth {
        Some(0) => Vec::new(),
        _ => rows
            .iter()
            .filter(|(rel_path, _, _, _)| is_child(under, rel_path))
            .map(|child| grow(rows, child, &child.0, depth.map(|left| left - 1)))
            .collect(),
    };

    TreeNode {
        name: if node.1.is_empty() {
            "/".to_string()
        } else {
            node.1.clone()
        },
        rel_path: node.0.clone(),
        kind: node.2.clone(),
        described: node.3,
        children,
    }
}

/// Whether `path` sits directly below `parent`.
fn is_child(parent: &str, path: &str) -> bool {
    if path == parent {
        return false;
    }
    match parent.is_empty() {
        true => !path.contains('/'),
        false => path
            .strip_prefix(parent)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.contains('/')),
    }
}

/// An open code index.
pub struct CodeIndex {
    db: Database,
    config: Config,
    embedder: Provider,
}

impl CodeIndex {
    /// Opens the index that this machine keeps.
    pub fn open(paths: &Paths, config: Config) -> Result<Self> {
        let db = Database::open(&config.code_database_file(paths), &config)?;
        let embedder = Provider::from_config(&config, paths)?;
        Ok(Self {
            db,
            config,
            embedder,
        })
    }

    /// Opens an index that lives in RAM. The tests use this.
    pub fn open_in_memory(config: Config) -> Result<Self> {
        let db = Database::open_in_memory(&config)?;
        Ok(Self {
            db,
            config,
            embedder: Provider::off(),
        })
    }

    /// Replaces the embedder. The tests use this.
    pub fn with_embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Provider::ready(embedder);
        self
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    pub fn code_config(&self) -> &CodeConfig {
        &self.config.code
    }

    /// Brings one collection up to date with the files on disk.
    pub fn index(&mut self, root: &Path, collection: &str, all: bool) -> Result<IndexReport> {
        let config = self.config.code.clone();
        index::index(&mut self.db, root, collection, &config, all)
    }

    /// How many nodes wait, by kind.
    pub fn status(&self, collection: &str) -> Result<Vec<(String, usize, usize)>> {
        queue::status(&self.db, collection)
    }

    /// The next unit of work, or `None` when nothing waits.
    pub fn next(&self, collection: &str, update_root: bool) -> Result<Option<Batch>> {
        queue::next(&self.db, collection, update_root)
    }

    /// Up to `limit` units of work, for a caller that hands work to several
    /// agents at once. See [`queue::next_batches`] for what that means for
    /// two agents that ask before either writes a summary back.
    pub fn next_batch(
        &self,
        collection: &str,
        update_root: bool,
        limit: usize,
    ) -> Result<Vec<Batch>> {
        queue::next_batches(&self.db, collection, update_root, limit)
    }

    /// Takes the summaries that an agent wrote, and indexes them.
    pub fn describe(
        &mut self,
        collection: &str,
        written: &[Written],
        author: &str,
    ) -> Result<usize> {
        let count = queue::describe(&self.db, collection, written, author)?;
        self.embed(collection, written)?;
        Ok(count)
    }

    /// Searches the summaries of one collection.
    ///
    /// Two indexes answer, as they do for the memory. What is absent here is
    /// the third term: a fact of the memory loses strength with time, and a
    /// summary of code does not. A summary is right until the code moves, and
    /// a moved hash says that at once.
    pub fn recall(
        &mut self,
        collection: &str,
        query: &str,
        limit: Option<usize>,
        kind: Option<NodeKind>,
    ) -> Result<Vec<Hit>> {
        let collection_id = queue::collection_id(&self.db, collection)?;
        let limit = limit.unwrap_or(self.config.code.limit);
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let keyword = self.by_keyword(collection_id, query, limit, kind)?;
        let vector = self.by_vector(collection_id, query, limit, kind)?;

        let weights = &self.config.code;
        let mut mixed: Vec<Hit> = Vec::new();
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for (hit, keyword_score, vector_score) in keyword
            .into_iter()
            .map(|(hit, score)| (hit, Some(score), None))
            .chain(
                vector
                    .into_iter()
                    .map(|(hit, score)| (hit, None, Some(score))),
            )
        {
            match seen.get(&hit.qualified_name) {
                Some(&at) => {
                    let entry: &mut Hit = &mut mixed[at];
                    entry.keyword_score = entry.keyword_score.or(keyword_score);
                    entry.vector_score = entry.vector_score.or(vector_score);
                }
                None => {
                    seen.insert(hit.qualified_name.clone(), mixed.len());
                    mixed.push(Hit {
                        keyword_score,
                        vector_score,
                        ..hit
                    });
                }
            }
        }

        for entry in &mut mixed {
            entry.score = weights.keyword_weight * entry.keyword_score.unwrap_or(0.0)
                + weights.vector_weight * entry.vector_score.unwrap_or(0.0);
        }
        mixed.sort_by(|a, b| b.score.total_cmp(&a.score));
        mixed.truncate(limit);
        Ok(mixed)
    }

    /// One node, with what an agent wrote about it.
    pub fn cat(&self, collection: &str, name: &str) -> Result<Described> {
        let collection_id = queue::collection_id(&self.db, collection)?;
        self.db
            .conn()
            .query_row(
                "SELECT n.qualified_name, n.kind, n.rel_path, n.start_line, n.end_line,
                        s.summary, s.description, s.written_at, s.author
                 FROM nodes n
                 LEFT JOIN summaries s ON s.pool_key = n.pool_key
                 WHERE n.collection_id = ? AND (n.qualified_name = ? OR n.ulid = ?)",
                params![collection_id, name, name],
                |row| {
                    Ok(Described {
                        qualified_name: row.get(0)?,
                        kind: row.get(1)?,
                        rel_path: row.get(2)?,
                        start_line: row.get(3)?,
                        end_line: row.get(4)?,
                        summary: row.get(5)?,
                        description: row.get(6)?,
                        written_at: row.get(7)?,
                        author: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NoSuchNode(name.to_string()))
    }

    /// Writes the vector of each summary that just arrived.
    ///
    /// An index with no model writes nothing here, and everything else works
    /// exactly as it does with one.
    fn embed(&mut self, collection: &str, written: &[Written]) -> Result<()> {
        if self.embedder.is_off() || written.is_empty() {
            return Ok(());
        }
        let collection_id = queue::collection_id(&self.db, collection)?;

        // The title of a summary is the name of what it describes, which is
        // itself a good part of the answer to a question.
        let mut rows: Vec<(i64, String, String)> = Vec::new();
        for entry in written {
            let found: Option<(i64, String)> = self
                .db
                .conn()
                .query_row(
                    "SELECT s.id, n.qualified_name FROM nodes n
                     JOIN summaries s ON s.pool_key = n.pool_key
                     WHERE n.collection_id = ? AND n.ulid = ?",
                    params![collection_id, entry.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((id, name)) = found {
                rows.push((
                    id,
                    name,
                    format!("{}\n{}", entry.summary.trim(), entry.description.trim()),
                ));
            }
        }
        if rows.is_empty() {
            return Ok(());
        }

        let dimensions = self.db.dimensions();
        let Some(embedder) = self.embedder.get()? else {
            return Ok(());
        };
        let inputs: Vec<Input<'_>> = rows
            .iter()
            .map(|(_, title, content)| Input::Titled { title, content })
            .collect();
        let vectors = embedder.embed(&inputs)?;

        for ((id, _, _), vector) in rows.iter().zip(vectors) {
            let vector = embedding::shape(vector, dimensions)?;
            self.db.conn().execute(
                &format!("DELETE FROM {VEC_TABLE} WHERE summary_id = ?"),
                [id],
            )?;
            self.db.conn().execute(
                &format!("INSERT INTO {VEC_TABLE}(summary_id, embedding) VALUES (?, ?)"),
                params![id, embedding::to_blob(&vector)],
            )?;
        }
        Ok(())
    }

    /// Reads the keyword index.
    fn by_keyword(
        &self,
        collection_id: i64,
        query: &str,
        limit: usize,
        kind: Option<NodeKind>,
    ) -> Result<Vec<(Hit, f64)>> {
        let expression = self.keyword_expression(query)?;
        if expression.is_empty() {
            return Ok(Vec::new());
        }

        let candidates = score::candidate_count(limit);
        let (clause, wanted) = kind_clause(kind);
        let sql = format!(
            "SELECT n.qualified_name, n.kind, n.rel_path, n.start_line,
                    s.summary, s.description, bm25(summaries_fts) AS rank
             FROM summaries_fts
             JOIN summaries s ON s.id = summaries_fts.rowid
             JOIN nodes n ON n.pool_key = s.pool_key
             WHERE summaries_fts MATCH ? AND n.collection_id = ?{clause}
             ORDER BY rank
             LIMIT ?"
        );

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(bind(&expression, collection_id, &wanted, candidates)),
            hit_row,
        )?;
        Ok(score::rescale(rows.collect::<rusqlite::Result<_>>()?))
    }

    /// Reads the vector index.
    fn by_vector(
        &mut self,
        collection_id: i64,
        query: &str,
        limit: usize,
        kind: Option<NodeKind>,
    ) -> Result<Vec<(Hit, f64)>> {
        if self.embedder.is_off() {
            return Ok(Vec::new());
        }
        let dimensions = self.db.dimensions();
        let Some(embedder) = self.embedder.get()? else {
            return Ok(Vec::new());
        };
        let vector = embedding::shape(embedder.embed_one(Input::Query(query))?, dimensions)?;
        let blob = embedding::to_blob(&vector);

        let candidates = score::candidate_count(limit);
        let (clause, wanted) = kind_clause(kind);
        let sql = format!(
            "WITH knn AS (
                 SELECT summary_id, distance FROM {VEC_TABLE}
                 WHERE embedding MATCH ? AND k = ?
             )
             SELECT n.qualified_name, n.kind, n.rel_path, n.start_line,
                    s.summary, s.description, knn.distance AS rank
             FROM knn
             JOIN summaries s ON s.id = knn.summary_id
             JOIN nodes n ON n.pool_key = s.pool_key
             WHERE n.collection_id = ?{clause}
             ORDER BY rank"
        );

        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(blob),
            Box::new(candidates),
            Box::new(collection_id),
        ];
        for value in &wanted {
            bound.push(Box::new(value.clone()));
        }

        let mut stmt = self.db.conn().prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), hit_row)?;
        let mut hits: Vec<(Hit, f64)> = rows
            .collect::<rusqlite::Result<Vec<(Hit, f64)>>>()?
            .into_iter()
            .map(|(hit, distance)| (hit, score::similarity(distance)))
            .collect();

        let weights = &self.config.code;
        score::cut(&mut hits, weights.vector_floor, weights.vector_share);
        Ok(hits)
    }

    /// Builds the expression that the keyword index reads.
    ///
    /// A word that most of the summaries hold tells one node from no other, so
    /// it leaves the expression. The count runs through the index itself, and
    /// not through a list of words, so it needs no list per language.
    fn keyword_expression(&self, query: &str) -> Result<String> {
        let terms = crate::memory::api::fts_terms(query);
        if terms.is_empty() {
            return Ok(String::new());
        }

        let total: i64 = self
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM summaries", [], |row| row.get(0))?;
        let ceiling = score::ceiling(total, self.config.code.keyword_ceiling);

        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT COUNT(*) FROM summaries_fts WHERE summaries_fts MATCH ?")?;
        let mut kept = Vec::with_capacity(terms.len());
        for term in &terms {
            let holders: i64 = stmt.query_row([term], |row| row.get(0))?;
            if holders <= ceiling {
                kept.push(term.clone());
            }
        }

        // A question of nothing but common words still asks something, and so
        // does any question of an index that is still small.
        if kept.is_empty() {
            return Ok(terms.join(" OR "));
        }
        Ok(kept.join(" OR "))
    }
}

/// Reads one row of either index.
fn hit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Hit, f64)> {
    Ok((
        Hit {
            qualified_name: row.get(0)?,
            kind: row.get(1)?,
            rel_path: row.get(2)?,
            start_line: row.get(3)?,
            summary: row.get(4)?,
            description: row.get(5)?,
            score: 0.0,
            keyword_score: None,
            vector_score: None,
        },
        row.get(6)?,
    ))
}

/// The condition that keeps one kind of node, and what it binds.
fn kind_clause(kind: Option<NodeKind>) -> (String, Vec<String>) {
    match kind {
        Some(kind) => (
            " AND n.kind = ?".to_string(),
            vec![kind.as_str().to_string()],
        ),
        None => (String::new(), Vec::new()),
    }
}

fn bind(
    expression: &str,
    collection_id: i64,
    wanted: &[String],
    candidates: i64,
) -> Vec<Box<dyn rusqlite::ToSql>> {
    let mut bound: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(expression.to_string()), Box::new(collection_id)];
    for value in wanted {
        bound.push(Box::new(value.clone()));
    }
    bound.push(Box::new(candidates));
    bound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::queue::Written;

    /// How many topics [`Topics`] knows, and the width of its vectors.
    const TOPICS: usize = 4;

    /// The words that put a text into a topic. The last one holds every text
    /// that no word above reaches.
    const TOPIC_WORDS: [&[&str]; TOPICS] = [
        &["token", "secret", "credential"],
        &["parse", "grammar", "syntax"],
        &["directory", "walk", "file"],
        &[],
    ];

    /// A model that puts a text into one topic and answers with the corner of
    /// the space that the topic owns.
    ///
    /// Two texts of one topic sit in the same place, so a test can say what
    /// the vector index must find without a file of weights.
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

    struct Repo {
        root: std::path::PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("embornal-api-{name}"));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            Self {
                root: std::fs::canonicalize(&root).unwrap(),
            }
        }
        fn write(&self, rel: &str, text: &str) {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// An index whose vector side is the fake model above.
    fn open(repo: &Repo) -> CodeIndex {
        let mut config = Config::default();
        config.embedding.dimensions = TOPICS;
        let mut index = CodeIndex::open_in_memory(config)
            .unwrap()
            .with_embedder(Box::new(Topics));
        index.index(&repo.root, "test", false).unwrap();
        index
    }

    /// Describes one node by its qualified name.
    fn say(index: &mut CodeIndex, name: &str, summary: &str, description: &str) {
        let ulid: String = index
            .database()
            .conn()
            .query_row(
                "SELECT ulid FROM nodes WHERE qualified_name = ?",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        index
            .describe(
                "test",
                &[Written {
                    id: ulid,
                    summary: summary.to_string(),
                    description: description.to_string(),
                }],
                "default",
            )
            .unwrap();
    }

    fn names(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|hit| hit.qualified_name.as_str()).collect()
    }

    #[test]
    fn a_question_finds_the_summary_that_holds_its_words() {
        let repo = Repo::new("keyword");
        // The name of the file is part of what a summary is filed under, so a
        // fixture must not put the word of the question in the path.
        repo.write("src/guard.rs", "fn check() {}\nfn walk() {}\n");
        let mut index = open(&repo);

        say(
            &mut index,
            "src/guard.rs::check",
            "Checks that a token opens the memory.",
            "Reads the token of the request and compares it with the stored secret.",
        );
        say(
            &mut index,
            "src/guard.rs::walk",
            "Walks a directory.",
            "Reads every file below a directory and gives back the paths.",
        );

        let hits = index.recall("test", "token", None, None).unwrap();
        assert_eq!(names(&hits), vec!["src/guard.rs::check"]);
    }

    #[test]
    fn a_question_finds_a_summary_that_shares_no_word_with_it() {
        // This is what the vector index is for: the summary says "secret" and
        // the question says "credential", and the two never meet in FTS5.
        let repo = Repo::new("vector");
        repo.write("src/a.rs", "fn check() {}\nfn walk() {}\n");
        let mut index = open(&repo);

        say(
            &mut index,
            "src/a.rs::check",
            "Compares the given secret with the stored one.",
            "Reads the secret of the request and answers whether it opens the memory.",
        );
        say(
            &mut index,
            "src/a.rs::walk",
            "Reads a directory.",
            "Gives back every path below a directory.",
        );

        let hits = index.recall("test", "credential", None, None).unwrap();
        assert_eq!(names(&hits), vec!["src/a.rs::check"]);
        // It came from the vector side alone.
        assert!(hits[0].vector_score.is_some());
        assert!(hits[0].keyword_score.is_none());
    }

    #[test]
    fn a_node_that_both_indexes_name_rises_above_one_that_only_one_names() {
        let repo = Repo::new("mixed");
        repo.write("src/a.rs", "fn one() {}\nfn two() {}\n");
        let mut index = open(&repo);

        say(
            &mut index,
            "src/a.rs::one",
            "Reads the token of a request.",
            "The token names the caller, and the grammar of it is fixed.",
        );
        say(
            &mut index,
            "src/a.rs::two",
            "Parses a file with a grammar.",
            "Reads the syntax of a file and gives back its definitions.",
        );

        let hits = index.recall("test", "token", None, None).unwrap();
        assert_eq!(hits[0].qualified_name, "src/a.rs::one");
        assert!(hits[0].keyword_score.is_some() && hits[0].vector_score.is_some());
    }

    #[test]
    fn a_question_can_ask_for_one_kind_of_node() {
        let repo = Repo::new("kind");
        repo.write("src/a.rs", "fn parse() {}\n");
        let mut index = open(&repo);

        say(
            &mut index,
            "src/a.rs",
            "A file about parsing.",
            "It parses.",
        );
        say(
            &mut index,
            "src/a.rs::parse",
            "A function about parsing.",
            "It parses.",
        );

        let all = index.recall("test", "parsing", None, None).unwrap();
        assert_eq!(all.len(), 2);

        let only = index
            .recall("test", "parsing", None, Some(NodeKind::Function))
            .unwrap();
        assert_eq!(names(&only), vec!["src/a.rs::parse"]);
    }

    #[test]
    fn a_recall_answers_for_its_own_collection_only() {
        // The summaries are shared through the pool, and the nodes are not.
        let repo = Repo::new("scoped");
        repo.write("src/a.rs", "fn check() {}\n");
        let mut index = open(&repo);
        say(
            &mut index,
            "src/a.rs::check",
            "Checks a token.",
            "Reads the token and answers.",
        );

        index.index(&repo.root, "other", false).unwrap();
        assert_eq!(index.recall("other", "token", None, None).unwrap().len(), 1);

        let error = index.recall("absent", "token", None, None).unwrap_err();
        assert!(matches!(error, Error::NoSuchCollection(_)), "{error}");
    }

    #[test]
    fn an_empty_question_asks_nothing() {
        let repo = Repo::new("empty");
        repo.write("src/a.rs", "fn a() {}\n");
        let mut index = open(&repo);
        assert!(index.recall("test", "   ", None, None).unwrap().is_empty());
    }

    #[test]
    fn an_index_with_no_model_still_answers_by_word() {
        let repo = Repo::new("nomodel");
        repo.write("src/a.rs", "fn check() {}\n");
        let mut index = CodeIndex::open_in_memory(Config::default()).unwrap();
        index.index(&repo.root, "test", false).unwrap();
        say(
            &mut index,
            "src/a.rs::check",
            "Checks a token.",
            "Reads the token of a request.",
        );

        let hits = index.recall("test", "token", None, None).unwrap();
        assert_eq!(names(&hits), vec!["src/a.rs::check"]);
        assert!(hits[0].vector_score.is_none());
    }

    #[test]
    fn a_rewritten_summary_replaces_its_vector_as_well() {
        let repo = Repo::new("revector");
        repo.write("src/a.rs", "fn a() {}\n");
        let mut index = open(&repo);

        say(
            &mut index,
            "src/a.rs::a",
            "Reads a token.",
            "About secrets.",
        );
        say(
            &mut index,
            "src/a.rs::a",
            "Reads a directory.",
            "About walking files.",
        );

        // One summary, one vector: the older one did not stay behind.
        let vectors: i64 = index
            .database()
            .conn()
            .query_row(&format!("SELECT COUNT(*) FROM {VEC_TABLE}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(vectors, 1);

        let hits = index.recall("test", "walk", None, None).unwrap();
        assert_eq!(names(&hits), vec!["src/a.rs::a"]);
    }

    #[test]
    fn cat_gives_a_node_with_what_was_written_about_it() {
        let repo = Repo::new("cat");
        repo.write("src/a.rs", "fn a() {\n}\n");
        let mut index = open(&repo);

        let before = index.cat("test", "src/a.rs::a").unwrap();
        assert_eq!(before.kind, "function");
        assert_eq!((before.start_line, before.end_line), (Some(1), Some(2)));
        assert_eq!(before.summary, None);

        say(
            &mut index,
            "src/a.rs::a",
            "Does a thing.",
            "It does a thing.",
        );
        let after = index.cat("test", "src/a.rs::a").unwrap();
        assert_eq!(after.summary.as_deref(), Some("Does a thing."));
        assert_eq!(after.author.as_deref(), Some("default"));

        let error = index.cat("test", "src/a.rs::absent").unwrap_err();
        assert!(matches!(error, Error::NoSuchNode(_)), "{error}");
    }
}
