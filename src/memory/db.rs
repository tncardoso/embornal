//! The database.
//!
//! One SQLite file holds the tree, the facts, the two search indexes and the
//! access rules. The file is the unit of backup: to move a memory, copy it.

use crate::common::sqlite::{self, Migration};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::memory::acl::{Action, Effect, PolicyObject, Subject};
use crate::memory::fact::INITIAL_STABILITY_DAYS;
use crate::memory::path::{MEMORY_PATH, ROOT_ID, WikiPath};
use crate::memory::time;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use ulid::Ulid;

/// The schema version that this build writes.
pub const SCHEMA_VERSION: i64 = 2;

/// The key under which the width of the vector index is recorded.
pub const META_EMBEDDING_DIMENSIONS: &str = "embedding_dimensions";

/// The name of the vector index.
pub const VEC_TABLE: &str = "facts_vec";

const MIGRATION_001: &str = include_str!("schema/001_init.sql");
const MIGRATION_002: &str = include_str!("schema/002_owner_tokens.sql");

/// The steps of this schema, in order. The last one names [`SCHEMA_VERSION`].
const MIGRATIONS: [Migration; 2] = [
    Migration {
        version: 1,
        sql: MIGRATION_001,
    },
    Migration {
        version: 2,
        sql: MIGRATION_002,
    },
];

/// The facts that a new memory knows about itself.
///
/// The memory carries its own instructions, so an agent that reads
/// `/memory` learns how to use the rest of the tree.
const MEMORY_SEED_TEXT: &str = include_str!("../prompts/memory.txt");

fn memory_seed() -> Vec<&'static str> {
    MEMORY_SEED_TEXT
        .lines()
        .filter(|line| !line.is_empty())
        .collect()
}

pub const MEMORY_SEED_LEN: usize = 6;

/// An open memory database.
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

    /// Opens a memory that lives in RAM. The tests use this.
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
    /// The first step builds the tree and the tables; the memory then writes
    /// the facts that it knows about itself, so that an agent that reads
    /// `/memory` learns how to use the rest of the tree.
    fn migrate(&mut self) -> Result<()> {
        sqlite::migrate(&mut self.conn, &MIGRATIONS, |tx, version| {
            if version == 1 { seed(tx) } else { Ok(()) }
        })
    }

    /// Compares the width in the file with the width in the configuration.
    ///
    /// A vector index has a fixed width. If the two disagree, the search
    /// gives wrong answers, so the memory stops instead.
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
                 fact_id   INTEGER PRIMARY KEY,
                 embedding FLOAT[{}]
             );",
            self.dimensions
        ))?;
        Ok(())
    }
}

/// Writes the rows that a new memory needs: the root, `/memory` with its own
/// instructions, and the rules that let the command line work.
fn seed(tx: &Connection) -> Result<()> {
    let now = Utc::now();
    let stamp = time::to_sql(now);

    tx.execute(
        "INSERT INTO paths(id, ulid, parent_id, segment, full_path, created_at)
         VALUES (?, ?, NULL, '', '/', ?)",
        params![ROOT_ID.0, Ulid::generate().to_string(), stamp],
    )?;

    let memory_path = WikiPath::parse(MEMORY_PATH).expect("the constant path is valid");
    tx.execute(
        "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![
            Ulid::generate().to_string(),
            ROOT_ID.0,
            memory_path.segment().expect("the path is not the root"),
            memory_path.as_str(),
            stamp
        ],
    )?;
    let memory_id = tx.last_insert_rowid();

    let mut insert_fact = tx.prepare(
        "INSERT INTO facts(ulid, path_id, content, created_at, stability_days)
         VALUES (?, ?, ?, ?, ?)",
    )?;
    for content in memory_seed() {
        insert_fact.execute(params![
            Ulid::generate().to_string(),
            memory_id,
            content,
            stamp,
            INITIAL_STABILITY_DAYS
        ])?;
    }
    drop(insert_fact);

    // The command line owns the whole tree until real subjects exist.
    let everything = PolicyObject::parse("path:/*")
        .expect("the constant object is valid")
        .to_string();
    let subject = Subject::cli().to_string();
    let mut insert_rule =
        tx.prepare("INSERT INTO casbin_rule(ptype, v0, v1, v2, v3) VALUES ('p', ?, ?, ?, ?)")?;
    for action in Action::ALL {
        insert_rule.execute(params![
            subject,
            everything,
            action.as_str(),
            Effect::Allow.as_str()
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::acl::{AccessFilter, DEFAULT_SUBJECT, EVERYONE_ROLE, PolicyRule};

    fn db() -> Database {
        Database::open_in_memory(&Config::default()).unwrap()
    }

    #[test]
    fn a_new_file_reaches_the_current_version() {
        let db = db();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    /// Builds a file with the schema of version 1 and one fact in it, the way
    /// an older build left it.
    fn version_one_file(name: &str) -> std::path::PathBuf {
        let file = std::env::temp_dir().join(format!("embornal-v1-{name}.db"));
        std::fs::remove_file(&file).ok();

        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        seed(&conn).unwrap();
        conn.execute(
            "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
             VALUES (?, 1, 'notes', '/notes', ?)",
            params![Ulid::generate().to_string(), time::to_sql(Utc::now())],
        )
        .unwrap();
        let path_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO facts(ulid, path_id, content, created_at) VALUES (?, ?, ?, ?)",
            params![
                Ulid::generate().to_string(),
                path_id,
                "a fact from before",
                time::to_sql(Utc::now())
            ],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);
        file
    }

    #[test]
    fn a_file_of_version_one_keeps_its_facts_and_gains_an_owner() {
        let file = version_one_file("owner");
        let db = Database::open(&file, &Config::default()).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);

        // The fact that the older build wrote belongs to the subject that
        // could write it, and it is still there.
        let (content, owner, tag): (String, String, String) = db
            .conn()
            .query_row(
                "SELECT f.content, f.owner, t.value
                   FROM facts f
                   JOIN fact_tags t ON t.fact_id = f.id AND t.key = 'owner'
                  WHERE f.content = 'a fact from before'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "a fact from before");
        assert_eq!(owner, DEFAULT_SUBJECT);
        assert_eq!(tag, DEFAULT_SUBJECT);

        // The facts about the memory itself use the same default owner.
        let seeded: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM facts f
                  JOIN paths p ON p.id = f.path_id
                 WHERE f.owner = ? AND p.full_path = '/memory'",
                [DEFAULT_SUBJECT],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seeded as usize, MEMORY_SEED_LEN);

        // No fact is left without an owner.
        let orphans: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM facts WHERE owner IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_file_of_version_one_keeps_the_access_that_it_had() {
        let file = version_one_file("access");
        let db = Database::open(&file, &Config::default()).unwrap();

        // The one subject of that file still reads everything, so a memory on
        // one machine works exactly as it did.
        let rules = rules_of(&db, DEFAULT_SUBJECT);
        for action in Action::ALL {
            assert!(AccessFilter::build(&rules, action).is_unrestricted());
        }

        // It also joins the role that reads the facts about the memory.
        let joined: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM casbin_rule WHERE ptype = 'g' AND v0 = ? AND v1 = ?",
                params![DEFAULT_SUBJECT, EVERYONE_ROLE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(joined, 1);

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_second_open_of_a_file_migrates_nothing_again() {
        let file = version_one_file("twice");
        drop(Database::open(&file, &Config::default()).unwrap());
        let db = Database::open(&file, &Config::default()).unwrap();

        // A migration that ran a second time would double the tags.
        let tags: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM fact_tags WHERE key = 'owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let facts: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tags, facts);

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn the_root_is_row_one() {
        let db = db();
        let (id, full_path): (i64, String) = db
            .conn()
            .query_row(
                "SELECT id, full_path FROM paths WHERE parent_id IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(id, ROOT_ID.0);
        assert_eq!(full_path, "/");
    }

    #[test]
    fn a_second_root_is_refused() {
        let db = db();
        let result = db.conn().execute(
            "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
             VALUES ('x', NULL, '', '/other', '2026-01-01T00:00:00.000000Z')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn two_children_cannot_share_a_name() {
        let db = db();
        let insert = |segment: &str, full: &str| {
            db.conn().execute(
                "INSERT INTO paths(ulid, parent_id, segment, full_path, created_at)
                 VALUES (?, 1, ?, ?, '2026-01-01T00:00:00.000000Z')",
                params![Ulid::generate().to_string(), segment, full],
            )
        };
        assert!(insert("work", "/work").is_ok());
        assert!(insert("work", "/work-again").is_err());
    }

    #[test]
    fn the_memory_path_carries_its_instructions() {
        let db = db();
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id
                 WHERE p.full_path = ?",
                [MEMORY_PATH],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count as usize, MEMORY_SEED_LEN);
    }

    /// Reads the `p` rules of one subject out of the policy table.
    fn rules_of(db: &Database, subject: &str) -> Vec<PolicyRule> {
        let mut stmt = db
            .conn()
            .prepare("SELECT v0, v1, v2, v3 FROM casbin_rule WHERE ptype = 'p' AND v0 = ?")
            .unwrap();
        stmt.query_map([subject], |row| {
            Ok(vec![
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ])
        })
        .unwrap()
        .map(|fields| PolicyRule::from_casbin(&fields.unwrap()).unwrap())
        .collect()
    }

    #[test]
    fn the_command_line_starts_with_full_access() {
        let db = db();
        let rules = rules_of(&db, DEFAULT_SUBJECT);

        assert_eq!(rules.len(), 3);
        for action in Action::ALL {
            assert!(AccessFilter::build(&rules, action).is_unrestricted());
        }
    }

    #[test]
    fn every_subject_reads_public_facts() {
        let db = db();
        let rules = rules_of(&db, EVERYONE_ROLE);

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, Action::Read);
        assert_eq!(rules[0].effect, Effect::Allow);
        assert_eq!(rules[0].object.to_string(), "tag:visibility=public");

        // The command line joins that role, so it reads them too.
        let joined: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM casbin_rule WHERE ptype = 'g' AND v0 = ? AND v1 = ?",
                params![DEFAULT_SUBJECT, EVERYONE_ROLE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(joined, 1);
    }

    #[test]
    fn the_facts_of_a_new_memory_are_owned_by_default_and_public() {
        let db = db();
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT f.owner, owner.value, visibility.value
                   FROM facts f
                   LEFT JOIN fact_tags owner
                     ON owner.fact_id = f.id AND owner.key = 'owner'
                   LEFT JOIN fact_tags visibility
                     ON visibility.fact_id = f.id AND visibility.key = 'visibility'",
            )
            .unwrap();
        let rows: Vec<(Option<String>, Option<String>, Option<String>)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();

        assert_eq!(rows.len(), MEMORY_SEED_LEN);
        for (owner, owner_tag, visibility) in rows {
            // The column is the truth, and the tag says the same, because the
            // access rules read the tag.
            assert_eq!(owner.as_deref(), Some(DEFAULT_SUBJECT));
            assert_eq!(owner_tag.as_deref(), Some(DEFAULT_SUBJECT));
            assert_eq!(visibility.as_deref(), Some("public"));
        }
    }

    #[test]
    fn the_keyword_index_follows_the_facts() {
        let db = db();
        // The test writes its own word, so that it stays correct when the
        // text of the seeded facts changes.
        let memory_id: i64 = db
            .conn()
            .query_row(
                "SELECT id FROM paths WHERE full_path = ?",
                [MEMORY_PATH],
                |row| row.get(0),
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO facts(ulid, path_id, content, created_at)
                 VALUES (?, ?, 'a fact about zarquon', '2026-01-01T00:00:00.000000Z')",
                params![Ulid::generate().to_string(), memory_id],
            )
            .unwrap();

        let hits = |db: &Database| -> i64 {
            db.conn()
                .query_row(
                    "SELECT COUNT(*) FROM facts_fts WHERE facts_fts MATCH 'zarquon'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(hits(&db), 1);

        // A delete must clear the index as well.
        db.conn()
            .execute("DELETE FROM facts WHERE content LIKE '%zarquon%'", [])
            .unwrap();
        assert_eq!(hits(&db), 0);
    }

    #[test]
    fn the_vector_index_has_the_configured_width() {
        let mut config = Config::default();
        config.embedding.dimensions = 4;
        let db = Database::open_in_memory(&config).unwrap();

        db.conn()
            .execute(
                &format!("INSERT INTO {VEC_TABLE}(fact_id, embedding) VALUES (1, ?)"),
                [bytes_of(&[0.1f32, 0.2, 0.3, 0.4])],
            )
            .unwrap();

        // A vector of the wrong width does not fit.
        let wrong = db.conn().execute(
            &format!("INSERT INTO {VEC_TABLE}(fact_id, embedding) VALUES (2, ?)"),
            [bytes_of(&[0.1f32, 0.2])],
        );
        assert!(wrong.is_err());
    }

    #[test]
    fn the_vector_index_answers_a_nearest_neighbour_query() {
        let mut config = Config::default();
        config.embedding.dimensions = 3;
        let db = Database::open_in_memory(&config).unwrap();
        for (id, v) in [(1i64, [1.0f32, 0.0, 0.0]), (2, [0.0, 1.0, 0.0])] {
            db.conn()
                .execute(
                    &format!("INSERT INTO {VEC_TABLE}(fact_id, embedding) VALUES (?, ?)"),
                    params![id, bytes_of(&v)],
                )
                .unwrap();
        }

        let nearest: i64 = db
            .conn()
            .query_row(
                &format!(
                    "SELECT fact_id FROM {VEC_TABLE}
                     WHERE embedding MATCH ? AND k = 1 ORDER BY distance"
                ),
                [bytes_of(&[0.9f32, 0.1, 0.0])],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(nearest, 1);
    }

    #[test]
    fn a_changed_width_stops_the_memory() {
        let dir = std::env::temp_dir().join("embornal-db-width");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("memory.db");
        std::fs::remove_file(&file).ok();

        let mut config = Config::default();
        config.embedding.dimensions = 8;
        Database::open(&file, &config).unwrap();

        config.embedding.dimensions = 16;
        let result = Database::open(&file, &config);
        assert!(matches!(
            result,
            Err(Error::EmbeddingDimensionsMismatch {
                stored: 8,
                configured: 16
            })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opening_twice_changes_nothing() {
        let dir = std::env::temp_dir().join("embornal-db-twice");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("memory.db");
        std::fs::remove_file(&file).ok();

        let config = Config::default();
        Database::open(&file, &config).unwrap();
        let db = Database::open(&file, &config).unwrap();

        let paths: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM paths", [], |row| row.get(0))
            .unwrap();
        let facts: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(paths, 2);
        assert_eq!(facts as usize, MEMORY_SEED_LEN);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_fact_needs_content() {
        let db = db();
        let result = db.conn().execute(
            "INSERT INTO facts(ulid, path_id, content, created_at)
             VALUES ('x', 2, '   ', '2026-01-01T00:00:00.000000Z')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_embedding_needs_the_name_of_its_model() {
        let db = db();
        let result = db
            .conn()
            .execute("UPDATE facts SET embedding = X'00000000' WHERE id = 1", []);
        assert!(result.is_err());
    }

    #[test]
    fn a_fact_cannot_hang_under_a_path_that_is_absent() {
        let db = db();
        let result = db.conn().execute(
            "INSERT INTO facts(ulid, path_id, content, created_at)
             VALUES ('x', 999, 'orphan', '2026-01-01T00:00:00.000000Z')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_tag_on_a_path_reaches_the_facts_below_it() {
        let db = db();
        db.conn()
            .execute(
                "INSERT INTO path_tags(path_id, key, value) VALUES (1, 'scope', 'root')",
                [],
            )
            .unwrap();

        let value: String = db
            .conn()
            .query_row(
                "SELECT value FROM effective_fact_tags WHERE fact_id = 1 AND key = 'scope'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "root");
    }

    #[test]
    fn the_nearest_path_decides_the_value_of_a_tag() {
        let db = db();
        let memory_id: i64 = db
            .conn()
            .query_row(
                "SELECT id FROM paths WHERE full_path = ?",
                [MEMORY_PATH],
                |r| r.get(0),
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO path_tags(path_id, key, value) VALUES (1, 'scope', 'root')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO path_tags(path_id, key, value) VALUES (?, 'scope', 'nearest')",
                [memory_id],
            )
            .unwrap();

        let value: String = db
            .conn()
            .query_row(
                "SELECT value FROM effective_fact_tags WHERE fact_id = 1 AND key = 'scope'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "nearest");
    }

    #[test]
    fn a_tag_on_a_fact_beats_the_tag_that_it_inherits() {
        let db = db();
        db.conn()
            .execute(
                "INSERT INTO path_tags(path_id, key, value) VALUES (1, 'scope', 'inherited')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO fact_tags(fact_id, key, value) VALUES (1, 'scope', 'fact')",
                [],
            )
            .unwrap();

        let mut stmt = db
            .conn()
            .prepare("SELECT value FROM effective_fact_tags WHERE fact_id = 1 AND key = 'scope'")
            .unwrap();
        let values: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(values, ["fact"]);
    }

    #[test]
    fn the_access_filter_runs_against_the_schema() {
        let db = db();
        let rules = [
            PolicyRule::from_casbin(&strings(&["cli", "path:/*", "read", "allow"])).unwrap(),
            PolicyRule::from_casbin(&strings(&["cli", "path:/memory/*", "read", "deny"])).unwrap(),
        ];
        let filter = AccessFilter::build(&rules, Action::Read);

        let sql = format!(
            "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id WHERE {}",
            filter.sql()
        );
        let params = rusqlite::params_from_iter(filter.params());
        let visible: i64 = db.conn().query_row(&sql, params, |row| row.get(0)).unwrap();
        assert_eq!(visible, 0, "the deny must hide every seeded fact");
    }

    #[test]
    fn the_access_filter_reads_the_tag_view() {
        let db = db();
        db.conn()
            .execute(
                "INSERT INTO fact_tags(fact_id, key, value) VALUES (1, 'kind', 'shared')",
                [],
            )
            .unwrap();
        let rules =
            [
                PolicyRule::from_casbin(&strings(&["cli", "tag:kind=shared", "read", "allow"]))
                    .unwrap(),
            ];
        let filter = AccessFilter::build(&rules, Action::Read);

        let sql = format!(
            "SELECT COUNT(*) FROM facts f JOIN paths p ON p.id = f.path_id WHERE {}",
            filter.sql()
        );
        let params = rusqlite::params_from_iter(filter.params());
        let visible: i64 = db.conn().query_row(&sql, params, |row| row.get(0)).unwrap();
        assert_eq!(visible, 1);
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn bytes_of(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}
