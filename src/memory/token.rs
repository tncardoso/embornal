//! The tokens that let a client reach a server.
//!
//! A token names one subject. The client sends it with each request, and the
//! server reads the subject from it. The client cannot say who it is: the
//! token says that, and nothing else does.
//!
//! The secret itself is never written down. The table holds its SHA-256, so
//! somebody who reads the database still cannot reach the server with it.
//! The secret is 32 random bytes, so a hash with no salt and no work factor
//! is enough; a password would need more, because a person chooses a password
//! and a person chooses badly.

use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::time;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

/// The mark that says a string is a token of this tool.
pub const TOKEN_PREFIX: &str = "emb";

/// How many random bytes one secret holds.
///
/// 32 bytes is far above what a guess can reach, so the server needs no wait
/// between tries and the hash needs no work factor.
pub const SECRET_BYTES: usize = 32;

/// One token, without its secret.
///
/// The secret leaves the tool one time, at the moment that [`create`] makes
/// it. After that only this record stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The public name of the token. `token ls` shows it, and `token revoke`
    /// reads it.
    pub ulid: Ulid,
    pub subject: Subject,
    /// What the token is for, in the words of the person who made it.
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl Token {
    /// Says whether the token still opens the door at `now`.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        match self.expires_at {
            Some(end) => now < end,
            None => true,
        }
    }

    /// Says why the token does not work, or nothing when it does.
    pub fn refusal(&self, now: DateTime<Utc>) -> Option<&'static str> {
        if self.revoked_at.is_some() {
            Some("revoked")
        } else if self.expires_at.is_some_and(|end| now >= end) {
            Some("expired")
        } else {
            None
        }
    }
}

/// A token together with the secret that opens it.
///
/// [`create`] gives this back one time. Nothing reads the secret out of the
/// database later, because the database does not hold it.
#[derive(Debug, Clone)]
pub struct NewToken {
    pub token: Token,
    pub secret: String,
}

/// Writes a token for `subject` and gives back its secret.
///
/// The caller shows the secret to the person who asked for it, and then
/// forgets it.
pub fn create(
    conn: &Connection,
    subject: &Subject,
    name: &str,
    expires_at: Option<DateTime<Utc>>,
) -> Result<NewToken> {
    let ulid = Ulid::generate();
    let secret = mint(ulid)?;
    let now = Utc::now();

    conn.execute(
        "INSERT INTO tokens(ulid, subject, hash, name, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            ulid.to_string(),
            subject.as_str(),
            digest(&secret),
            name,
            time::to_sql(now),
            expires_at.map(time::to_sql),
        ],
    )?;

    Ok(NewToken {
        token: Token {
            ulid,
            subject: subject.clone(),
            name: name.to_string(),
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked_at: None,
        },
        secret,
    })
}

/// Builds one secret: the mark, the public name, and 32 random bytes.
///
/// The public name travels with the secret so that a person who finds a
/// secret in a log can say which token to revoke, without a lookup.
fn mint(ulid: Ulid) -> Result<String> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|err| Error::Token(format!("cannot read random bytes: {err}")))?;
    Ok(format!(
        "{TOKEN_PREFIX}_{ulid}_{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

/// Returns the SHA-256 of a secret, in the form that the table holds.
pub fn digest(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Finds the token of a secret, whether or not it still works.
///
/// The lookup goes by hash, so a secret that no token matches gives nothing
/// and no query ever holds the secret itself.
pub fn find(conn: &Connection, secret: &str) -> Result<Option<Token>> {
    let mut stmt = conn.prepare(SELECT)?;
    Ok(stmt.query_row([digest(secret)], read).optional()?)
}

/// Finds the token of a secret and marks it as used.
///
/// It gives back the subject that the token names. The caller must not read
/// the subject from anywhere else: the token is the only thing that says who
/// the caller is.
pub fn authenticate(conn: &Connection, secret: &str) -> Result<Subject> {
    let now = Utc::now();
    let Some(token) = find(conn, secret)? else {
        return Err(Error::Unauthorized("no such token".to_string()));
    };
    if let Some(reason) = token.refusal(now) {
        return Err(Error::Unauthorized(format!("the token is {reason}")));
    }

    conn.execute(
        "UPDATE tokens SET last_used_at = ? WHERE ulid = ?",
        params![time::to_sql(now), token.ulid.to_string()],
    )?;
    Ok(token.subject)
}

/// Lists every token, newest first. No secret is here to list.
pub fn list(conn: &Connection) -> Result<Vec<Token>> {
    let mut stmt = conn.prepare(&format!("{SELECT_ALL} ORDER BY created_at DESC, id DESC"))?;
    let tokens = stmt
        .query_map([], read)?
        .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
    Ok(tokens)
}

/// Stops a token. A token that is already stopped keeps the time that it had.
pub fn revoke(conn: &Connection, ulid: &str) -> Result<Token> {
    let changed = conn.execute(
        "UPDATE tokens SET revoked_at = ? WHERE ulid = ? AND revoked_at IS NULL",
        params![time::to_sql(Utc::now()), ulid],
    )?;

    let mut stmt = conn.prepare(&format!("{SELECT_ALL} WHERE ulid = ?"))?;
    let token = stmt
        .query_row([ulid], read)
        .optional()?
        .ok_or_else(|| Error::NoSuchToken(ulid.to_string()))?;

    if changed == 0 {
        return Err(Error::Token(format!(
            "the token {ulid} was already stopped"
        )));
    }
    Ok(token)
}

const SELECT_ALL: &str = "SELECT ulid, subject, name, created_at, expires_at, last_used_at, \
     revoked_at FROM tokens";

const SELECT: &str = "SELECT ulid, subject, name, created_at, expires_at, last_used_at, \
     revoked_at FROM tokens WHERE hash = ?";

/// Reads one row into a token.
fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Token> {
    let ulid: String = row.get(0)?;
    let subject: String = row.get(1)?;
    Ok(Token {
        ulid: ulid.parse().unwrap_or_else(|_| Ulid::nil()),
        subject: Subject::new(subject),
        name: row.get(2)?,
        created_at: stamp(row, 3)?.unwrap_or_else(Utc::now),
        expires_at: stamp(row, 4)?,
        last_used_at: stamp(row, 5)?,
        revoked_at: stamp(row, 6)?,
    })
}

/// Reads one time out of a column that can be empty.
fn stamp(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let text: Option<String> = row.get(index)?;
    Ok(text.as_deref().and_then(|text| time::from_sql(text).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::memory::db::Database;
    use chrono::Duration;

    fn db() -> Database {
        Database::open_in_memory(&Config::default()).unwrap()
    }

    fn subject(name: &str) -> Subject {
        Subject::parse(name).unwrap()
    }

    #[test]
    fn a_new_token_carries_its_mark_and_its_name() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();

        // The random part is base64 for a URL, which itself holds `_`, so the
        // split stops after the two fields that come before it.
        let parts: Vec<&str> = made.secret.splitn(3, '_').collect();
        assert_eq!(parts.len(), 3, "{}", made.secret);
        assert_eq!(parts[0], TOKEN_PREFIX);
        // The public name travels with the secret, so that a secret in a log
        // says which token to stop.
        assert_eq!(parts[1], made.token.ulid.to_string());
        // 32 bytes of base64, with no padding.
        assert_eq!(parts[2].len(), 43, "{}", made.secret);
    }

    #[test]
    fn two_tokens_never_share_a_secret() {
        let db = db();
        let one = create(db.conn(), &subject("alice"), "", None).unwrap();
        let two = create(db.conn(), &subject("alice"), "", None).unwrap();
        assert_ne!(one.secret, two.secret);
    }

    #[test]
    fn the_table_holds_no_secret() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();

        let (hash, count): (String, i64) = db
            .conn()
            .query_row(
                "SELECT hash, count(*) FROM tokens WHERE ulid = ?",
                [made.token.ulid.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(hash, digest(&made.secret));
        assert_eq!(hash.len(), 64);
        // Nothing of the secret itself reaches the row.
        assert!(!hash.contains(&made.secret));

        let dump: String = db
            .conn()
            .query_row(
                "SELECT group_concat(ulid || subject || hash || name) FROM tokens",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!dump.contains(&made.secret));
    }

    #[test]
    fn a_token_says_which_subject_asks() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();
        assert_eq!(
            authenticate(db.conn(), &made.secret).unwrap(),
            subject("alice")
        );
    }

    #[test]
    fn a_secret_that_no_token_matches_opens_nothing() {
        let db = db();
        create(db.conn(), &subject("alice"), "laptop", None).unwrap();

        for guess in ["", "emb_x_y", "not a token"] {
            assert!(matches!(
                authenticate(db.conn(), guess),
                Err(Error::Unauthorized(_))
            ));
        }
    }

    #[test]
    fn a_token_that_was_stopped_opens_nothing() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();
        revoke(db.conn(), &made.token.ulid.to_string()).unwrap();

        let refused = authenticate(db.conn(), &made.secret);
        assert!(
            matches!(refused, Err(Error::Unauthorized(_))),
            "{refused:?}"
        );
        assert!(refused.unwrap_err().to_string().contains("revoked"));
    }

    #[test]
    fn a_token_that_ran_out_of_time_opens_nothing() {
        let db = db();
        let past = Utc::now() - Duration::days(1);
        let made = create(db.conn(), &subject("alice"), "laptop", Some(past)).unwrap();

        let refused = authenticate(db.conn(), &made.secret);
        assert!(refused.unwrap_err().to_string().contains("expired"));

        // A token with time left still works.
        let future = Utc::now() + Duration::days(1);
        let live = create(db.conn(), &subject("alice"), "laptop", Some(future)).unwrap();
        assert!(authenticate(db.conn(), &live.secret).is_ok());
    }

    #[test]
    fn a_token_remembers_when_it_last_opened_the_door() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();
        assert!(made.token.last_used_at.is_none());

        authenticate(db.conn(), &made.secret).unwrap();
        let seen = find(db.conn(), &made.secret).unwrap().unwrap();
        assert!(seen.last_used_at.is_some());
    }

    #[test]
    fn the_list_shows_every_token_and_no_secret() {
        let db = db();
        create(db.conn(), &subject("alice"), "laptop", None).unwrap();
        let bob = create(db.conn(), &subject("bob"), "server", None).unwrap();

        let tokens = list(db.conn()).unwrap();
        assert_eq!(tokens.len(), 2);
        let subjects: Vec<String> = tokens.iter().map(|t| t.subject.to_string()).collect();
        assert!(subjects.contains(&"alice".to_string()));
        assert!(subjects.contains(&"bob".to_string()));
        // The record of a token holds no field that could carry the secret.
        assert!(!format!("{tokens:?}").contains(&bob.secret));
    }

    #[test]
    fn stopping_a_token_twice_says_so() {
        let db = db();
        let made = create(db.conn(), &subject("alice"), "laptop", None).unwrap();
        let ulid = made.token.ulid.to_string();

        assert!(revoke(db.conn(), &ulid).unwrap().revoked_at.is_some());
        assert!(revoke(db.conn(), &ulid).is_err());
        assert!(matches!(
            revoke(db.conn(), "01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            Err(Error::NoSuchToken(_))
        ));
    }

    #[test]
    fn a_live_token_is_the_one_with_no_end_and_no_stop() {
        let now = Utc::now();
        let mut token = Token {
            ulid: Ulid::generate(),
            subject: subject("alice"),
            name: String::new(),
            created_at: now,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        };
        assert!(token.is_live(now));
        assert_eq!(token.refusal(now), None);

        token.expires_at = Some(now + Duration::days(1));
        assert!(token.is_live(now));

        token.expires_at = Some(now);
        assert!(!token.is_live(now));
        assert_eq!(token.refusal(now), Some("expired"));

        // A stop beats time that is left.
        token.expires_at = Some(now + Duration::days(1));
        token.revoked_at = Some(now);
        assert!(!token.is_live(now));
        assert_eq!(token.refusal(now), Some("revoked"));
    }
}
