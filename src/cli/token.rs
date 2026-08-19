//! The `embornal token` commands.
//!
//! A token lets a client reach a server. These commands run on the machine
//! that holds the memory, against the file itself: the first token cannot
//! come through the server, because the server needs a token to answer.

use crate::cli::table::{Align, Table};
use crate::cli::write_error;
use crate::error::{Error, Result};
use crate::memory::acl::{Action, EVERYONE_ROLE, OWNER_KEY, Subject};
use crate::memory::api::Memory;
use crate::memory::token::{self, Token};
use chrono::{DateTime, Duration, Utc};
use clap::{Args, Subcommand};
use rusqlite::{Connection, params};
use std::io::Write;

#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Writes a token for a subject and shows it one time.
    Add(AddArgs),
    /// Lists the tokens. It shows no secret, because it holds none.
    Ls(LsArgs),
    /// Stops a token.
    Revoke(RevokeArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The subject that the token speaks for, such as alice.
    pub subject: String,
    /// What the token is for, such as "the laptop".
    #[arg(long, default_value = "")]
    pub name: String,
    /// How many days the token works. With no value it does not run out.
    #[arg(long = "expires-in", value_name = "DAYS")]
    pub expires_in: Option<i64>,
    /// Writes no access rules. Use this for a subject that has its own.
    #[arg(long = "no-rules")]
    pub no_rules: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// Shows the tokens that no longer work as well.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct RevokeArgs {
    /// The name of the token, which `token ls` shows.
    pub ulid: String,
}

/// Runs one token command.
pub fn run(command: TokenCommand, memory: Memory, out: &mut impl Write) -> Result<()> {
    match command {
        TokenCommand::Add(args) => add(args, memory, out),
        TokenCommand::Ls(args) => ls(args, memory, out),
        TokenCommand::Revoke(args) => revoke(args, memory, out),
    }
}

/// `embornal token add [SUBJECT]`
pub fn add(args: AddArgs, memory: Memory, out: &mut impl Write) -> Result<()> {
    let subject = Subject::parse(&args.subject)?;
    let expires_at = match args.expires_in {
        Some(days) if days <= 0 => {
            return Err(Error::BadArgument(
                "--expires-in needs a number of days above zero".to_string(),
            ));
        }
        Some(days) => Some(Utc::now() + Duration::days(days)),
        None => None,
    };

    let conn = memory.database().conn();
    if !args.no_rules {
        enrol(conn, &subject)?;
    }
    let made = token::create(conn, &subject, &args.name, expires_at)?;

    writeln!(out, "{}", made.secret).map_err(write_error)?;
    writeln!(out).map_err(write_error)?;
    writeln!(
        out,
        "This is the one time that the token is readable. The memory keeps its\n\
         hash, not the token, so nothing can show it again.",
    )
    .map_err(write_error)?;
    writeln!(out).map_err(write_error)?;
    writeln!(out, "Put it in the config.yaml of the client:").map_err(write_error)?;
    writeln!(out).map_err(write_error)?;
    writeln!(out, "  server:").map_err(write_error)?;
    writeln!(out, "    url: http://the-server:1338").map_err(write_error)?;
    writeln!(out, "    token: {}", made.secret).map_err(write_error)?;
    Ok(())
}

/// Gives a new subject the rules that it needs: it writes anywhere, it reads
/// what it wrote, and it reads the facts that the memory holds about itself.
///
/// A memory that wants another shape says `--no-rules` and writes its own.
fn enrol(conn: &Connection, subject: &Subject) -> Result<()> {
    let owner = format!("tag:{OWNER_KEY}={subject}");
    for action in Action::ALL {
        conn.execute(
            "INSERT OR IGNORE INTO casbin_rule(ptype, v0, v1, v2, v3)
             VALUES ('p', ?, ?, ?, 'allow')",
            params![subject.as_str(), owner, action.as_str()],
        )?;
    }
    // A subject that cannot make a path cannot write its first fact.
    conn.execute(
        "INSERT OR IGNORE INTO casbin_rule(ptype, v0, v1, v2, v3)
         VALUES ('p', ?, 'path:/*', 'write', 'allow')",
        [subject.as_str()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO casbin_rule(ptype, v0, v1) VALUES ('g', ?, ?)",
        params![subject.as_str(), EVERYONE_ROLE],
    )?;
    Ok(())
}

/// `embornal token ls`
pub fn ls(args: LsArgs, memory: Memory, out: &mut impl Write) -> Result<()> {
    let now = Utc::now();
    let tokens = token::list(memory.database().conn())?;
    let shown: Vec<&Token> = tokens
        .iter()
        .filter(|token| args.all || token.is_live(now))
        .collect();

    print_tokens(&shown, now, out)
}

/// `embornal token revoke [ULID]`
pub fn revoke(args: RevokeArgs, memory: Memory, out: &mut impl Write) -> Result<()> {
    let token = token::revoke(memory.database().conn(), &args.ulid)?;
    writeln!(out, "{} {} stopped", token.ulid, token.subject).map_err(write_error)
}

/// Writes the table of tokens.
fn print_tokens(tokens: &[&Token], now: DateTime<Utc>, out: &mut impl Write) -> Result<()> {
    let mut table = Table::new(&[
        ("Token", Align::Left),
        ("Subject", Align::Left),
        ("Name", Align::Left),
        ("Created", Align::Left),
        ("Last used", Align::Left),
        ("State", Align::Left),
    ]);

    for token in tokens {
        table.row([
            token.ulid.to_string(),
            token.subject.to_string(),
            token.name.clone(),
            day(Some(token.created_at)),
            day(token.last_used_at),
            state(token, now),
        ]);
    }
    table.render(out)
}

/// Writes the day of a moment, or a dash when there is none.
fn day(value: Option<DateTime<Utc>>) -> String {
    match value {
        Some(moment) => moment.format("%Y-%m-%d").to_string(),
        None => "-".to_string(),
    }
}

/// Says what a token is at `now`.
fn state(token: &Token, now: DateTime<Utc>) -> String {
    match token.refusal(now) {
        Some(reason) => reason.to_string(),
        None => match token.expires_at {
            Some(end) => format!("until {}", end.format("%Y-%m-%d")),
            None => "live".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use ulid::Ulid;

    fn memory() -> Memory {
        Memory::open_in_memory(Config::default()).unwrap()
    }

    /// Runs a command and gives back what it wrote.
    fn text(f: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn token_of(subject: &str, memory: Memory) -> String {
        text(|out| {
            add(
                AddArgs {
                    subject: subject.to_string(),
                    name: "laptop".to_string(),
                    expires_in: None,
                    no_rules: false,
                },
                memory,
                out,
            )
        })
    }

    #[test]
    fn a_new_token_reaches_the_reader_one_time() {
        let memory = memory();
        let conn_check = {
            let output = token_of("alice", memory);
            assert!(output.contains("emb_"), "{output}");
            // The reader learns that the tool cannot show it again.
            assert!(output.contains("one time"), "{output}");
            // The output says where the token goes.
            assert!(output.contains("server:"), "{output}");
            assert!(output.contains("token:"), "{output}");
            output
        };
        assert!(conn_check.lines().next().unwrap().starts_with("emb_"));
    }

    #[test]
    fn a_new_subject_reads_its_own_facts_and_the_facts_of_the_memory() {
        let memory = memory();
        let conn = memory.database().conn();
        enrol(conn, &Subject::parse("alice").unwrap()).unwrap();

        let objects: Vec<String> = conn
            .prepare("SELECT v1 FROM casbin_rule WHERE ptype = 'p' AND v0 = 'alice' ORDER BY v1")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            objects,
            [
                "path:/*",
                "tag:owner=alice",
                "tag:owner=alice",
                "tag:owner=alice"
            ]
        );

        // It joins the role that reads the facts about the memory itself.
        let joined: i64 = conn
            .query_row(
                "SELECT count(*) FROM casbin_rule WHERE ptype = 'g' AND v0 = 'alice' AND v1 = ?",
                [EVERYONE_ROLE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(joined, 1);
    }

    #[test]
    fn enrolling_a_subject_twice_writes_one_set_of_rules() {
        let memory = memory();
        let conn = memory.database().conn();
        let alice = Subject::parse("alice").unwrap();
        enrol(conn, &alice).unwrap();
        enrol(conn, &alice).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM casbin_rule WHERE v0 = 'alice'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn a_name_that_cannot_be_a_subject_writes_no_token() {
        let memory = memory();
        let refused = add(
            AddArgs {
                subject: "alice bob".to_string(),
                name: String::new(),
                expires_in: None,
                no_rules: false,
            },
            memory,
            &mut Vec::new(),
        );
        assert!(refused.is_err());
    }

    #[test]
    fn a_life_of_no_days_is_refused() {
        for days in [0, -1] {
            let refused = add(
                AddArgs {
                    subject: "alice".to_string(),
                    name: String::new(),
                    expires_in: Some(days),
                    no_rules: false,
                },
                memory(),
                &mut Vec::new(),
            );
            assert!(matches!(refused, Err(Error::BadArgument(_))), "{days}");
        }
    }

    #[test]
    fn the_list_shows_the_token_and_never_the_secret() {
        let memory = memory();
        let conn = memory.database().conn();
        let made = token::create(conn, &Subject::parse("alice").unwrap(), "laptop", None).unwrap();

        let output = text(|out| ls(LsArgs { all: false }, memory, out));
        assert!(output.contains(&made.token.ulid.to_string()), "{output}");
        assert!(output.contains("alice"), "{output}");
        assert!(output.contains("laptop"), "{output}");
        assert!(output.contains("live"), "{output}");
        assert!(!output.contains(&made.secret), "{output}");
    }

    #[test]
    fn the_list_hides_a_token_that_no_longer_works() {
        let memory = memory();
        let conn = memory.database().conn();
        let made = token::create(conn, &Subject::parse("alice").unwrap(), "laptop", None).unwrap();
        let ulid = made.token.ulid.to_string();
        token::revoke(conn, &ulid).unwrap();

        let hidden = text(|out| ls(LsArgs { all: false }, memory, out));
        assert!(!hidden.contains(&ulid), "{hidden}");

        let memory = Memory::open_in_memory(Config::default()).unwrap();
        let conn = memory.database().conn();
        let made = token::create(conn, &Subject::parse("alice").unwrap(), "laptop", None).unwrap();
        let ulid = made.token.ulid.to_string();
        token::revoke(conn, &ulid).unwrap();
        let shown = text(|out| ls(LsArgs { all: true }, memory, out));
        assert!(shown.contains(&ulid), "{shown}");
        assert!(shown.contains("revoked"), "{shown}");
    }

    #[test]
    fn stopping_a_token_says_which_one_stopped() {
        let memory = memory();
        let made =
            token::create(memory.database().conn(), &Subject::cli(), "laptop", None).unwrap();
        let ulid = made.token.ulid.to_string();

        let output = text(|out| revoke(RevokeArgs { ulid: ulid.clone() }, memory, out));
        assert!(output.contains(&ulid), "{output}");
        assert!(output.contains("stopped"), "{output}");
    }

    #[test]
    fn stopping_a_token_that_is_not_there_says_so() {
        let refused = revoke(
            RevokeArgs {
                ulid: Ulid::generate().to_string(),
            },
            memory(),
            &mut Vec::new(),
        );
        assert!(matches!(refused, Err(Error::NoSuchToken(_))), "{refused:?}");
    }

    #[test]
    fn a_token_with_an_end_shows_that_end() {
        let now = Utc::now();
        let token = Token {
            ulid: Ulid::generate(),
            subject: Subject::cli(),
            name: String::new(),
            created_at: now,
            expires_at: Some(now + Duration::days(30)),
            last_used_at: None,
            revoked_at: None,
        };
        assert!(state(&token, now).starts_with("until "));
    }
}
