//! Tests of the wiki server. Each test starts the real binary.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// A running server with its own memory.
struct Server {
    home: PathBuf,
    child: Child,
    port: u16,
}

impl Server {
    /// Writes the facts, starts the server and waits until it answers.
    fn start(name: &str, port: u16, facts: &[(&str, &str)]) -> Self {
        let home = std::env::temp_dir().join(format!("embornal-serve-{name}"));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();

        for (path, content) in facts {
            let status = Command::new(env!("CARGO_BIN_EXE_embornal"))
                .args(["memory", "store", path, content])
                .env("EMBORNAL_HOME", &home)
                .status()
                .unwrap();
            assert!(status.success(), "the store of {path} failed");
        }

        let mut child = Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(["memory", "serve", "--port", &port.to_string()])
            .env("EMBORNAL_HOME", &home)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        // The server prints its address when it is ready.
        let stdout = child.stdout.take().unwrap();
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        assert!(line.contains(&port.to_string()), "the server said: {line}");

        Self { home, child, port }
    }

    /// Asks for one page and returns the whole answer.
    fn get(&self, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
        std::fs::remove_dir_all(&self.home).ok();
    }
}

#[test]
fn the_root_page_lists_the_tree() {
    let server = Server::start(
        "root",
        18801,
        &[("/projects/embornal", "It holds the memory.")],
    );
    let page = server.get("/");

    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("<title>/ — embornal</title>"));
    assert!(page.contains("href=\"/projects\""));
    // The path that the memory seeds is there as well.
    assert!(page.contains("href=\"/memory\""));
}

#[test]
fn a_page_shows_the_facts_of_its_path() {
    let server = Server::start("facts", 18802, &[("/notes", "The first fact.")]);
    let page = server.get("/notes");

    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("The first fact."));
    assert!(page.contains("<h1>/notes</h1>"));
}

#[test]
fn a_link_in_a_fact_becomes_a_link_in_the_page() {
    let server = Server::start(
        "links",
        18803,
        &[("/a", "see [[/projects/embornal]] for more")],
    );
    let page = server.get("/a");

    assert!(page.contains("<a href=\"/projects/embornal\">/projects/embornal</a>"));
    assert!(!page.contains("[[/projects/embornal]]"));
}

#[test]
fn the_page_carries_a_trail_up_to_the_root() {
    let server = Server::start("trail", 18804, &[("/a/b/c", "deep")]);
    let page = server.get("/a/b/c");

    assert!(page.contains("<a href=\"/\">root</a>"));
    assert!(page.contains("<a href=\"/a\">a</a>"));
    assert!(page.contains("<a href=\"/a/b\">b</a>"));
    assert!(page.contains("<strong>c</strong>"));
}

#[test]
fn the_search_page_finds_a_fact() {
    let server = Server::start(
        "search",
        18805,
        &[
            ("/db", "The memory uses SQLite."),
            ("/lang", "The tool is written in Rust."),
        ],
    );
    let page = server.get("/search?q=sqlite");

    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("The memory uses SQLite."));
    assert!(!page.contains("The tool is written in Rust."));
}

#[test]
fn a_path_that_holds_nothing_answers_404() {
    let server = Server::start("missing", 18806, &[("/a", "one")]);
    let page = server.get("/nowhere");
    assert!(page.starts_with("HTTP/1.1 404"), "{}", &page[..40]);
}

#[test]
fn a_path_that_breaks_the_rules_answers_400() {
    let server = Server::start("bad-path", 18807, &[("/a", "one")]);
    let page = server.get("/with%20space");
    assert!(page.starts_with("HTTP/1.1 400"), "{}", &page[..40]);
}

#[test]
fn a_fact_cannot_carry_markup_into_the_page() {
    let server = Server::start(
        "escaping",
        18808,
        &[("/danger", "<script>alert(1)</script>")],
    );
    let page = server.get("/danger");

    assert!(!page.contains("<script>alert(1)</script>"));
    assert!(page.contains("&lt;script&gt;"));
}

#[test]
fn the_page_does_not_count_as_a_recall() {
    let server = Server::start("no-recall", 18809, &[("/notes", "one fact")]);
    server.get("/notes");
    server.get("/notes");

    let conn = rusqlite::Connection::open(server.home.join("memory.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT recall_count FROM facts WHERE content = 'one fact'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_search_counts_as_a_recall() {
    let server = Server::start("search-recall", 18810, &[("/notes", "a fact about rust")]);
    server.get("/search?q=rust");

    let conn = rusqlite::Connection::open(server.home.join("memory.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT recall_count FROM facts WHERE content = 'a fact about rust'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
