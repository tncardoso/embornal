//! End to end tests. Each test runs the real binary against its own memory.

use std::path::PathBuf;
use std::process::{Command, Output};

/// One memory in its own directory.
struct Sandbox {
    home: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let home = std::env::temp_dir().join(format!("embornal-cli-{name}"));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        Self { home }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(args)
            // These tests read the keyword index. Without this, each of them
            // would fetch 300 MB of weights.
            .env("EMBORNAL_EMBEDDING", "off")
            .env("EMBORNAL_HOME", &self.home)
            .output()
            .expect("the binary runs")
    }

    /// Runs the command and returns its output. The command must succeed.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Runs the command that must fail, and returns what it said.
    fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(!output.status.success(), "{args:?} was expected to fail");
        String::from_utf8(output.stderr).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

/// A machine that has no `EMBORNAL_HOME`, so that the binary reads the
/// directories of the system the way a real installation does.
///
/// Linux reads the XDG variables. macOS ignores them and puts the files below
/// `~/Library`, so each test asks the sandbox for the place instead of
/// spelling it out.
struct SystemSandbox {
    root: PathBuf,
}

impl SystemSandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("embornal-system-{name}"));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    /// The directory that an older build wrote to.
    fn legacy(&self) -> PathBuf {
        self.root.join(".embornal")
    }

    fn config(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.root.join("Library/Application Support/embornal")
        } else {
            self.root.join("config/embornal")
        }
    }

    fn data(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.root.join("Library/Application Support/embornal")
        } else {
            self.root.join("data/embornal")
        }
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(args)
            .env("EMBORNAL_EMBEDDING", "off")
            .env_remove("EMBORNAL_HOME")
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .output()
            .expect("the binary runs");
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for SystemSandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn a_new_memory_goes_to_the_directories_of_the_system() {
    let sandbox = SystemSandbox::new("fresh");
    sandbox.ok(&["memory", "store", "/notes", "A fact."]);

    assert!(sandbox.data().join("memory.db").exists());
    assert!(sandbox.config().exists());
    // Nothing writes the older directory again.
    assert!(!sandbox.legacy().exists());
}

#[test]
fn a_memory_of_an_older_build_moves_to_the_directories_of_the_system() {
    let sandbox = SystemSandbox::new("adopt");

    // Build a memory the way an older build did: everything in one directory.
    let legacy = sandbox.legacy();
    std::fs::create_dir_all(legacy.join("models")).unwrap();
    std::fs::write(legacy.join("config.yaml"), "recall:\n  limit: 7\n").unwrap();
    std::fs::write(legacy.join("models/weights.gguf"), b"heavy").unwrap();
    Command::new(env!("CARGO_BIN_EXE_embornal"))
        .args(["memory", "store", "/notes", "A fact from before."])
        .env("EMBORNAL_EMBEDDING", "off")
        .env("EMBORNAL_HOME", &legacy)
        .output()
        .unwrap();
    assert!(legacy.join("memory.db").exists());

    // The next command finds the same fact in the new place.
    let doc = sandbox.ok(&["memory", "cat", "/notes"]);
    assert!(doc.contains("A fact from before."), "{doc}");
    assert!(sandbox.data().join("memory.db").exists());
    assert!(sandbox.config().join("config.yaml").exists());
    assert!(!legacy.join("memory.db").exists());

    // The weights are a cache. They stay, and the older directory stays with
    // them, because throwing away a 300 MB download is not this tool's call.
    assert!(legacy.join("models/weights.gguf").exists());
}

#[test]
fn a_new_memory_builds_itself_on_the_first_command() {
    let sandbox = Sandbox::new("first-run");
    let listing = sandbox.ok(&["memory", "ls"]);

    // The memory comes with its own instructions.
    assert!(listing.contains("memory"));
    assert!(sandbox.home.join("memory.db").exists());

    let doc = sandbox.ok(&["memory", "cat", "/memory"]);
    assert!(doc.starts_with("# /memory"));
    assert!(doc.contains("path"));
}

#[test]
fn store_then_read_it_back() {
    let sandbox = Sandbox::new("round-trip");
    sandbox.ok(&[
        "memory",
        "store",
        "/projects/embornal",
        "The memory lives in SQLite.",
    ]);

    let doc = sandbox.ok(&["memory", "cat", "/projects/embornal"]);
    assert!(doc.contains("The memory lives in SQLite."));
}

#[test]
fn store_prints_the_public_identifier() {
    let sandbox = Sandbox::new("identifier");
    let output = sandbox.ok(&["memory", "store", "/a", "one"]);
    let (ulid, path) = output.trim().split_once(' ').unwrap();
    assert_eq!(ulid.len(), 26, "a ULID has 26 characters: {ulid}");
    assert_eq!(path, "/a");
}

#[test]
fn store_creates_the_paths_that_it_needs() {
    let sandbox = Sandbox::new("deep-path");
    sandbox.ok(&["memory", "store", "/a/b/c", "deep"]);

    assert_eq!(
        sandbox.ok(&["memory", "ls", "--plain", "/a"]).trim(),
        "/a/b/"
    );
    assert_eq!(
        sandbox.ok(&["memory", "ls", "--plain", "/a/b"]).trim(),
        "/a/b/c*"
    );
}

#[test]
fn store_folds_the_path_to_one_spelling() {
    let sandbox = Sandbox::new("folding");
    sandbox.ok(&["memory", "store", "/Projects/Embornal", "one"]);
    sandbox.ok(&["memory", "store", "/projects/embornal", "two"]);

    let doc = sandbox.ok(&["memory", "cat", "/PROJECTS/EMBORNAL"]);
    assert!(doc.contains("one"));
    assert!(doc.contains("two"));
    assert_eq!(
        sandbox
            .ok(&["memory", "ls", "/"])
            .matches("projects")
            .count(),
        1
    );
}

#[test]
fn ls_prints_a_table_of_whole_paths() {
    let sandbox = Sandbox::new("listing");
    sandbox.ok(&["memory", "store", "/work/acme/notes", "deep"]);
    sandbox.ok(&["memory", "store", "/work/acme", "here"]);

    let listing = sandbox.ok(&["memory", "ls", "/work"]);
    let lines: Vec<&str> = listing.lines().collect();

    assert_eq!(lines[0], "| Path       | Facts | Children |");
    assert_eq!(lines[1], "+------------+-------+----------+");
    // The path holds facts and children at the same time.
    assert_eq!(lines[2], "| /work/acme |     1 |        1 |");
}

#[test]
fn ls_lists_one_level_only() {
    let sandbox = Sandbox::new("one-level");
    sandbox.ok(&["memory", "store", "/work/acme/notes", "deep"]);
    sandbox.ok(&["memory", "store", "/home", "there"]);

    let root = sandbox.ok(&["memory", "ls", "/"]);
    assert!(root.contains("| /home "));
    assert!(root.contains("| /work "));
    // The grandchild does not show up.
    assert!(!root.contains("acme"));
}

#[test]
fn the_table_reports_the_counts() {
    let sandbox = Sandbox::new("counts");
    sandbox.ok(&["memory", "store", "/a", "one"]);
    sandbox.ok(&["memory", "store", "/a", "two"]);
    sandbox.ok(&["memory", "store", "/a/b", "three"]);

    let listing = sandbox.ok(&["memory", "ls", "/"]);
    let line = listing.lines().find(|l| l.contains("/a ")).unwrap();
    // Two facts of its own, and one child.
    assert!(line.contains("|     2 |        1 |"), "{line}");
}

#[test]
fn the_table_of_a_path_with_no_child_holds_its_heading_only() {
    let sandbox = Sandbox::new("empty-table");
    sandbox.ok(&["memory", "store", "/a", "one"]);

    let listing = sandbox.ok(&["memory", "ls", "/a"]);
    assert_eq!(
        listing,
        "| Path | Facts | Children |\n+------+-------+----------+\n"
    );
}

#[test]
fn the_plain_form_feeds_another_command() {
    let sandbox = Sandbox::new("plain-listing");
    sandbox.ok(&["memory", "store", "/work/acme", "one"]);

    let listing = sandbox.ok(&["memory", "ls", "--plain", "/work"]);
    assert_eq!(listing.trim(), "/work/acme*");

    // The path reads back without the mark.
    let path = listing.trim().trim_end_matches(['*', '/']);
    let doc = sandbox.ok(&["memory", "cat", path]);
    assert!(doc.contains("one"));
}

#[test]
fn tree_draws_the_whole_tree() {
    let sandbox = Sandbox::new("tree");
    sandbox.ok(&["memory", "store", "/projects/embornal/design", "one"]);
    sandbox.ok(&["memory", "store", "/projects/embornal", "two"]);
    sandbox.ok(&["memory", "store", "/projects/rust", "three"]);

    let tree = sandbox.ok(&["memory", "tree", "/projects"]);
    assert_eq!(
        tree,
        "/projects\n\
         ├── embornal*\n\
         │   └── design*\n\
         └── rust*\n"
    );
}

#[test]
fn tree_starts_at_the_root_by_default() {
    let sandbox = Sandbox::new("tree-root");
    sandbox.ok(&["memory", "store", "/a/b", "one"]);

    let tree = sandbox.ok(&["memory", "tree"]);
    assert!(tree.starts_with("/\n"), "{tree}");
    assert!(tree.contains("── a\n"), "{tree}");
    assert!(tree.contains("── b*\n"), "{tree}");
    // The path that the memory seeds is there as well.
    assert!(tree.contains("── memory*"), "{tree}");
}

#[test]
fn tree_with_dirs_only_keeps_the_branches() {
    let sandbox = Sandbox::new("tree-dirs");
    sandbox.ok(&["memory", "store", "/a/branch/leaf", "deep"]);
    sandbox.ok(&["memory", "store", "/a/leaf", "shallow"]);

    let full = sandbox.ok(&["memory", "tree", "/a"]);
    assert!(full.contains("leaf"));

    let dirs = sandbox.ok(&["memory", "tree", "/a", "--dirs-only"]);
    assert_eq!(dirs, "/a\n└── branch\n");
}

#[test]
fn tree_of_a_path_that_is_absent_fails() {
    let sandbox = Sandbox::new("tree-missing");
    let error = sandbox.fails(&["memory", "tree", "/nowhere"]);
    assert!(error.contains("not found"), "{error}");
}

#[test]
fn tree_hides_what_the_policy_refuses() {
    let sandbox = Sandbox::new("tree-deny");
    sandbox.ok(&["memory", "store", "/a/open", "visible"]);
    sandbox.ok(&["memory", "store", "/a/secret/deep", "hidden"]);

    let conn = rusqlite::Connection::open(sandbox.home.join("memory.db")).unwrap();
    conn.execute(
        "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
         VALUES ('p', 'cli', 'path:/a/secret/*', 'read', 'deny')",
        [],
    )
    .unwrap();
    drop(conn);

    let tree = sandbox.ok(&["memory", "tree", "/a"]);
    assert!(tree.contains("open"));
    assert!(!tree.contains("secret"), "{tree}");
}

#[test]
fn ls_of_a_path_that_is_absent_fails_with_a_clear_word() {
    let sandbox = Sandbox::new("missing-path");
    let error = sandbox.fails(&["memory", "ls", "/nowhere"]);
    assert!(error.contains("not found"), "{error}");
}

#[test]
fn cat_holds_the_order_and_the_limit() {
    let sandbox = Sandbox::new("cat-order");
    for content in ["first", "second", "third"] {
        sandbox.ok(&["memory", "store", "/notes", content]);
    }

    let doc = sandbox.ok(&["memory", "cat", "/notes"]);
    let lines: Vec<&str> = doc.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(lines, ["- first", "- second", "- third"]);

    let short = sandbox.ok(&["memory", "cat", "/notes", "--limit", "2"]);
    assert_eq!(short.lines().filter(|l| l.starts_with("- ")).count(), 2);
}

#[test]
fn cat_sorts_by_signal_when_it_is_asked_to() {
    let sandbox = Sandbox::new("cat-signal");
    sandbox.ok(&["memory", "store", "/notes", "the quiet fact"]);
    sandbox.ok(&["memory", "store", "/notes", "the recalled fact"]);
    // A recall lifts the second fact.
    sandbox.ok(&["memory", "recall", "recalled"]);

    let doc = sandbox.ok(&["memory", "cat", "/notes", "--order-by", "signal"]);
    let first = doc.lines().find(|l| l.starts_with("- ")).unwrap();
    assert_eq!(first, "- the recalled fact");
}

#[test]
fn cat_refuses_an_order_that_it_does_not_know() {
    let sandbox = Sandbox::new("cat-bad-order");
    sandbox.ok(&["memory", "store", "/a", "one"]);
    let error = sandbox.fails(&["memory", "cat", "/a", "--order-by", "ease"]);
    assert!(error.contains("date"), "{error}");
}

#[test]
fn recall_finds_a_fact_by_its_words() {
    let sandbox = Sandbox::new("recall-words");
    sandbox.ok(&["memory", "store", "/db", "The memory uses SQLite."]);
    sandbox.ok(&["memory", "store", "/lang", "The tool is written in Rust."]);

    let hits = sandbox.ok(&["memory", "recall", "sqlite"]);
    assert!(hits.contains("/db"));
    assert!(!hits.contains("/lang"));
}

#[test]
fn recall_prints_the_path_the_signal_and_the_fact() {
    let sandbox = Sandbox::new("recall-table");
    sandbox.ok(&["memory", "store", "/db", "The memory uses SQLite."]);

    let hits = sandbox.ok(&["memory", "recall", "sqlite"]);
    let lines: Vec<&str> = hits.lines().collect();

    assert_eq!(lines[0], "| Path | Signal | Fact                    |");
    assert_eq!(lines[1], "+------+--------+-------------------------+");
    // The fact was written a moment ago, so it is at full strength.
    assert_eq!(lines[2], "| /db  |  1.000 | The memory uses SQLite. |");
}

#[test]
fn the_score_column_comes_only_when_it_is_asked_for() {
    let sandbox = Sandbox::new("recall-scores");
    sandbox.ok(&["memory", "store", "/db", "one fact about sqlite"]);

    assert!(
        !sandbox
            .ok(&["memory", "recall", "sqlite"])
            .contains("Score")
    );
    let with_scores = sandbox.ok(&["memory", "recall", "sqlite", "--scores"]);
    assert!(
        with_scores.contains("| Path | Signal | Score |"),
        "{with_scores}"
    );
}

#[test]
fn the_signal_column_falls_for_an_old_fact() {
    let sandbox = Sandbox::new("recall-signal-column");
    sandbox.ok(&["memory", "store", "/old", "a fact about sqlite"]);

    // Push the fact far into the past, without touching its stability.
    let conn = rusqlite::Connection::open(sandbox.home.join("memory.db")).unwrap();
    conn.execute(
        "UPDATE facts SET created_at = '2020-01-01T00:00:00.000000Z' WHERE content LIKE '%sqlite%'",
        [],
    )
    .unwrap();
    drop(conn);

    let hits = sandbox.ok(&["memory", "recall", "sqlite"]);
    let row = hits.lines().find(|l| l.contains("/old")).unwrap();
    assert!(row.contains("|  0.000 |"), "{row}");
}

#[test]
fn the_plain_form_of_a_recall_feeds_a_pipe() {
    let sandbox = Sandbox::new("recall-plain");
    sandbox.ok(&["memory", "store", "/db", "The memory uses SQLite."]);

    let hits = sandbox.ok(&["memory", "recall", "sqlite", "--plain"]);
    assert_eq!(hits, "/db\tThe memory uses SQLite.\n");
}

#[test]
fn recall_with_no_words_gives_the_strong_facts() {
    let sandbox = Sandbox::new("recall-empty");
    sandbox.ok(&["memory", "store", "/a", "one fact"]);

    let hits = sandbox.ok(&["memory", "recall"]);
    assert!(!hits.trim().is_empty());
    assert!(hits.contains("one fact"));
}

#[test]
fn recall_stays_under_the_path_that_it_is_given() {
    let sandbox = Sandbox::new("recall-under");
    sandbox.ok(&["memory", "store", "/work/notes", "the shared word"]);
    sandbox.ok(&["memory", "store", "/home/notes", "the shared word"]);

    let hits = sandbox.ok(&["memory", "recall", "shared", "--under", "/work"]);
    assert!(hits.contains("/work/notes"));
    assert!(!hits.contains("/home/notes"));
}

#[test]
fn recall_lifts_the_signal_of_what_it_returns() {
    let sandbox = Sandbox::new("recall-signal");
    sandbox.ok(&["memory", "store", "/a", "a fact about rust"]);

    let before = sandbox.ok(&["memory", "recall", "rust", "--scores"]);
    let after = sandbox.ok(&["memory", "recall", "rust", "--scores"]);
    // The scores are printed, so the two runs are comparable.
    assert!(before.contains("rust") && after.contains("rust"));
}

#[test]
fn reindex_says_that_the_memory_has_no_model() {
    let sandbox = Sandbox::new("reindex-off");
    sandbox.ok(&["memory", "store", "/db", "a fact about sqlite"]);

    // The sandbox turns the model off, so nothing can fill the queue.
    let report = sandbox.ok(&["memory", "reindex"]);
    assert!(report.contains("no embedding model"), "{report}");
}

#[test]
fn a_stored_fact_has_no_vector_when_the_model_is_off() {
    let sandbox = Sandbox::new("store-no-vector");
    sandbox.ok(&["memory", "store", "/db", "a fact about sqlite"]);

    let conn = rusqlite::Connection::open(sandbox.home.join("memory.db")).unwrap();
    let waiting: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE embedding IS NULL AND deleted_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(waiting > 0, "each fact must wait for a vector");
}

/// The whole path, with the real weights.
///
/// This test fetches 300 MB the first time that it runs, so it waits for
/// `cargo test -- --ignored`.
#[test]
#[ignore = "it needs the weights of the model"]
fn the_real_model_finds_a_fact_by_its_sense() {
    let home = std::env::temp_dir().join("embornal-cli-real-model");
    std::fs::remove_dir_all(&home).ok();
    std::fs::create_dir_all(&home).unwrap();

    // The weights are large, so every run of this test shares one copy.
    let models = std::env::temp_dir().join("embornal-models");
    std::fs::create_dir_all(&models).unwrap();
    std::os::unix::fs::symlink(&models, home.join("models")).ok();

    let run = |args: &[&str]| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_embornal"))
            .args(args)
            .env("EMBORNAL_HOME", &home)
            .output()
            .expect("the binary runs");
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    run(&[
        "memory",
        "store",
        "/db",
        "The memory keeps everything in one file.",
    ]);

    // The question shares no word with the fact, so only the vector index can
    // answer it.
    let hits = run(&["memory", "recall", "where do my notes live"]);
    assert!(
        hits.contains("one file"),
        "the vector index found nothing: {hits}"
    );

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_tag_travels_from_the_command_line_to_the_policy() {
    let sandbox = Sandbox::new("tags");
    sandbox.ok(&[
        "memory",
        "store",
        "/work",
        "a private note",
        "--tag",
        "visibility=private",
    ]);
    let doc = sandbox.ok(&["memory", "cat", "/work"]);
    assert!(doc.contains("a private note"));
}

#[test]
fn cat_and_recall_show_fact_metadata_when_asked() {
    let sandbox = Sandbox::new("read-meta");
    sandbox.ok(&[
        "memory",
        "store",
        "/notes",
        "a tagged note",
        "--tag",
        "kind=note",
    ]);

    let document = sandbox.ok(&["memory", "cat", "/notes", "--meta"]);
    assert!(document.contains("Owner: cli"), "{document}");
    assert!(document.contains("Tags: kind=note owner=cli"), "{document}");

    let recalled = sandbox.ok(&["memory", "recall", "tagged", "--meta"]);
    assert!(recalled.contains("Owner"), "{recalled}");
    assert!(recalled.contains("kind=note owner=cli"), "{recalled}");

    let plain = sandbox.ok(&["memory", "recall", "tagged", "--plain", "--meta"]);
    assert_eq!(plain, "/notes\tcli\tkind=note owner=cli\ta tagged note\n");
}

#[test]
fn a_bad_tag_stops_the_store() {
    let sandbox = Sandbox::new("bad-tag");
    let error = sandbox.fails(&["memory", "store", "/a", "one", "--tag", "novalue"]);
    assert!(error.contains("key=value"), "{error}");
}

#[test]
fn a_path_that_breaks_the_rules_stops_the_store() {
    let sandbox = Sandbox::new("bad-path");
    for bad in ["relative", "/with space", "/a/../b"] {
        let error = sandbox.fails(&["memory", "store", bad, "one"]);
        assert!(error.contains("path"), "{bad}: {error}");
    }
}

#[test]
fn the_root_holds_no_facts() {
    let sandbox = Sandbox::new("root-store");
    let error = sandbox.fails(&["memory", "store", "/", "nowhere"]);
    assert!(error.contains("root"), "{error}");
}

#[test]
fn a_deny_policy_hides_the_facts_from_every_command() {
    let sandbox = Sandbox::new("deny");
    sandbox.ok(&["memory", "store", "/open", "a fact about rust"]);
    sandbox.ok(&["memory", "store", "/secret", "a fact about rust"]);

    // Write the deny straight into the policy table.
    let db = sandbox.home.join("memory.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO casbin_rule(ptype, v0, v1, v2, v3)
         VALUES ('p', 'cli', 'path:/secret/*', 'read', 'deny')",
        [],
    )
    .unwrap();
    drop(conn);

    assert!(
        !sandbox
            .ok(&["memory", "recall", "rust"])
            .contains("/secret")
    );
    assert!(!sandbox.ok(&["memory", "ls", "/"]).contains("secret"));
    let doc = sandbox.ok(&["memory", "cat", "/secret"]);
    assert!(!doc.contains("a fact about rust"), "{doc}");
}

#[test]
fn a_subject_with_no_policy_sees_nothing() {
    let sandbox = Sandbox::new("other-subject");
    sandbox.ok(&["memory", "store", "/a", "a fact about rust"]);

    // The table comes back with its heading and no row.
    let hits = sandbox.ok(&["--as-subject", "stranger", "memory", "recall", "rust"]);
    assert!(!hits.contains("a fact about rust"), "{hits}");
    assert_eq!(hits.lines().count(), 2, "{hits}");

    // In a pipe the answer is empty.
    let plain = sandbox.ok(&[
        "--as-subject",
        "stranger",
        "memory",
        "recall",
        "rust",
        "--plain",
    ]);
    assert_eq!(plain, "");

    let error = sandbox.fails(&["--as-subject", "stranger", "memory", "store", "/a", "two"]);
    assert!(error.contains("stranger"), "{error}");
}

#[test]
fn a_link_survives_the_round_trip() {
    let sandbox = Sandbox::new("links");
    sandbox.ok(&[
        "memory",
        "store",
        "/a",
        "see [[/projects/embornal]] for more",
    ]);
    let doc = sandbox.ok(&["memory", "cat", "/a"]);
    assert!(doc.contains("[[/projects/embornal]]"));
}

#[test]
fn a_subject_reads_its_own_facts_and_not_the_facts_of_another() {
    let sandbox = Sandbox::new("owners");
    // A token gives each subject the rules of a new user.
    sandbox.ok(&["token", "add", "alice"]);
    sandbox.ok(&["token", "add", "bob"]);

    sandbox.ok(&[
        "--as-subject",
        "alice",
        "memory",
        "store",
        "/notes",
        "alice wrote this",
    ]);
    sandbox.ok(&[
        "--as-subject",
        "bob",
        "memory",
        "store",
        "/notes",
        "bob wrote this",
    ]);

    let alice = sandbox.ok(&["--as-subject", "alice", "memory", "cat", "/notes"]);
    assert!(alice.contains("alice wrote this"), "{alice}");
    assert!(!alice.contains("bob wrote this"), "{alice}");

    let bob = sandbox.ok(&["--as-subject", "bob", "memory", "recall", "wrote"]);
    assert!(bob.contains("bob wrote this"), "{bob}");
    assert!(!bob.contains("alice wrote this"), "{bob}");

    // Each of them still reads the facts that the memory holds about itself.
    let instructions = sandbox.ok(&["--as-subject", "alice", "memory", "cat", "/memory"]);
    assert!(
        instructions.contains("A path names one topic"),
        "{instructions}"
    );

    // The subject of this machine keeps the whole memory, so a memory that
    // one person uses works exactly as it did.
    let all = sandbox.ok(&["memory", "cat", "/notes"]);
    assert!(all.contains("alice wrote this"), "{all}");
    assert!(all.contains("bob wrote this"), "{all}");
}

#[test]
fn nobody_but_the_memory_names_the_writer_of_a_fact() {
    let sandbox = Sandbox::new("forged-owner");
    let said = sandbox.fails(&[
        "memory",
        "store",
        "/notes",
        "not mine",
        "--tag",
        "owner=somebody-else",
    ]);
    assert!(said.contains("owner"), "{said}");

    // The fact did not reach the memory either.
    let listing = sandbox.ok(&["memory", "ls"]);
    assert!(!listing.contains("notes"), "{listing}");
}

#[test]
fn a_token_reaches_the_reader_one_time_and_the_memory_keeps_no_secret() {
    let sandbox = Sandbox::new("tokens");
    let made = sandbox.ok(&["token", "add", "alice", "--name", "laptop"]);
    let secret = made.lines().next().unwrap().to_string();
    assert!(secret.starts_with("emb_"), "{made}");

    let listed = sandbox.ok(&["token", "ls"]);
    assert!(listed.contains("alice"), "{listed}");
    assert!(listed.contains("laptop"), "{listed}");
    assert!(listed.contains("live"), "{listed}");
    // Nothing shows the secret again, because nothing holds it.
    assert!(!listed.contains(&secret), "{listed}");

    let conn = rusqlite::Connection::open(sandbox.home.join("memory.db")).unwrap();
    let hash: String = conn
        .query_row("SELECT hash FROM tokens", [], |row| row.get(0))
        .unwrap();
    assert_eq!(hash.len(), 64);
    assert!(!hash.contains(&secret));

    // The public name of the token travels with the secret, so that a secret
    // in a log says which token to stop.
    let ulid: String = conn
        .query_row("SELECT ulid FROM tokens", [], |row| row.get(0))
        .unwrap();
    assert!(secret.contains(&ulid), "{secret}");

    let stopped = sandbox.ok(&["token", "revoke", &ulid]);
    assert!(stopped.contains("stopped"), "{stopped}");
    assert!(!sandbox.ok(&["token", "ls"]).contains(&ulid));
    assert!(sandbox.ok(&["token", "ls", "--all"]).contains("revoked"));
}

#[test]
fn a_name_that_cannot_be_an_access_tag_is_not_a_subject() {
    let sandbox = Sandbox::new("bad-subject");
    // The name becomes the value of the owner tag, so it must hold no space.
    let said = sandbox.fails(&["--as-subject", "alice bob", "memory", "ls"]);
    assert!(said.contains("subject"), "{said}");
}

#[test]
fn the_help_names_each_command() {
    let sandbox = Sandbox::new("help");
    let help = sandbox.ok(&["memory", "--help"]);
    for command in ["store", "ls", "tree", "cat", "recall", "reindex", "wiki"] {
        assert!(help.contains(command), "{command} is missing from the help");
    }
    let top = sandbox.ok(&["--help"]);
    for command in ["memory", "token", "bootstrap"] {
        assert!(top.contains(command), "{command} is missing from the help");
    }
}

#[test]
fn the_bootstrap_reads_as_instructions() {
    let sandbox = Sandbox::new("bootstrap");
    let bootstrap = sandbox.ok(&["bootstrap"]);

    assert!(bootstrap.starts_with("## Memory\n"), "{bootstrap}");
    assert!(bootstrap.contains("the `embornal` command"));
    assert!(bootstrap.contains("embornal memory cat /memory"));
    assert!(bootstrap.contains("embornal memory recall <query>"));
    assert!(bootstrap.contains(
        "Subagents should never update memories. Leave that for the main agent"
    ));
}

#[test]
fn the_bootstrap_needs_no_memory_on_disk() {
    let sandbox = Sandbox::new("bootstrap-no-db");
    // The command runs before any other, so nothing built the memory yet.
    let bootstrap = sandbox.ok(&["bootstrap"]);

    assert!(!bootstrap.is_empty());
    assert!(
        !sandbox.home.join("memory.db").exists(),
        "the bootstrap command must not build a memory"
    );
}

#[test]
fn the_bootstrap_names_only_commands_that_exist() {
    let sandbox = Sandbox::new("bootstrap-commands");
    let bootstrap = sandbox.ok(&["bootstrap"]);

    // Each `embornal memory X` in the text must be a real command.
    for line in bootstrap.lines() {
        for start in line.match_indices("embornal memory ").map(|(i, _)| i) {
            let rest = &line[start + "embornal memory ".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase())
                .collect();
            assert!(
                ["store", "ls", "tree", "cat", "recall", "reindex", "serve"]
                    .contains(&name.as_str()),
                "the bootstrap names '{name}', which is not a command"
            );
        }
    }
}
