//! Tests of the server and of the client that talks to it.
//!
//! The point of these tests is that the same commands give the same answers
//! whether the memory is a file on this machine or a server on another one.
//! The battery in [`the_same_commands_answer_the_same_either_way`] runs twice
//! for exactly that reason.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

/// A memory on a machine, whether it holds the file or talks to a server.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("embornal-server-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// Points this memory at a server, which makes it a client.
    fn points_at(&self, port: u16, token: &str) {
        std::fs::write(
            self.dir.join("config.yaml"),
            format!("server:\n  url: http://127.0.0.1:{port}\n  token: {token}\n"),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(args)
            .env("EMBORNAL_EMBEDDING", "off")
            .env("EMBORNAL_HOME", &self.dir)
            .output()
            .expect("the binary runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(!output.status.success(), "{args:?} was expected to fail");
        String::from_utf8(output.stderr).unwrap()
    }

    /// Writes a token for a subject and gives back the secret.
    fn token_for(&self, subject: &str) -> String {
        self.ok(&["token", "add", subject, "--name", "test"])
            .lines()
            .next()
            .unwrap()
            .to_string()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A running server, with the memory that it holds.
struct Server {
    home: Home,
    child: Child,
    port: u16,
}

impl Server {
    fn start(name: &str, port: u16) -> Self {
        let home = Home::new(name);
        // The memory must exist before the server opens it.
        home.ok(&["memory", "ls"]);

        let mut child = Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(["serve", "--port", &port.to_string()])
            .env("EMBORNAL_EMBEDDING", "off")
            .env("EMBORNAL_HOME", &home.dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("the binary runs");

        // The server prints its address when it is ready.
        let stdout = child.stdout.take().expect("the server writes to stdout");
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        assert!(line.contains(&port.to_string()), "the server said: {line}");

        Self { home, child, port }
    }

    /// Sends one raw request and gives back the whole answer.
    fn request(&self, request: &str) -> String {
        use std::io::Read;
        let mut stream =
            std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("the server listens");
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    /// Asks the API with the token, and gives back the whole answer.
    fn get(&self, tail: &str, token: &str) -> String {
        self.request(&format!(
            "GET /api/v1{tail} HTTP/1.1\r\nHost: localhost\r\n\
             Authorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        ))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// The commands that every memory answers, and what each of them must say.
///
/// This runs against a file and against a server. Anything that differs is a
/// place where the two drifted apart.
fn battery(memory: &Home) {
    memory.ok(&[
        "memory",
        "store",
        "/projects/embornal",
        "The memory is a wiki.",
    ]);
    memory.ok(&[
        "memory",
        "store",
        "/projects/embornal",
        "SQLite holds the facts.",
    ]);
    memory.ok(&["memory", "store", "/notes", "A note.", "--tag", "kind=note"]);

    let doc = memory.ok(&["memory", "cat", "/projects/embornal"]);
    assert!(doc.starts_with("# /projects/embornal"), "{doc}");
    assert!(doc.contains("The memory is a wiki."), "{doc}");
    assert!(doc.contains("SQLite holds the facts."), "{doc}");

    let listing = memory.ok(&["memory", "ls", "/"]);
    assert!(listing.contains("/projects"), "{listing}");
    assert!(listing.contains("/notes"), "{listing}");

    let tree = memory.ok(&["memory", "tree", "/projects"]);
    assert!(tree.contains("embornal"), "{tree}");

    let hits = memory.ok(&["memory", "recall", "sqlite"]);
    assert!(hits.contains("SQLite holds the facts."), "{hits}");

    let plain = memory.ok(&["memory", "recall", "sqlite", "--plain"]);
    assert!(plain.contains("SQLite holds the facts."), "{plain}");

    // The memory carries its own instructions either way.
    let instructions = memory.ok(&["memory", "cat", "/memory"]);
    assert!(
        instructions.contains("A path names one topic"),
        "{instructions}"
    );

    // A rule that a path breaks is the rule of the memory, wherever it runs.
    let said = memory.fails(&["memory", "store", "nope", "no leading slash"]);
    assert!(said.contains('/'), "{said}");
    let refused = memory.fails(&["memory", "store", "/notes", "mine", "--tag", "owner=bob"]);
    assert!(refused.contains("owner"), "{refused}");
}

#[test]
fn the_same_commands_answer_the_same_either_way() {
    // On this machine.
    let local = Home::new("battery-local");
    battery(&local);

    // On a server. The subject differs, so the facts are its own, but every
    // command answers the same way.
    let server = Server::start("battery-remote", 18921);
    let token = server.home.token_for("alice");
    let client = Home::new("battery-client");
    client.points_at(server.port, &token);
    battery(&client);

    // The client holds no memory of its own: it wrote nothing but its
    // configuration.
    let left: Vec<String> = std::fs::read_dir(&client.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(left, ["config.yaml"], "{left:?}");
}

#[test]
fn a_request_with_no_token_reaches_nothing() {
    let server = Server::start("no-token", 18922);

    let answer = server
        .request("GET /api/v1/whoami HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");

    // A token that names nothing opens nothing either.
    let answer = server.get("/whoami", "emb_not_a_token");
    assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");
    assert!(answer.contains("no such token"), "{answer}");
}

#[test]
fn the_token_says_who_asks_and_the_client_flag_is_ignored() {
    let server = Server::start("identity", 18923);
    let alice = server.home.token_for("alice");
    let bob = server.home.token_for("bob");

    assert!(
        server
            .get("/whoami", &alice)
            .contains("\"subject\":\"alice\"")
    );
    assert!(server.get("/whoami", &bob).contains("\"subject\":\"bob\""));

    // The local flag cannot replace the subject that the token names.
    let client = Home::new("identity-client");
    client.points_at(server.port, &alice);
    client.ok(&[
        "--as-subject",
        "bob",
        "memory",
        "store",
        "/notes",
        "written with alice's token",
    ]);
    let owned = client.ok(&["memory", "cat", "/notes", "--meta"]);
    assert!(owned.contains("Owner: alice"), "{owned}");
    assert!(owned.contains("Tags: owner=alice"), "{owned}");
}

#[test]
fn a_subject_reads_its_own_facts_through_the_server() {
    let server = Server::start("owners", 18924);
    server
        .home
        .ok(&["memory", "store", "/private", "default kept this"]);
    let alice = Home::new("owners-alice");
    alice.points_at(server.port, &server.home.token_for("alice"));
    let bob = Home::new("owners-bob");
    bob.points_at(server.port, &server.home.token_for("bob"));

    alice.ok(&["memory", "store", "/notes", "alice wrote this"]);
    bob.ok(&["memory", "store", "/notes", "bob wrote this"]);
    alice.ok(&[
        "memory",
        "store",
        "/news",
        "alice shared this",
        "--tag",
        "visibility=public",
    ]);

    let owned = alice.ok(&["memory", "cat", "/notes", "--meta"]);
    assert!(owned.contains("Owner: alice"), "{owned}");
    assert!(owned.contains("Tags: owner=alice"), "{owned}");

    let seen = alice.ok(&["memory", "cat", "/notes"]);
    assert!(seen.contains("alice wrote this"), "{seen}");
    assert!(!seen.contains("bob wrote this"), "{seen}");

    let found = bob.ok(&["memory", "recall", "wrote"]);
    assert!(found.contains("bob wrote this"), "{found}");
    assert!(!found.contains("alice wrote this"), "{found}");

    let public = bob.ok(&["memory", "cat", "/news", "--meta"]);
    assert!(public.contains("alice shared this"), "{public}");
    assert!(public.contains("visibility=public"), "{public}");

    let private = alice.ok(&["memory", "cat", "/private"]);
    assert!(!private.contains("default kept this"), "{private}");

    // Both of them read the facts that the memory holds about itself.
    for memory in [&alice, &bob] {
        let bootstrap = memory.ok(&["memory", "cat", "/memory", "--meta"]);
        assert!(bootstrap.contains("A path names one topic"), "{bootstrap}");
        assert!(bootstrap.contains("Owner: default"), "{bootstrap}");
        assert!(bootstrap.contains("visibility=public"), "{bootstrap}");
    }
}

#[test]
fn a_token_that_stopped_reaches_nothing() {
    let server = Server::start("revoked", 18925);
    let token = server.home.token_for("alice");
    let client = Home::new("revoked-client");
    client.points_at(server.port, &token);
    client.ok(&["memory", "ls"]);

    // The name of the token sits inside the token itself.
    let ulid = token.split('_').nth(1).unwrap().to_string();
    server.home.ok(&["token", "revoke", &ulid]);

    let said = client.fails(&["memory", "ls"]);
    assert!(said.contains("revoked"), "{said}");
}

#[test]
fn a_server_that_does_not_answer_stops_the_command() {
    let client = Home::new("offline");
    // Nothing listens on this port.
    client.points_at(18926, "emb_a_b");

    for args in [
        vec!["memory", "ls"],
        vec!["memory", "recall", "anything"],
        vec!["memory", "store", "/notes", "this must not land anywhere"],
    ] {
        let said = client.fails(&args);
        assert!(said.contains("does not answer"), "{args:?}: {said}");
    }

    // The command never falls back to a memory of this machine, so nothing
    // was written and nothing can drift apart.
    assert!(!client.dir.join("memory.db").exists());
}

#[test]
fn the_commands_that_need_the_file_say_where_to_run_them() {
    let client = Home::new("local-only");
    client.points_at(18927, "emb_a_b");

    for args in [
        vec!["dashboard"],
        vec!["memory", "reindex"],
        vec!["token", "ls"],
        vec!["serve"],
    ] {
        let said = client.fails(&args);
        assert!(
            said.contains("machine that holds the memory"),
            "{args:?}: {said}"
        );
    }
}

#[test]
fn a_configuration_that_names_no_token_says_so() {
    let client = Home::new("no-token-config");
    std::fs::write(
        client.dir.join("config.yaml"),
        "server:\n  url: http://127.0.0.1:18928\n",
    )
    .unwrap();

    let said = client.fails(&["memory", "ls"]);
    assert!(said.contains("token"), "{said}");
}
