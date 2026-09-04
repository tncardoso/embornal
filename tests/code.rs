//! End to end tests of `embornal code`.
//!
//! Each test runs the real binary against its own index and its own
//! repository, so that nothing on the machine reaches them.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// One index and one repository, in their own directories.
struct Sandbox {
    home: PathBuf,
    repo: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("embornal-code-cli-{name}"));
        std::fs::remove_dir_all(&base).ok();
        let home = base.join("home");
        let repo = base.join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        // A `.git` makes the walk stop here, whatever sits above it.
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        Self {
            home,
            repo: std::fs::canonicalize(&repo).unwrap(),
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_embornal"));
        command
            .args(args)
            // These tests read the keyword index. Without this, each of them
            // would fetch 300 MB of weights.
            .env("EMBORNAL_EMBEDDING", "off")
            .env("EMBORNAL_HOME", &self.home)
            .current_dir(&self.repo);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("the binary runs")
    }

    /// Runs the command, which must succeed.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Runs the command that must fail, and gives back what it said.
    fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(!output.status.success(), "{args:?} was expected to fail");
        String::from_utf8(output.stderr).unwrap()
    }

    /// Sends a JSON array to `code describe`.
    fn describe(&self, body: &str) -> String {
        let mut child = self
            .command(&["code", "describe", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "describe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Describes every node of the next batch, and gives back how many.
    fn answer_next(&self) -> usize {
        self.answer(&["code", "next", "--json"])
    }

    /// The same, with the root of the repository in the queue.
    fn answer_next_with_root(&self) -> usize {
        self.answer(&["code", "next", "--json", "--update-root"])
    }

    fn answer(&self, args: &[&str]) -> usize {
        let batch: serde_json::Value = serde_json::from_str(&self.ok(args)).unwrap();
        let Some(nodes) = batch.get("nodes").and_then(|nodes| nodes.as_array()) else {
            return 0;
        };
        let written: Vec<serde_json::Value> = nodes
            .iter()
            .map(|node| {
                let name = node["name"].as_str().unwrap();
                serde_json::json!({
                    "id": node["id"],
                    "summary": format!("Handles the {name} of this file."),
                    "description": format!("The node {name} reads its input and gives back a value."),
                })
            })
            .collect();
        self.describe(&serde_json::to_string(&written).unwrap());
        nodes.len()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.home.parent().unwrap()).ok();
    }
}

#[test]
fn the_index_is_built_and_then_costs_nothing_to_keep() {
    let sandbox = Sandbox::new("incremental");
    sandbox.write("src/a.rs", "fn one() {}\nfn two() {}\n");
    sandbox.write("src/b.rs", "fn three() {}\n");

    let first = sandbox.ok(&["code", "index"]);
    assert!(first.contains("2 files, 2 parsed"), "{first}");

    // The whole point: a pass over a repository that did not change reads no
    // file at all.
    let second = sandbox.ok(&["code", "index"]);
    assert!(second.contains("2 files, 0 parsed"), "{second}");
}

#[test]
fn a_change_reaches_only_the_file_that_changed() {
    let sandbox = Sandbox::new("changed");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.write("src/b.rs", "fn two() {}\n");
    sandbox.ok(&["code", "index"]);

    sandbox.write("src/a.rs", "fn one() { work() }\n");
    let again = sandbox.ok(&["code", "index"]);
    assert!(again.contains("2 files, 1 parsed"), "{again}");
}

#[test]
fn the_status_counts_what_still_waits() {
    let sandbox = Sandbox::new("status");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);

    let status = sandbox.ok(&["code", "status"]);
    assert!(status.contains("Kind"), "{status}");
    assert!(status.contains("function"), "{status}");
}

#[test]
fn the_loop_of_an_agent_empties_the_queue() {
    let sandbox = Sandbox::new("loop");
    sandbox.write(
        "src/memory/api.rs",
        "struct M;\nimpl M {\n    fn open() {}\n}\n",
    );
    sandbox.write("src/main.rs", "fn main() {}\n");
    sandbox.ok(&["code", "index"]);

    // next -> describe, until nothing waits.
    let mut batches = 0;
    while sandbox.answer_next() > 0 {
        batches += 1;
        assert!(batches < 20, "the queue does not empty");
    }
    assert!(batches >= 4, "two files and two directories at least");

    assert!(sandbox.ok(&["code", "next"]).contains("nothing waits"));
    assert_eq!(sandbox.ok(&["code", "next", "--json"]).trim(), "null");

    // The root is what is left, and only when it is asked for.
    let root: serde_json::Value =
        serde_json::from_str(&sandbox.ok(&["code", "next", "--json", "--update-root"])).unwrap();
    assert_eq!(root["nodes"][0]["kind"], "repo");
}

#[test]
fn the_payload_names_the_file_and_carries_no_source() {
    let sandbox = Sandbox::new("payload");
    sandbox.write(
        "src/a.rs",
        "fn one() {\n    a_word_that_is_only_in_the_body();\n}\n",
    );
    sandbox.ok(&["code", "index"]);

    let text = sandbox.ok(&["code", "next", "--json"]);
    assert!(text.contains("src/a.rs"), "{text}");
    assert!(
        !text.contains("a_word_that_is_only_in_the_body"),
        "the payload must not carry source: {text}"
    );

    let batch: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(batch["kind"], "file");
    assert_eq!(batch["language"], "rust");
    assert_eq!(batch["nodes"][0]["lines"][0], 1);
}

#[test]
fn what_an_agent_writes_comes_back_from_cat_and_from_recall() {
    let sandbox = Sandbox::new("write");
    sandbox.write("src/token.rs", "fn check() {}\n");
    sandbox.ok(&["code", "index"]);

    let batch: serde_json::Value =
        serde_json::from_str(&sandbox.ok(&["code", "next", "--json"])).unwrap();
    let id = batch["nodes"][0]["id"].as_str().unwrap();
    sandbox.describe(&format!(
        r#"[{{"id": "{id}",
             "summary": "Compares a secret with the stored one.",
             "description": "Reads the secret of a request and answers whether it opens the memory."}}]"#
    ));

    let shown = sandbox.ok(&["code", "cat", "src/token.rs::check"]);
    assert!(shown.contains("Compares a secret"), "{shown}");
    assert!(shown.contains("function src/token.rs:1-1"), "{shown}");

    let found = sandbox.ok(&["code", "recall", "secret"]);
    assert!(found.contains("src/token.rs::check"), "{found}");
}

#[test]
fn a_write_against_a_node_that_moved_is_refused() {
    let sandbox = Sandbox::new("guard");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);

    let batch: serde_json::Value =
        serde_json::from_str(&sandbox.ok(&["code", "next", "--json"])).unwrap();
    let id = batch["nodes"][0]["id"].as_str().unwrap().to_string();

    // The code moves, and the id of the older node goes with it.
    sandbox.write("src/a.rs", "fn one() { other() }\n");
    sandbox.ok(&["code", "index"]);

    let mut child = sandbox
        .command(&["code", "describe", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            format!(r#"[{{"id": "{id}", "summary": "Older.", "description": "Older."}}]"#)
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("there is no node"), "{said}");
}

#[test]
fn a_second_collection_over_the_same_code_costs_nothing() {
    let sandbox = Sandbox::new("fork");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);
    // The root as well, so that the first collection owes nothing at all.
    while sandbox.answer_next_with_root() > 0 {}

    let forked = sandbox.ok(&["code", "index", "--collection", "experiment"]);
    assert!(
        forked.contains("0 stale"),
        "a fork must not pay again: {forked}"
    );
}

#[test]
fn the_tree_marks_what_still_waits() {
    let sandbox = Sandbox::new("tree");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);

    let waiting = sandbox.ok(&["code", "tree"]);
    assert!(waiting.contains("a.rs*"), "{waiting}");

    while sandbox.answer_next() > 0 {}

    let described = sandbox.ok(&["code", "tree", "src"]);
    assert!(described.contains("a.rs"), "{described}");
    assert!(!described.contains("a.rs*"), "{described}");
}

#[test]
fn a_file_that_no_grammar_can_read_becomes_one_node() {
    let sandbox = Sandbox::new("broken");
    sandbox.write(
        "src/a.rs",
        "fn a() {\n<<<<<<< HEAD\n=======\n>>>>>>> b\n}\n",
    );

    let report = sandbox.ok(&["code", "index"]);
    assert!(
        report.contains("1 files that no grammar could read"),
        "{report}"
    );

    let batch: serde_json::Value =
        serde_json::from_str(&sandbox.ok(&["code", "next", "--json"])).unwrap();
    assert_eq!(batch["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(batch["nodes"][0]["kind"], "file");
}

#[test]
fn a_command_before_the_first_index_says_what_to_run() {
    let sandbox = Sandbox::new("never");
    sandbox.write("src/a.rs", "fn one() {}\n");

    let said = sandbox.fails(&["code", "status"]);
    assert!(said.contains("embornal code index"), "{said}");
}

#[test]
fn describe_says_how_to_call_it_when_it_is_given_half_of_what_it_needs() {
    let sandbox = Sandbox::new("halves");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);

    let said = sandbox.fails(&["code", "describe", "01K7", "--summary", "Only this."]);
    assert!(said.contains("--stdin"), "{said}");
}

#[test]
fn an_unknown_kind_is_refused_by_name() {
    let sandbox = Sandbox::new("kind");
    sandbox.write("src/a.rs", "fn one() {}\n");
    sandbox.ok(&["code", "index"]);

    let said = sandbox.fails(&["code", "recall", "one", "--kind", "closure"]);
    assert!(said.contains("'closure' is not a kind of node"), "{said}");
}

#[test]
fn the_bootstrap_answers_before_any_index_exists() {
    let sandbox = Sandbox::new("bootstrap");

    // No `code index` has run, and no file has been made.
    let code = sandbox.ok(&["code", "bootstrap"]);
    assert!(code.starts_with("## Code\n"), "{code}");
    assert!(code.contains("embornal code next --json"), "{code}");
    assert!(!code.contains("## Memory"), "{code}");

    let memory = sandbox.ok(&["memory", "bootstrap"]);
    assert!(memory.starts_with("## Memory\n"), "{memory}");
    assert!(!memory.contains("## Code"), "{memory}");

    let whole = sandbox.ok(&["bootstrap"]);
    assert!(whole.contains("## Memory"), "{whole}");
    assert!(whole.contains("## Code"), "{whole}");
}
