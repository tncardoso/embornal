//! How the memory writes time.
//!
//! Timestamps go into the database as RFC 3339 text in UTC, with a fixed
//! number of digits. The format has two properties that the queries need: a
//! human can read it with any SQLite tool, and a text sort gives the same
//! order as a time sort.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};

/// Writes a moment in the form that the database holds.
pub fn to_sql(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Reads a moment back.
pub fn from_sql(text: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    Ok(DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc))
}

/// A moment as the database holds it.
///
/// The type exists so that a query can read a column straight into a
/// `DateTime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SqlTime(pub DateTime<Utc>);

impl FromSql for SqlTime {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        from_sql(text)
            .map(SqlTime)
            .map_err(|err| FromSqlError::Other(Box::new(err)))
    }
}

impl rusqlite::ToSql for SqlTime {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::from(to_sql(self.0)))
    }
}

impl From<DateTime<Utc>> for SqlTime {
    fn from(value: DateTime<Utc>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn writes_and_reads_the_same_moment() {
        let now = Utc.with_ymd_and_hms(2026, 7, 28, 12, 34, 56).unwrap();
        assert_eq!(from_sql(&to_sql(now)).unwrap(), now);
    }

    #[test]
    fn always_writes_utc_with_a_z() {
        let text = to_sql(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(text, "2026-01-01T00:00:00.000000Z");
    }

    #[test]
    fn a_text_sort_is_a_time_sort() {
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut stamps: Vec<String> = [400, 1, 40, 0]
            .iter()
            .map(|d| to_sql(base + Duration::days(*d)))
            .collect();
        stamps.sort();
        assert_eq!(stamps[0], to_sql(base));
        assert_eq!(stamps[3], to_sql(base + Duration::days(400)));
    }

    #[test]
    fn sqlite_reads_the_format_as_a_date() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let stamp = to_sql(Utc.with_ymd_and_hms(2026, 7, 28, 12, 0, 0).unwrap());
        let day: String = conn
            .query_row("SELECT date(?)", [&stamp], |row| row.get(0))
            .unwrap();
        assert_eq!(day, "2026-07-28");
    }
}
