//! The queue of work, and what comes back from it.
//!
//! Embornal writes no summary. It says which nodes have none, an outside agent
//! writes them, and [`describe`] takes them back. That is the whole of the
//! harness: no model, no key, no provider.
//!
//! One batch is one file. An agent that described a file one function at a
//! time would read that file once for every function in it, and the file is
//! what it must read either way. A batch of the whole file also lets the
//! sibling functions inform each other.
//!
//! The batch carries no source. It says which file and which lines, and the
//! agent opens the file with the tools that it already has. Putting the source
//! in the payload would only send it through the context twice.

use crate::code::db::Database;
use crate::code::node::NodeKind;
use crate::error::{Error, Result};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// What a batch asks the agent to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchKind {
    /// A file and the definitions inside it. The agent opens the file.
    File,
    /// A directory, or the root. There is no file to open: the summaries of
    /// the children are the whole of the material.
    Dir,
}

/// One node that waits for a summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// What `describe` needs to write this one back.
    pub id: String,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    /// The lines of the node, counted from one. A directory has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
    /// The children that already carry a summary.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<Child>,
}

/// A child that is already described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    pub name: String,
    pub kind: String,
    pub summary: String,
}

/// One unit of work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub kind: BatchKind,
    pub collection: String,
    pub rel_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub nodes: Vec<Item>,
}

/// What an agent sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub id: String,
    pub summary: String,
    pub description: String,
}

/// Finds the next batch.
///
/// Files come first, the deepest one first, because a directory is a summary
/// of what it holds and cannot be written before them. The root comes last and
/// only when it is asked for: its hash follows every file of the repository,
/// so it would come back on every commit and the queue would never empty.
pub fn next(db: &Database, collection: &str, update_root: bool) -> Result<Option<Batch>> {
    Ok(next_batches(db, collection, update_root, 1)?
        .into_iter()
        .next())
}

/// Finds up to `limit` batches, in the same order as [`next`]: files before
/// directories before the root.
///
/// The queue is a read query, not a lease: nothing marks a batch as taken.
/// Two callers that both list batches before either calls [`describe`] can
/// therefore see the same file, and a caller that hands these out to parallel
/// workers should expect the rare duplicate rather than treat it as a bug.
pub fn next_batches(
    db: &Database,
    collection: &str,
    update_root: bool,
    limit: usize,
) -> Result<Vec<Batch>> {
    let collection_id = collection_id(db, collection)?;
    let mut batches = Vec::new();

    for rel_path in next_files(db, collection_id, limit)? {
        batches.push(file_batch(db, collection_id, collection, &rel_path)?);
    }

    if batches.len() < limit {
        let remaining = limit - batches.len();
        for rel_path in next_dirs(db, collection_id, remaining)? {
            batches.push(dir_batch(db, collection_id, collection, &rel_path)?);
        }
    }

    if batches.len() < limit
        && update_root
        && let Some(batch) = root_batch(db, collection_id, collection)?
    {
        batches.push(batch);
    }

    Ok(batches)
}

/// How many nodes of a collection still wait, by kind.
pub fn status(db: &Database, collection: &str) -> Result<Vec<(String, usize, usize)>> {
    let collection_id = collection_id(db, collection)?;
    let mut stmt = db.conn().prepare(
        "SELECT n.kind, COUNT(*), SUM(CASE WHEN s.id IS NULL THEN 1 ELSE 0 END)
         FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ?
         GROUP BY n.kind
         ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt.query_map([collection_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as usize,
            row.get::<_, i64>(2)? as usize,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Takes the summaries that an agent wrote.
///
/// A node whose id is unknown is refused. That is the guard against a write
/// that describes code which has since moved: a pass of `index` replaces the
/// nodes of every file that changed, and the ids of the old ones go with them.
/// The node then comes back in the queue instead of carrying a summary that
/// was written for other bytes.
pub fn describe(
    db: &Database,
    collection: &str,
    written: &[Written],
    author: &str,
) -> Result<usize> {
    let collection_id = collection_id(db, collection)?;
    let now = Utc::now().to_rfc3339();
    let mut count = 0;

    for entry in written {
        if entry.summary.trim().is_empty() || entry.description.trim().is_empty() {
            return Err(Error::BadArgument(format!(
                "{}: a summary needs both a `summary` and a `description`",
                entry.id
            )));
        }

        let pool_key: Option<String> = db
            .conn()
            .query_row(
                "SELECT pool_key FROM nodes WHERE collection_id = ? AND ulid = ?",
                params![collection_id, entry.id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(pool_key) = pool_key else {
            return Err(Error::NoSuchNode(entry.id.clone()));
        };

        db.conn().execute(
            "INSERT INTO summaries(pool_key, summary, description, author, written_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(pool_key) DO UPDATE SET
                 summary = ?2, description = ?3, author = ?4, written_at = ?5",
            params![
                pool_key,
                entry.summary.trim(),
                entry.description.trim(),
                author,
                now
            ],
        )?;
        count += 1;
    }
    Ok(count)
}

/// The row of a collection, or a message that says how to make one.
pub fn collection_id(db: &Database, name: &str) -> Result<i64> {
    db.conn()
        .query_row("SELECT id FROM collections WHERE name = ?", [name], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| Error::NoSuchCollection(name.to_string()))
}

/// The `limit` distinct files with a node that has no summary, the file that
/// holds the deepest one first.
fn next_files(db: &Database, collection_id: i64, limit: usize) -> Result<Vec<String>> {
    let mut stmt = db.conn().prepare(
        "SELECT n.rel_path FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ? AND n.kind <> 'dir' AND n.kind <> 'repo'
               AND s.id IS NULL
         GROUP BY n.rel_path
         ORDER BY MAX(n.depth) DESC, n.rel_path
         LIMIT ?",
    )?;
    let rows = stmt.query_map(params![collection_id, limit as i64], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The `limit` distinct directories with no summary whose children all have
/// one, the deepest first.
fn next_dirs(db: &Database, collection_id: i64, limit: usize) -> Result<Vec<String>> {
    let mut stmt = db.conn().prepare(
        "SELECT n.rel_path FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ? AND n.kind = 'dir' AND s.id IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM nodes child
                   LEFT JOIN summaries cs ON cs.pool_key = child.pool_key
                   WHERE child.parent_id = n.id AND cs.id IS NULL
               )
         ORDER BY n.depth DESC, n.rel_path
         LIMIT ?",
    )?;
    let rows = stmt.query_map(params![collection_id, limit as i64], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every node of one file that waits, the deepest first and the file last.
fn file_batch(
    db: &Database,
    collection_id: i64,
    collection: &str,
    rel_path: &str,
) -> Result<Batch> {
    let language: Option<String> = db
        .conn()
        .query_row(
            "SELECT language FROM nodes WHERE collection_id = ? AND rel_path = ? AND kind = 'file'",
            params![collection_id, rel_path],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let mut stmt = db.conn().prepare(
        "SELECT n.id, n.ulid, n.kind, n.name, n.qualified_name, n.start_line, n.end_line
         FROM nodes n
         LEFT JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.collection_id = ? AND n.rel_path = ? AND n.kind <> 'dir' AND s.id IS NULL
         ORDER BY n.depth DESC, n.start_line",
    )?;
    let rows = stmt.query_map(params![collection_id, rel_path], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            Item {
                id: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                qualified_name: row.get(4)?,
                lines: match (row.get::<_, Option<u32>>(5)?, row.get::<_, Option<u32>>(6)?) {
                    (Some(from), Some(to)) => Some([from, to]),
                    _ => None,
                },
                children: Vec::new(),
            },
        ))
    })?;

    let mut nodes = Vec::new();
    for row in rows {
        let (id, mut item) = row?;
        item.children = described_children(db, id)?;
        nodes.push(item);
    }

    Ok(Batch {
        kind: BatchKind::File,
        collection: collection.to_string(),
        rel_path: rel_path.to_string(),
        language,
        nodes,
    })
}

/// One directory, with the summaries of what it holds.
fn dir_batch(db: &Database, collection_id: i64, collection: &str, rel_path: &str) -> Result<Batch> {
    let (id, ulid, name, qualified_name): (i64, String, String, String) = db.conn().query_row(
        "SELECT id, ulid, name, qualified_name FROM nodes
         WHERE collection_id = ? AND kind = 'dir' AND rel_path = ?",
        params![collection_id, rel_path],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;

    Ok(Batch {
        kind: BatchKind::Dir,
        collection: collection.to_string(),
        rel_path: rel_path.to_string(),
        language: None,
        nodes: vec![Item {
            id: ulid,
            kind: NodeKind::Dir.as_str().to_string(),
            name,
            qualified_name,
            lines: None,
            children: described_children(db, id)?,
        }],
    })
}

/// The root, when the caller asked for it and it still waits.
fn root_batch(db: &Database, collection_id: i64, collection: &str) -> Result<Option<Batch>> {
    let row: Option<(i64, String)> = db
        .conn()
        .query_row(
            "SELECT n.id, n.ulid FROM nodes n
             LEFT JOIN summaries s ON s.pool_key = n.pool_key
             WHERE n.collection_id = ? AND n.kind = 'repo' AND s.id IS NULL",
            [collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, ulid)) = row else {
        return Ok(None);
    };

    Ok(Some(Batch {
        kind: BatchKind::Dir,
        collection: collection.to_string(),
        rel_path: String::new(),
        language: None,
        nodes: vec![Item {
            id: ulid,
            kind: NodeKind::Repo.as_str().to_string(),
            name: collection
                .rsplit('/')
                .next()
                .unwrap_or(collection)
                .to_string(),
            qualified_name: String::new(),
            lines: None,
            children: described_children(db, id)?,
        }],
    }))
}

/// The children of one node that already carry a summary.
fn described_children(db: &Database, parent: i64) -> Result<Vec<Child>> {
    let mut stmt = db.conn().prepare(
        "SELECT n.name, n.kind, s.summary FROM nodes n
         JOIN summaries s ON s.pool_key = n.pool_key
         WHERE n.parent_id = ?
         ORDER BY n.qualified_name",
    )?;
    let rows = stmt.query_map([parent], |row| {
        Ok(Child {
            name: row.get(0)?,
            kind: row.get(1)?,
            summary: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::index::index;
    use crate::config::{CodeConfig, Config};

    struct Repo {
        root: std::path::PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("embornal-queue-{name}"));
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

    /// An index over a repository, already walked once.
    fn indexed(repo: &Repo) -> Database {
        let mut db = Database::open_in_memory(&Config::default()).unwrap();
        index(&mut db, &repo.root, "test", &CodeConfig::default(), false).unwrap();
        db
    }

    /// Writes a summary for every node of one batch.
    fn answer(db: &Database, batch: &Batch) -> usize {
        let written: Vec<Written> = batch
            .nodes
            .iter()
            .map(|item| Written {
                id: item.id.clone(),
                summary: format!("Summary of {}.", item.name),
                description: format!("A description of what {} does.", item.name),
            })
            .collect();
        describe(db, "test", &written, "default").unwrap()
    }

    /// Empties the queue, giving back the batches in the order that they came.
    fn drain(db: &Database, update_root: bool) -> Vec<Batch> {
        let mut batches = Vec::new();
        while let Some(batch) = next(db, "test", update_root).unwrap() {
            answer(db, &batch);
            batches.push(batch);
        }
        batches
    }

    #[test]
    fn one_batch_is_one_file_and_holds_every_node_of_it() {
        let repo = Repo::new("onefile");
        repo.write("src/a.rs", "struct M;\nimpl M {\n    fn open() {}\n}\n");
        let db = indexed(&repo);

        let batch = next(&db, "test", false).unwrap().unwrap();
        assert_eq!(batch.kind, BatchKind::File);
        assert_eq!(batch.rel_path, "src/a.rs");
        assert_eq!(batch.language.as_deref(), Some("rust"));

        let names: Vec<&str> = batch.nodes.iter().map(|n| n.name.as_str()).collect();
        // The file itself comes last, after everything that it holds.
        assert_eq!(names.last(), Some(&"a.rs"));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn a_batch_says_which_lines_to_read_and_carries_no_source() {
        let repo = Repo::new("nosource");
        repo.write("src/a.rs", "\nfn a() {\n    work()\n}\n");
        let db = indexed(&repo);

        let batch = next(&db, "test", false).unwrap().unwrap();
        let function = batch.nodes.iter().find(|n| n.name == "a").unwrap();
        assert_eq!(function.lines, Some([2, 4]));

        // The payload names the file and the lines. The agent opens the file.
        let json = serde_json::to_string(&batch).unwrap();
        assert!(json.contains("src/a.rs"), "{json}");
        assert!(
            !json.contains("work()"),
            "the payload must not carry source"
        );
    }

    #[test]
    fn files_come_before_directories_and_the_deepest_file_first() {
        let repo = Repo::new("order");
        repo.write("src/memory/api.rs", "fn deep() {}\n");
        repo.write("top.rs", "fn top() {}\n");
        let db = indexed(&repo);

        let batches = drain(&db, false);
        let order: Vec<String> = batches
            .iter()
            .map(|b| format!("{:?} {}", b.kind, b.rel_path))
            .collect();
        assert_eq!(
            order,
            vec![
                "File src/memory/api.rs",
                "File top.rs",
                "Dir src/memory",
                "Dir src",
            ]
        );
    }

    #[test]
    fn a_directory_arrives_only_after_everything_it_holds_is_described() {
        let repo = Repo::new("bottomup");
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("src/b.rs", "fn b() {}\n");
        let db = indexed(&repo);

        // The first two batches are the two files. Only then does `src` come.
        let first = next(&db, "test", false).unwrap().unwrap();
        assert_eq!(first.kind, BatchKind::File);
        answer(&db, &first);

        let second = next(&db, "test", false).unwrap().unwrap();
        assert_eq!(second.kind, BatchKind::File);
        answer(&db, &second);

        let third = next(&db, "test", false).unwrap().unwrap();
        assert_eq!(third.kind, BatchKind::Dir);
        assert_eq!(third.rel_path, "src");
    }

    #[test]
    fn a_directory_batch_carries_the_summaries_of_what_it_holds() {
        let repo = Repo::new("children");
        repo.write("src/a.rs", "fn a() {}\n");
        let db = indexed(&repo);

        answer(&db, &next(&db, "test", false).unwrap().unwrap());
        let batch = next(&db, "test", false).unwrap().unwrap();

        assert_eq!(batch.kind, BatchKind::Dir);
        assert_eq!(batch.nodes[0].children.len(), 1);
        assert_eq!(batch.nodes[0].children[0].name, "a.rs");
        assert!(batch.nodes[0].children[0].summary.contains("a.rs"));
        // There is no file to open, so there are no lines.
        assert_eq!(batch.nodes[0].lines, None);
    }

    #[test]
    fn the_root_stays_out_of_the_queue_unless_it_is_asked_for() {
        let repo = Repo::new("root");
        repo.write("src/a.rs", "fn a() {}\n");
        let db = indexed(&repo);

        // Without the flag the queue empties, and the root is what is left.
        drain(&db, false);
        assert!(next(&db, "test", false).unwrap().is_none());

        let root = next(&db, "test", true).unwrap().unwrap();
        assert_eq!(root.kind, BatchKind::Dir);
        assert_eq!(root.rel_path, "");
        answer(&db, &root);
        assert!(next(&db, "test", true).unwrap().is_none());
    }

    #[test]
    fn the_queue_empties() {
        let repo = Repo::new("empties");
        repo.write("src/memory/api.rs", "struct M;\nimpl M { fn a() {} }\n");
        repo.write("src/main.rs", "fn main() {}\n");
        let db = indexed(&repo);

        assert!(!drain(&db, true).is_empty());
        assert!(next(&db, "test", true).unwrap().is_none());

        let waiting: usize = status(&db, "test")
            .unwrap()
            .iter()
            .map(|(_, _, stale)| stale)
            .sum();
        assert_eq!(waiting, 0);
    }

    #[test]
    fn a_summary_written_once_answers_after_the_file_is_indexed_again() {
        let repo = Repo::new("reindex");
        repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
        let mut db = indexed(&repo);
        drain(&db, true);

        // A change to one function reopens it, the file and the directories,
        // and leaves its sibling alone.
        repo.write("src/a.rs", "fn a() { work() }\nfn b() {}\n");
        let report = index(&mut db, &repo.root, "test", &CodeConfig::default(), false).unwrap();
        assert_eq!(report.files_parsed, 1);

        let batch = next(&db, "test", false).unwrap().unwrap();
        let waiting: Vec<&str> = batch.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(waiting, vec!["a", "a.rs"], "b keeps its summary");
    }

    #[test]
    fn a_node_that_the_index_no_longer_holds_is_refused() {
        // The id is the token. A pass of `index` replaces the nodes of a file
        // that changed, so a write against an older id cannot land.
        let repo = Repo::new("stale");
        repo.write("src/a.rs", "fn a() {}\n");
        let mut db = indexed(&repo);

        let batch = next(&db, "test", false).unwrap().unwrap();
        let id = batch.nodes[0].id.clone();

        repo.write("src/a.rs", "fn a() { other() }\n");
        index(&mut db, &repo.root, "test", &CodeConfig::default(), false).unwrap();

        let error = describe(
            &db,
            "test",
            &[Written {
                id: id.clone(),
                summary: "Written against the older bytes.".into(),
                description: "This must not land.".into(),
            }],
            "default",
        )
        .unwrap_err();
        assert!(
            matches!(error, Error::NoSuchNode(ref got) if *got == id),
            "{error}"
        );
    }

    #[test]
    fn a_summary_needs_both_of_its_texts() {
        let repo = Repo::new("halves");
        repo.write("src/a.rs", "fn a() {}\n");
        let db = indexed(&repo);
        let batch = next(&db, "test", false).unwrap().unwrap();

        let error = describe(
            &db,
            "test",
            &[Written {
                id: batch.nodes[0].id.clone(),
                summary: "Only half of it.".into(),
                description: "   ".into(),
            }],
            "default",
        )
        .unwrap_err();
        assert!(matches!(error, Error::BadArgument(_)), "{error}");
    }

    #[test]
    fn describing_a_node_a_second_time_replaces_what_it_said() {
        let repo = Repo::new("rewrite");
        repo.write("src/a.rs", "fn a() {}\n");
        let db = indexed(&repo);
        let batch = next(&db, "test", false).unwrap().unwrap();
        let id = batch.nodes[0].id.clone();

        for text in ["The older word.", "The newer word."] {
            describe(
                &db,
                "test",
                &[Written {
                    id: id.clone(),
                    summary: text.into(),
                    description: "A description.".into(),
                }],
                "agent",
            )
            .unwrap();
        }

        let (summary, author): (String, String) = db
            .conn()
            .query_row(
                "SELECT s.summary, s.author FROM summaries s
                 JOIN nodes n ON n.pool_key = s.pool_key WHERE n.ulid = ?",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(summary, "The newer word.");
        assert_eq!(author, "agent");
    }

    #[test]
    fn a_collection_that_was_never_indexed_says_so() {
        let db = Database::open_in_memory(&Config::default()).unwrap();
        let error = next(&db, "never", false).unwrap_err();
        assert!(matches!(error, Error::NoSuchCollection(_)), "{error}");
    }

    #[test]
    fn status_counts_what_waits_by_kind() {
        let repo = Repo::new("status");
        repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
        let db = indexed(&repo);

        let before = status(&db, "test").unwrap();
        let functions = before
            .iter()
            .find(|(kind, _, _)| kind == "function")
            .unwrap();
        assert_eq!((functions.1, functions.2), (2, 2));

        drain(&db, true);
        let after = status(&db, "test").unwrap();
        assert!(after.iter().all(|(_, _, stale)| *stale == 0), "{after:?}");
    }
}
