//! The SQLite that the memory and the code index share.
//!
//! Both keep one file, both open it the same way, and both walk their schema
//! forward with `user_version`. What differs is the schema itself, so that
//! stays with each of them.

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::sync::Once;

static VEC_EXTENSION: Once = Once::new();

/// Registers the vector extension with SQLite.
///
/// SQLite loads the extension into each connection that opens after this
/// call, so the function runs one time for the whole process. Each database
/// that wants a vector index calls it before it opens a connection, and the
/// call after the first one does nothing.
pub fn register_vec_extension() {
    VEC_EXTENSION.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the entry point that the sqlite-vec
        // crate exports for exactly this purpose, and the registration
        // happens one time, before any connection opens.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// Puts a fresh connection in the shape that both databases want.
///
/// WAL lets a reader work while a writer writes, which matters because a
/// server and a command line can hold the same file. The timeout then covers
/// the writer that must wait for another writer.
pub fn prepare(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

/// One step of a schema.
///
/// The version is the number that the file carries after the step runs. The
/// steps of one database are given in order, and the last one names the
/// version that this build writes.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: i64,
    pub sql: &'static str,
}

/// Reads the version that the file carries.
pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Applies the steps that the file still needs.
///
/// `after` runs inside the same transaction, once for each step that ran, with
/// the version of that step. A database that must write rows of its own after
/// a step uses it; one that has nothing to add gives back `Ok(())`.
///
/// A file from a newer build stops the tool instead of being written to: this
/// build does not know what that file holds, and a guess would lose facts.
pub fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
    mut after: impl FnMut(&Connection, i64) -> Result<()>,
) -> Result<()> {
    let Some(target) = migrations.last().map(|step| step.version) else {
        return Ok(());
    };

    let current = schema_version(conn)?;
    if current > target {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: target,
        });
    }
    if current == target {
        return Ok(());
    }

    let tx = conn.transaction()?;
    for step in migrations {
        if current < step.version {
            tx.execute_batch(step.sql)?;
            after(&tx, step.version)?;
        }
    }
    tx.pragma_update(None, "user_version", target)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: [Migration; 2] = [
        Migration {
            version: 1,
            sql: "CREATE TABLE one (id INTEGER PRIMARY KEY);",
        },
        Migration {
            version: 2,
            sql: "CREATE TABLE two (id INTEGER PRIMARY KEY);",
        },
    ];

    fn open() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        prepare(&conn).unwrap();
        conn
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get(0)).unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn a_new_file_walks_every_step() {
        let mut conn = open();
        migrate(&mut conn, &STEPS, |_, _| Ok(())).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 2);
        assert_eq!(tables(&conn), vec!["one", "two"]);
    }

    #[test]
    fn a_file_that_stopped_halfway_walks_the_rest() {
        let mut conn = open();
        migrate(&mut conn, &STEPS[..1], |_, _| Ok(())).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 1);

        migrate(&mut conn, &STEPS, |_, _| Ok(())).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 2);
        assert_eq!(tables(&conn), vec!["one", "two"]);
    }

    #[test]
    fn a_second_run_applies_nothing_again() {
        let mut conn = open();
        let mut ran = Vec::new();
        migrate(&mut conn, &STEPS, |_, version| {
            ran.push(version);
            Ok(())
        })
        .unwrap();
        assert_eq!(ran, vec![1, 2]);

        // A step that ran a second time would fail on CREATE TABLE, so the
        // hook staying silent is the whole assertion.
        ran.clear();
        migrate(&mut conn, &STEPS, |_, version| {
            ran.push(version);
            Ok(())
        })
        .unwrap();
        assert!(ran.is_empty());
    }

    #[test]
    fn the_hook_runs_inside_the_transaction_of_its_step() {
        let mut conn = open();
        migrate(&mut conn, &STEPS, |tx, version| {
            if version == 1 {
                tx.execute("INSERT INTO one(id) VALUES (7)", [])?;
            }
            Ok(())
        })
        .unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM one", [], |row| row.get(0))
            .unwrap();
        assert_eq!(id, 7);
    }

    #[test]
    fn a_failed_hook_leaves_the_file_untouched() {
        let mut conn = open();
        let result = migrate(&mut conn, &STEPS, |_, _| Err(Error::EmptyContent));
        assert!(result.is_err());
        assert_eq!(schema_version(&conn).unwrap(), 0);
        assert!(tables(&conn).is_empty());
    }

    #[test]
    fn a_file_from_a_newer_build_stops_the_tool() {
        let mut conn = open();
        conn.pragma_update(None, "user_version", 9).unwrap();
        let error = migrate(&mut conn, &STEPS, |_, _| Ok(())).unwrap_err();
        assert!(
            matches!(
                error,
                Error::SchemaTooNew {
                    found: 9,
                    supported: 2
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_schema_with_no_steps_asks_for_no_work() {
        let mut conn = open();
        migrate(&mut conn, &[], |_, _| Ok(())).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 0);
    }
}
