//! The database of the code index.
//!
//! One SQLite file holds every repository. A collection inside it names one
//! repository, and the nodes of that collection hold its tree. The summaries
//! sit apart from the collections, because a summary belongs to the code and
//! not to the checkout that happened to reach it first.

use crate::common::sqlite::{self, Migration};
use crate::config::Config;
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// The schema version that this build writes.
pub const SCHEMA_VERSION: i64 = 1;

/// The key under which the width of the vector index is recorded.
pub const META_EMBEDDING_DIMENSIONS: &str = "embedding_dimensions";

/// The name of the vector index.
pub const VEC_TABLE: &str = "summaries_vec";

const MIGRATION_001: &str = include_str!("schema/001_init.sql");

/// The steps of this schema, in order. The last one names [`SCHEMA_VERSION`].
const MIGRATIONS: [Migration; 1] = [Migration {
    version: 1,
    sql: MIGRATION_001,
}];

/// An open code index.
pub struct Database {
    conn: Connection,
    dimensions: usize,
}

impl Database {
    /// Opens the file that the configuration names, and prepares it.
    pub fn open(path: &Path, config: &Config) -> Result<Self> {
        sqlite::register_vec_extension();
        let conn = Connection::open(path)?;
        Self::prepare(conn, config)
    }

    /// Opens an index that lives in RAM. The tests use this.
    pub fn open_in_memory(config: &Config) -> Result<Self> {
        sqlite::register_vec_extension();
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn, config)
    }

    fn prepare(conn: Connection, config: &Config) -> Result<Self> {
        sqlite::prepare(&conn)?;

        let mut db = Self {
            conn,
            dimensions: config.embedding.dimensions,
        };
        db.migrate()?;
        db.check_dimensions()?;
        db.ensure_vector_index()?;
        Ok(db)
    }

    /// Returns the connection, for the modules that build queries.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Returns the width of the vector index.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// The version that the file carries. The tests read it to check that a
    /// migration ran.
    #[cfg(test)]
    fn schema_version(&self) -> Result<i64> {
        sqlite::schema_version(&self.conn)
    }

    /// Applies the migrations that the file still needs.
    ///
    /// The index has nothing to seed: an empty index is an index of no
    /// repository, and the first `code index` makes the first collection.
    fn migrate(&mut self) -> Result<()> {
        sqlite::migrate(&mut self.conn, &MIGRATIONS, |_, _| Ok(()))
    }

    /// Compares the width in the file with the width in the configuration.
    ///
    /// A vector index has a fixed width. If the two disagree, the search gives
    /// wrong answers, so the index stops instead.
    fn check_dimensions(&self) -> Result<()> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?",
                [META_EMBEDDING_DIMENSIONS],
                |row| row.get(0),
            )
            .optional()?;

        match stored {
            Some(text) => {
                let stored: usize = text.parse().unwrap_or_default();
                if stored != self.dimensions {
                    return Err(Error::EmbeddingDimensionsMismatch {
                        stored,
                        configured: self.dimensions,
                    });
                }
            }
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES (?, ?)",
                    params![META_EMBEDDING_DIMENSIONS, self.dimensions.to_string()],
                )?;
            }
        }
        Ok(())
    }

    /// Builds the vector index if it is absent.
    ///
    /// The width comes from the configuration, so this statement cannot live
    /// in the migration file.
    fn ensure_vector_index(&self) -> Result<()> {
        self.conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {VEC_TABLE} USING vec0(
                 summary_id INTEGER PRIMARY KEY,
                 embedding  FLOAT[{}]
             );",
            self.dimensions
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::node::{ContentHash, PoolKey};

    fn db() -> Database {
        Database::open_in_memory(&Config::default()).unwrap()
    }

    /// Writes one collection and gives back its row number.
    fn collection(db: &Database, name: &str) -> i64 {
        db.conn()
            .execute(
                "INSERT INTO collections(ulid, name, root, created_at)
                 VALUES (?, ?, ?, '2026-09-03T00:00:00Z')",
                params![ulid::Ulid::generate().to_string(), name, name],
            )
            .unwrap();
        db.conn().last_insert_rowid()
    }

    /// Writes the root node of a collection and gives back its row number.
    fn root(db: &Database, collection_id: i64) -> i64 {
        let hash = ContentHash::of_children([]);
        db.conn()
            .execute(
                "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name,
                                   qualified_name, rel_path, depth,
                                   content_hash, pool_key)
                 VALUES (?, ?, NULL, 'repo', '', '', '', 0, ?, ?)",
                params![
                    ulid::Ulid::generate().to_string(),
                    collection_id,
                    hash.as_str(),
                    PoolKey::new("", &hash).as_str(),
                ],
            )
            .unwrap();
        db.conn().last_insert_rowid()
    }

    fn summary(db: &Database, key: &PoolKey, summary: &str) {
        db.conn()
            .execute(
                "INSERT INTO summaries(pool_key, summary, description, author, written_at)
                 VALUES (?, ?, 'a longer description', 'default', '2026-09-03T00:00:00Z')",
                params![key.as_str(), summary],
            )
            .unwrap();
    }

    #[test]
    fn a_new_file_reaches_the_current_version() {
        assert_eq!(db().schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn opening_twice_changes_nothing() {
        let dir = std::env::temp_dir().join("embornal-code-twice");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("code.db");

        let config = Config::default();
        let first = Database::open(&file, &config).unwrap();
        let id = collection(&first, "/repo");
        drop(first);

        let second = Database::open(&file, &config).unwrap();
        assert_eq!(second.schema_version().unwrap(), SCHEMA_VERSION);
        let names: Vec<String> = second
            .conn()
            .prepare("SELECT name FROM collections")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(names, vec!["/repo".to_string()]);
        assert!(id > 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_vector_index_has_the_configured_width() {
        let mut config = Config::default();
        config.embedding.dimensions = 64;
        let db = Database::open_in_memory(&config).unwrap();
        assert_eq!(db.dimensions(), 64);

        // A vector of another width does not fit in the index.
        let short: Vec<f32> = vec![0.0; 8];
        let wrong = db.conn().execute(
            &format!("INSERT INTO {VEC_TABLE}(summary_id, embedding) VALUES (1, ?)"),
            params![crate::embedding::to_blob(&short)],
        );
        assert!(wrong.is_err());
    }

    #[test]
    fn a_changed_width_stops_the_index() {
        let dir = std::env::temp_dir().join("embornal-code-width");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("code.db");

        let mut config = Config::default();
        config.embedding.dimensions = 64;
        drop(Database::open(&file, &config).unwrap());

        config.embedding.dimensions = 128;
        let error = match Database::open(&file, &config) {
            Err(error) => error,
            Ok(_) => panic!("the index opened with a width that it cannot hold"),
        };
        assert!(
            matches!(
                error,
                Error::EmbeddingDimensionsMismatch {
                    stored: 64,
                    configured: 128
                }
            ),
            "{error}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_the_root_of_a_collection_has_no_parent() {
        let db = db();
        let id = collection(&db, "/repo");
        root(&db, id);

        // A second parentless node would be a second root.
        let hash = ContentHash::of_bytes(b"x");
        let second = db.conn().execute(
            "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name,
                               qualified_name, rel_path, depth, content_hash, pool_key)
             VALUES (?, ?, NULL, 'dir', 'src', 'src', 'src', 1, ?, ?)",
            params![
                ulid::Ulid::generate().to_string(),
                id,
                hash.as_str(),
                PoolKey::new("src", &hash).as_str()
            ],
        );
        assert!(second.is_err());
    }

    #[test]
    fn two_nodes_of_one_collection_cannot_share_a_name() {
        let db = db();
        let id = collection(&db, "/repo");
        let parent = root(&db, id);
        let hash = ContentHash::of_bytes(b"fn a() {}");

        let insert = |name: &str| {
            db.conn().execute(
                "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name,
                                   qualified_name, rel_path, depth, content_hash, pool_key)
                 VALUES (?, ?, ?, 'function', 'a', ?, 'src/a.rs', 2, ?, ?)",
                params![
                    ulid::Ulid::generate().to_string(),
                    id,
                    parent,
                    name,
                    hash.as_str(),
                    PoolKey::new(name, &hash).as_str()
                ],
            )
        };
        insert("src/a.rs::a").unwrap();
        assert!(insert("src/a.rs::a").is_err());
        // Another collection may hold the very same name.
        let other = collection(&db, "/other");
        let other_root = root(&db, other);
        assert!(
            db.conn()
                .execute(
                    "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name,
                                       qualified_name, rel_path, depth, content_hash, pool_key)
                     VALUES (?, ?, ?, 'function', 'a', 'src/a.rs::a', 'src/a.rs', 2, ?, ?)",
                    params![
                        ulid::Ulid::generate().to_string(),
                        other,
                        other_root,
                        hash.as_str(),
                        PoolKey::new("src/a.rs::a", &hash).as_str()
                    ],
                )
                .is_ok()
        );
    }

    #[test]
    fn dropping_a_collection_takes_its_tree_with_it() {
        let db = db();
        let id = collection(&db, "/repo");
        root(&db, id);
        db.conn()
            .execute("DELETE FROM collections WHERE id = ?", [id])
            .unwrap();

        let left: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_summary_needs_both_of_its_texts() {
        let db = db();
        let key = PoolKey::new("src/a.rs::a", &ContentHash::of_bytes(b"fn a() {}"));
        let empty = db.conn().execute(
            "INSERT INTO summaries(pool_key, summary, description, author, written_at)
             VALUES (?, '', 'text', 'default', '2026-09-03T00:00:00Z')",
            params![key.as_str()],
        );
        assert!(empty.is_err());
    }

    #[test]
    fn one_summary_answers_for_every_collection_that_holds_the_code() {
        let db = db();
        let hash = ContentHash::of_bytes(b"fn a() {}");
        let key = PoolKey::new("src/a.rs::a", &hash);
        summary(&db, &key, "Does the thing.");

        // Two collections, one and the same piece of code in both.
        for name in ["/repo", "/repo-fork"] {
            let id = collection(&db, name);
            let parent = root(&db, id);
            db.conn()
                .execute(
                    "INSERT INTO nodes(ulid, collection_id, parent_id, kind, name,
                                       qualified_name, rel_path, depth, content_hash, pool_key)
                     VALUES (?, ?, ?, 'function', 'a', 'src/a.rs::a', 'src/a.rs', 2, ?, ?)",
                    params![
                        ulid::Ulid::generate().to_string(),
                        id,
                        parent,
                        hash.as_str(),
                        key.as_str()
                    ],
                )
                .unwrap();
        }

        // The query that says what is stale: nothing is, in either of them.
        let stale: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM nodes n
                 LEFT JOIN summaries s ON s.pool_key = n.pool_key
                 WHERE n.kind = 'function' AND s.id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
    }

    #[test]
    fn the_keyword_index_follows_the_summaries() {
        let db = db();
        let hash = ContentHash::of_bytes(b"fn check(t: &Token) {}");
        let key = PoolKey::new("src/token.rs::check", &hash);
        summary(&db, &key, "Checks that a token opens the memory.");

        let found: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM summaries_fts WHERE summaries_fts MATCH 'token'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1);

        // A summary that goes takes its row of the index with it.
        db.conn()
            .execute("DELETE FROM summaries WHERE pool_key = ?", [key.as_str()])
            .unwrap();
        let left: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM summaries_fts WHERE summaries_fts MATCH 'token'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_rewritten_summary_leaves_no_older_copy_in_the_index() {
        let db = db();
        let hash = ContentHash::of_bytes(b"fn a() {}");
        let key = PoolKey::new("src/a.rs::a", &hash);
        summary(&db, &key, "Reads the older word.");

        db.conn()
            .execute(
                "UPDATE summaries SET summary = 'Reads the newer word.' WHERE pool_key = ?",
                [key.as_str()],
            )
            .unwrap();

        let older: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM summaries_fts WHERE summaries_fts MATCH 'older'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let newer: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM summaries_fts WHERE summaries_fts MATCH 'newer'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((older, newer), (0, 1));
    }
}
