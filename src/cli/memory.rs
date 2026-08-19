//! The `embornal memory` commands.
//!
//! Each command reads its arguments, asks [`Memory`] for the work and prints
//! the answer. No command decides what a fact or a path is; that belongs to
//! the memory itself.

use crate::cli::table::{Align, Table};
use crate::cli::write_error;
use crate::error::{Error, Result};
use crate::memory::api::{
    CatOptions, Listing, Memory, RecallOptions, ReindexOptions, ReindexReport, TreeNode,
    TreeOptions,
};
use crate::memory::backend::{Backend, MemoryApi};
use crate::memory::fact::{Fact, NewFact, OrderBy, ScoredFact};
use crate::memory::link::{self, Segment};
use crate::memory::path::WikiPath;
use crate::memory::tag::{Tag, TagSet};
use clap::{Args, Subcommand};
use std::io::Write;

/// The port that `wiki` listens on.
pub const WIKI_PORT: u16 = 1337;

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Writes one fact to a path.
    Store(StoreArgs),
    /// Lists the paths below a path, one level.
    Ls(LsArgs),
    /// Shows the whole tree below a path.
    Tree(TreeArgs),
    /// Shows the document of a path.
    Cat(CatArgs),
    /// Searches the memory.
    Recall(RecallArgs),
    /// Gives a vector to each fact that has none.
    Reindex(ReindexArgs),
    /// Starts the wiki, which shows the memory in a browser.
    Wiki(WikiArgs),
}

#[derive(Debug, Args)]
pub struct StoreArgs {
    /// The path, such as /projects/embornal.
    pub path: String,
    /// The fact. Links have the [[/path]] form.
    pub content: String,
    /// An access tag, in the key=value form. Repeat for more.
    #[arg(long = "tag", value_name = "KEY=VALUE")]
    pub tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// The path to list. The default is the root.
    #[arg(default_value = "/")]
    pub path: String,
    /// Writes one name for each line, with no table. Use this in a pipe.
    #[arg(long)]
    pub plain: bool,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    /// The path at the top of the tree. The default is the root.
    #[arg(default_value = "/")]
    pub path: String,
    /// Shows the paths that hold paths below them, and nothing else.
    #[arg(long = "dirs-only")]
    pub dirs_only: bool,
}

#[derive(Debug, Args)]
pub struct CatArgs {
    /// The path to read.
    pub path: String,
    /// How many facts to show.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// How to sort: date or signal.
    #[arg(long = "order-by", value_name = "METHOD")]
    pub order_by: Option<String>,
    /// Counts the reading as a recall, which lifts the signal.
    #[arg(long)]
    pub recall: bool,
    /// Shows the owner and tags of each fact.
    #[arg(long)]
    pub meta: bool,
}

#[derive(Debug, Args)]
pub struct RecallArgs {
    /// What to look for. With no words, the strongest facts come back.
    pub content: Option<String>,
    /// How many facts to show.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Searches below this path only.
    #[arg(long, value_name = "PATH")]
    pub under: Option<String>,
    /// Shows the value that decided the order of the answer.
    #[arg(long)]
    pub scores: bool,
    /// Writes one fact for each line, with no table. Use this in a pipe.
    #[arg(long)]
    pub plain: bool,
    /// Shows the owner and tags of each fact.
    #[arg(long)]
    pub meta: bool,
}

#[derive(Debug, Args)]
pub struct ReindexArgs {
    /// Stops after this many facts.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Writes the vector of every fact again. Use this after a change of
    /// model.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct WikiArgs {
    /// The port to listen on.
    #[arg(long, default_value_t = WIKI_PORT)]
    pub port: u16,
}

/// Runs one memory command.
pub fn run(command: MemoryCommand, mut memory: Backend, out: &mut impl Write) -> Result<()> {
    match command {
        MemoryCommand::Store(args) => store(args, &mut memory, out),
        MemoryCommand::Ls(args) => ls(args, &mut memory, out),
        MemoryCommand::Tree(args) => tree(args, &mut memory, out),
        MemoryCommand::Cat(args) => cat(args, &mut memory, out),
        MemoryCommand::Recall(args) => recall(args, &mut memory, out),

        // These two work on the file itself, so they run on the machine that
        // holds the memory and nowhere else.
        MemoryCommand::Reindex(args) => reindex(args, memory.into_local("memory reindex")?, out),
        MemoryCommand::Wiki(args) => wiki(args, memory.into_local("memory wiki")?),
    }
}

// ---------------------------------------------------------------------------
// The commands
// ---------------------------------------------------------------------------

/// `embornal memory store [PATH] "[CONTENT]"`
pub fn store(args: StoreArgs, memory: &mut impl MemoryApi, out: &mut impl Write) -> Result<()> {
    let path = WikiPath::parse(&args.path)?;
    let mut tags = Vec::with_capacity(args.tags.len());
    for text in &args.tags {
        tags.push(Tag::parse(text)?);
    }

    let fact = memory.store(NewFact {
        path,
        content: args.content,
        tags,
        supersedes_id: None,
    })?;

    writeln!(out, "{} {}", fact.ulid, fact.path).map_err(write_error)
}

/// `embornal memory ls [PATH]`
pub fn ls(args: LsArgs, memory: &mut impl MemoryApi, out: &mut impl Write) -> Result<()> {
    let path = WikiPath::parse(&args.path)?;
    let listing = memory.ls(&path)?;

    if args.plain {
        print_names(&listing, out)
    } else {
        print_listing(&listing, out)
    }
}

/// `embornal memory tree [PATH]`
pub fn tree(args: TreeArgs, memory: &mut impl MemoryApi, out: &mut impl Write) -> Result<()> {
    let path = WikiPath::parse(&args.path)?;
    let tree = memory.tree(
        &path,
        TreeOptions {
            dirs_only: args.dirs_only,
        },
    )?;
    print_tree(&tree, out)
}

/// `embornal memory cat [PATH]`
pub fn cat(args: CatArgs, memory: &mut impl MemoryApi, out: &mut impl Write) -> Result<()> {
    let path = WikiPath::parse(&args.path)?;
    let order_by = match args.order_by.as_deref() {
        Some(text) => text.parse::<OrderBy>().map_err(Error::BadArgument)?,
        None => memory.recall_defaults()?.order_by,
    };

    let facts = memory.cat(
        &path,
        CatOptions {
            order_by,
            limit: args.limit,
            reinforce: args.recall,
        },
    )?;
    let tags = tags_of(args.meta, &facts, memory)?;
    print_document(&path, &facts, tags.as_deref(), out)
}

/// `embornal memory recall [CONTENT]`
pub fn recall(args: RecallArgs, memory: &mut impl MemoryApi, out: &mut impl Write) -> Result<()> {
    let under = match args.under.as_deref() {
        Some(text) => Some(WikiPath::parse(text)?),
        None => None,
    };

    let defaults = memory.recall_defaults()?;
    let hits = memory.recall(
        args.content.as_deref(),
        RecallOptions {
            limit: args.limit.unwrap_or(defaults.limit),
            under,
            reinforce: true,
        },
    )?;

    let facts: Vec<Fact> = hits.iter().map(|hit| hit.fact.clone()).collect();
    let tags = tags_of(args.meta, &facts, memory)?;
    if args.plain {
        print_hit_lines(&hits, tags.as_deref(), out)
    } else {
        print_hits(&hits, args.scores, tags.as_deref(), out)
    }
}

/// Reads the resolved tags when the caller asked for metadata.
fn tags_of(meta: bool, facts: &[Fact], memory: &mut impl MemoryApi) -> Result<Option<Vec<TagSet>>> {
    if !meta {
        return Ok(None);
    }
    facts
        .iter()
        .map(|fact| memory.effective_tags(fact.id))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// `embornal memory reindex`
pub fn reindex(args: ReindexArgs, mut memory: Memory, out: &mut impl Write) -> Result<()> {
    let report = memory.reindex(ReindexOptions {
        limit: args.limit,
        all: args.all,
    })?;
    print_reindex(&report, out)
}

/// `embornal memory wiki`
pub fn wiki(args: WikiArgs, memory: Memory) -> Result<()> {
    crate::wiki::wiki(memory, args.port)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Prints a listing as a table.
///
/// The path column holds the whole path of each child, so that a reader can
/// hand a line straight to another command. The two counts say what the path
/// holds, so no mark is needed.
pub fn print_listing(listing: &Listing, out: &mut impl Write) -> Result<()> {
    let mut table = Table::new(&[
        ("Path", Align::Left),
        ("Facts", Align::Right),
        ("Children", Align::Right),
    ]);

    for entry in &listing.children {
        table.row([
            entry.path.to_string(),
            entry.fact_count.to_string(),
            entry.child_count.to_string(),
        ]);
    }
    table.render(out)
}

/// Prints one path for each line, with no table.
///
/// A path that holds children ends with `/`, as a directory does. A path that
/// holds facts of its own carries a `*`, because a path can be a prefix and
/// hold content at the same time.
pub fn print_names(listing: &Listing, out: &mut impl Write) -> Result<()> {
    for entry in &listing.children {
        let children = if entry.child_count > 0 { "/" } else { "" };
        let content = if entry.has_content() { "*" } else { "" };
        writeln!(out, "{}{children}{content}", entry.path).map_err(write_error)?;
    }
    Ok(())
}

/// Prints a tree.
///
/// The top holds its whole path, and each path below it shows its own name
/// only, because the lines already say where it sits. A name that carries a
/// `*` holds facts of its own.
pub fn print_tree(tree: &TreeNode, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{}{}", tree.path, mark(tree)).map_err(write_error)?;
    print_branches(tree, "", out)
}

/// Writes the paths below one path.
fn print_branches(node: &TreeNode, prefix: &str, out: &mut impl Write) -> Result<()> {
    let last = node.children.len().saturating_sub(1);
    for (index, child) in node.children.iter().enumerate() {
        let (elbow, next) = if index == last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };

        let name = child.path.segment().unwrap_or("/");
        writeln!(out, "{prefix}{elbow}{name}{}", mark(child)).map_err(write_error)?;
        print_branches(child, &format!("{prefix}{next}"), out)?;
    }
    Ok(())
}

/// Returns the mark of a path that holds facts of its own.
fn mark(node: &TreeNode) -> &'static str {
    if node.fact_count > 0 { "*" } else { "" }
}

/// Prints the document of one path.
pub fn print_document(
    path: &WikiPath,
    facts: &[Fact],
    tags: Option<&[TagSet]>,
    out: &mut impl Write,
) -> Result<()> {
    writeln!(out, "# {path}\n").map_err(write_error)?;
    for (index, fact) in facts.iter().enumerate() {
        writeln!(out, "- {}", fact.content).map_err(write_error)?;
        if let Some(tags) = tags {
            writeln!(out, "  - Owner: {}", fact.owner).map_err(write_error)?;
            writeln!(out, "  - Tags: {}", tags[index]).map_err(write_error)?;
        }
    }
    Ok(())
}

/// Prints what a recall found, as a table.
///
/// The signal column holds the strength of each fact at this moment, from
/// 1.000 for a fact that somebody just read to 0.000 for one that the memory
/// almost lost. It is not the order of the answer: the order also weighs how
/// well the fact matches the words.
///
/// With `scores`, the value that decided the order comes as well.
pub fn print_hits(
    hits: &[ScoredFact],
    scores: bool,
    tags: Option<&[TagSet]>,
    out: &mut impl Write,
) -> Result<()> {
    let mut columns = vec![("Path", Align::Left), ("Signal", Align::Right)];
    if scores {
        columns.push(("Score", Align::Right));
    }
    if tags.is_some() {
        columns.push(("Owner", Align::Left));
        columns.push(("Tags", Align::Left));
    }
    columns.push(("Fact", Align::Left));

    let mut table = Table::new(&columns);
    for (index, hit) in hits.iter().enumerate() {
        let mut row = vec![
            hit.fact.path.to_string(),
            format!("{:.3}", hit.signal_strength),
        ];
        if scores {
            row.push(format!("{:.3}", hit.score));
        }
        if let Some(tags) = tags {
            row.push(hit.fact.owner.clone());
            row.push(tags[index].to_string());
        }
        row.push(hit.fact.content.clone());
        table.row(row);
    }
    table.render(out)
}

/// Says what the backfill did.
pub fn print_reindex(report: &ReindexReport, out: &mut impl Write) -> Result<()> {
    if !report.has_model {
        return writeln!(
            out,
            "{} facts wait for a vector, but this memory has no embedding model",
            report.pending
        )
        .map_err(write_error);
    }
    let Some(model) = &report.model else {
        return writeln!(out, "each fact has a vector").map_err(write_error);
    };

    writeln!(
        out,
        "{} of {} facts have a vector from {model}",
        report.done, report.pending
    )
    .map_err(write_error)
}

/// Prints one fact for each line, with no table.
pub fn print_hit_lines(
    hits: &[ScoredFact],
    tags: Option<&[TagSet]>,
    out: &mut impl Write,
) -> Result<()> {
    for (index, hit) in hits.iter().enumerate() {
        if let Some(tags) = tags {
            writeln!(
                out,
                "{}\t{}\t{}\t{}",
                hit.fact.path, hit.fact.owner, tags[index], hit.fact.content
            )
            .map_err(write_error)?;
        } else {
            writeln!(out, "{}\t{}", hit.fact.path, hit.fact.content).map_err(write_error)?;
        }
    }
    Ok(())
}

/// Writes the content of a fact for a terminal, with the links flattened.
pub fn plain_text(content: &str) -> String {
    link::parse(content)
        .iter()
        .map(|segment| match segment {
            Segment::Text(text) => (*text).to_string(),
            Segment::Link { target, .. } => target.to_string(),
            Segment::Broken(text) => format!("[[{text}]]"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::memory::path::PathEntry;
    use clap::Parser;

    fn text(f: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> String {
        let mut buffer = Vec::new();
        f(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    fn parse(args: &[&str]) -> MemoryCommand {
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Memory(command) = cli.command else {
            panic!("expected a memory command");
        };
        command
    }

    #[test]
    fn reads_a_store_command() {
        let MemoryCommand::Store(args) = parse(&[
            "embornal",
            "memory",
            "store",
            "/projects/embornal",
            "It uses SQLite.",
            "--tag",
            "visibility=private",
        ]) else {
            panic!("expected a store command");
        };
        assert_eq!(args.path, "/projects/embornal");
        assert_eq!(args.content, "It uses SQLite.");
        assert_eq!(args.tags, ["visibility=private"]);
    }

    #[test]
    fn a_store_needs_a_path_and_content() {
        assert!(Cli::try_parse_from(["embornal", "memory", "store", "/a"]).is_err());
    }

    #[test]
    fn ls_reads_the_root_by_default() {
        let MemoryCommand::Ls(args) = parse(&["embornal", "memory", "ls"]) else {
            panic!("expected an ls command");
        };
        assert_eq!(args.path, "/");
        assert!(!args.plain);
    }

    #[test]
    fn reads_the_tree_options() {
        let MemoryCommand::Tree(args) = parse(&["embornal", "memory", "tree"]) else {
            panic!("expected a tree command");
        };
        assert_eq!(args.path, "/");
        assert!(!args.dirs_only);

        let MemoryCommand::Tree(args) = parse(&["embornal", "memory", "tree", "/a", "--dirs-only"])
        else {
            panic!("expected a tree command");
        };
        assert_eq!(args.path, "/a");
        assert!(args.dirs_only);
    }

    fn node(path: &str, facts: u64, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            path: WikiPath::parse(path).unwrap(),
            fact_count: facts,
            children,
        }
    }

    #[test]
    fn a_tree_draws_its_branches() {
        let tree = node(
            "/projects",
            0,
            vec![
                node(
                    "/projects/embornal",
                    2,
                    vec![
                        node("/projects/embornal/design", 1, vec![]),
                        node("/projects/embornal/tests", 0, vec![]),
                    ],
                ),
                node("/projects/rust", 1, vec![]),
            ],
        );

        let output = text(|out| print_tree(&tree, out));
        assert_eq!(
            output,
            "/projects\n\
             ├── embornal*\n\
             │   ├── design*\n\
             │   └── tests\n\
             └── rust*\n"
        );
    }

    #[test]
    fn a_tree_of_one_path_holds_one_line() {
        let output = text(|out| print_tree(&node("/a", 3, vec![]), out));
        assert_eq!(output, "/a*\n");
    }

    #[test]
    fn the_top_of_a_tree_shows_its_whole_path() {
        let tree = node("/a/b", 0, vec![node("/a/b/c", 0, vec![])]);
        let output = text(|out| print_tree(&tree, out));
        assert_eq!(output, "/a/b\n└── c\n");
    }

    #[test]
    fn the_root_reads_as_a_tree_as_well() {
        let tree = node("/", 0, vec![node("/a", 1, vec![])]);
        let output = text(|out| print_tree(&tree, out));
        assert_eq!(output, "/\n└── a*\n");
    }

    #[test]
    fn reads_the_cat_options() {
        let MemoryCommand::Cat(args) = parse(&[
            "embornal",
            "memory",
            "cat",
            "/a",
            "--limit",
            "5",
            "--order-by",
            "signal",
        ]) else {
            panic!("expected a cat command");
        };
        assert_eq!(args.limit, Some(5));
        assert_eq!(args.order_by.as_deref(), Some("signal"));
        assert!(!args.recall);
        assert!(!args.meta);

        let MemoryCommand::Cat(args) = parse(&["embornal", "memory", "cat", "/a", "--meta"]) else {
            panic!("expected a cat command");
        };
        assert!(args.meta);
    }

    #[test]
    fn recall_works_with_no_words() {
        let MemoryCommand::Recall(args) = parse(&["embornal", "memory", "recall"]) else {
            panic!("expected a recall command");
        };
        assert_eq!(args.content, None);
    }

    #[test]
    fn reads_the_recall_options() {
        let MemoryCommand::Recall(args) = parse(&[
            "embornal", "memory", "recall", "sqlite", "--limit", "3", "--under", "/db", "--scores",
        ]) else {
            panic!("expected a recall command");
        };
        assert_eq!(args.content.as_deref(), Some("sqlite"));
        assert_eq!(args.limit, Some(3));
        assert_eq!(args.under.as_deref(), Some("/db"));
        assert!(args.scores);
        assert!(!args.meta);

        let MemoryCommand::Recall(args) =
            parse(&["embornal", "memory", "recall", "sqlite", "--meta"])
        else {
            panic!("expected a recall command");
        };
        assert!(args.meta);
    }

    #[test]
    fn the_wiki_holds_the_documented_port() {
        let MemoryCommand::Wiki(args) = parse(&["embornal", "memory", "wiki"]) else {
            panic!("expected a wiki command");
        };
        assert_eq!(args.port, WIKI_PORT);
        assert_eq!(WIKI_PORT, 1337);
    }

    fn sample() -> Listing {
        Listing {
            path: WikiPath::root(),
            fact_count: 0,
            subtree_fact_count: 0,
            children: vec![
                PathEntry {
                    path: WikiPath::parse("/both").unwrap(),
                    fact_count: 2,
                    subtree_fact_count: 2,
                    child_count: 1,
                },
                PathEntry {
                    path: WikiPath::parse("/leaf").unwrap(),
                    fact_count: 1,
                    subtree_fact_count: 1,
                    child_count: 0,
                },
                PathEntry {
                    path: WikiPath::parse("/empty").unwrap(),
                    fact_count: 0,
                    subtree_fact_count: 0,
                    child_count: 2,
                },
            ],
        }
    }

    #[test]
    fn a_listing_prints_as_a_table_of_whole_paths() {
        let output = text(|out| print_listing(&sample(), out));
        assert_eq!(
            output,
            "| Path   | Facts | Children |\n\
             +--------+-------+----------+\n\
             | /both  |     2 |        1 |\n\
             | /leaf  |     1 |        0 |\n\
             | /empty |     0 |        2 |\n"
        );
    }

    #[test]
    fn a_deep_listing_shows_the_whole_path() {
        let listing = Listing {
            path: WikiPath::parse("/work").unwrap(),
            fact_count: 0,
            subtree_fact_count: 0,
            children: vec![PathEntry {
                path: WikiPath::parse("/work/acme").unwrap(),
                fact_count: 3,
                subtree_fact_count: 3,
                child_count: 0,
            }],
        };
        let output = text(|out| print_listing(&listing, out));
        assert!(output.contains("| /work/acme |"), "{output}");
    }

    #[test]
    fn a_listing_with_no_child_still_shows_its_heading() {
        let listing = Listing {
            path: WikiPath::root(),
            fact_count: 0,
            subtree_fact_count: 0,
            children: Vec::new(),
        };
        let output = text(|out| print_listing(&listing, out));
        assert_eq!(
            output,
            "| Path | Facts | Children |\n+------+-------+----------+\n"
        );
    }

    #[test]
    fn the_plain_form_marks_children_and_content() {
        let output = text(|out| print_names(&sample(), out));
        assert_eq!(output, "/both/*\n/leaf*\n/empty/\n");
    }

    #[test]
    fn the_plain_form_of_an_empty_listing_says_nothing() {
        let listing = Listing {
            path: WikiPath::root(),
            fact_count: 0,
            subtree_fact_count: 0,
            children: Vec::new(),
        };
        assert_eq!(text(|out| print_names(&listing, out)), "");
    }

    fn hit(path: &str, content: &str, signal: f64, score: f64) -> ScoredFact {
        use crate::memory::fact::{FactId, Signal};
        use crate::memory::path::PathId;
        use chrono::Utc;

        let now = Utc::now();
        ScoredFact {
            fact: Fact {
                id: FactId(1),
                ulid: ulid::Ulid::nil(),
                path_id: PathId(2),
                path: WikiPath::parse(path).unwrap(),
                content: content.to_string(),
                owner: "cli".to_string(),
                created_at: now,
                signal: Signal::new(now),
                supersedes_id: None,
                deleted_at: None,
                embedding_model: None,
            },
            keyword_score: None,
            vector_score: None,
            signal_strength: signal,
            score,
        }
    }

    #[test]
    fn a_recall_prints_the_path_the_signal_and_the_fact() {
        let hits = [
            hit("/db", "The memory uses SQLite.", 1.0, 1.5),
            hit("/lang", "Rust.", 0.25, 0.8),
        ];

        let output = text(|out| print_hits(&hits, false, None, out));
        assert_eq!(
            output,
            "| Path  | Signal | Fact                    |\n\
             +-------+--------+-------------------------+\n\
             | /db   |  1.000 | The memory uses SQLite. |\n\
             | /lang |  0.250 | Rust.                   |\n"
        );
    }

    #[test]
    fn the_score_column_comes_only_when_it_is_asked_for() {
        let hits = [hit("/db", "one", 0.5, 1.25)];

        let plain = text(|out| print_hits(&hits, false, None, out));
        assert!(!plain.contains("Score"));

        let with_scores = text(|out| print_hits(&hits, true, None, out));
        assert_eq!(
            with_scores,
            "| Path | Signal | Score | Fact |\n\
             +------+--------+-------+------+\n\
             | /db  |  0.500 | 1.250 | one  |\n"
        );
    }

    #[test]
    fn a_recall_that_found_nothing_still_shows_its_heading() {
        let output = text(|out| print_hits(&[], false, None, out));
        assert_eq!(
            output,
            "| Path | Signal | Fact |\n+------+--------+------+\n"
        );
    }

    #[test]
    fn the_plain_form_of_a_recall_feeds_a_pipe() {
        let hits = [hit("/db", "The memory uses SQLite.", 1.0, 1.5)];
        let output = text(|out| print_hit_lines(&hits, None, out));
        assert_eq!(output, "/db\tThe memory uses SQLite.\n");
    }

    #[test]
    fn a_document_starts_with_its_path() {
        let output = text(|out| print_document(&WikiPath::parse("/a").unwrap(), &[], None, out));
        assert_eq!(output, "# /a\n\n");
    }

    #[test]
    fn metadata_adds_the_owner_and_tags_to_a_recall() {
        let hits = [hit("/db", "one", 0.5, 1.25)];
        let tags: TagSet = [
            Tag::parse("kind=note").unwrap(),
            Tag::parse("owner=cli").unwrap(),
        ]
        .into_iter()
        .collect();

        let output = text(|out| print_hits(&hits, false, Some(&[tags]), out));
        assert_eq!(
            output,
            "| Path | Signal | Owner | Tags                | Fact |\n\
             +------+--------+-------+---------------------+------+\n\
             | /db  |  0.500 | cli   | kind=note owner=cli | one  |\n"
        );
    }

    #[test]
    fn metadata_adds_tab_separated_fields_to_plain_recall() {
        let hits = [hit("/db", "one", 0.5, 1.25)];
        let tags: TagSet = [Tag::parse("owner=cli").unwrap()].into_iter().collect();

        let output = text(|out| print_hit_lines(&hits, Some(&[tags]), out));
        assert_eq!(output, "/db\tcli\towner=cli\tone\n");
    }

    #[test]
    fn metadata_appears_below_each_fact_in_a_document() {
        let facts = [hit("/a", "one", 0.5, 1.25).fact];
        let tags: TagSet = [Tag::parse("owner=cli").unwrap()].into_iter().collect();

        let output =
            text(|out| print_document(&WikiPath::parse("/a").unwrap(), &facts, Some(&[tags]), out));
        assert_eq!(
            output,
            "# /a\n\n- one\n  - Owner: cli\n  - Tags: owner=cli\n"
        );
    }

    #[test]
    fn a_link_reads_as_its_target_in_a_terminal() {
        assert_eq!(plain_text("see [[/a/b]] now"), "see /a/b now");
        assert_eq!(plain_text("see [[TODO]] now"), "see [[TODO]] now");
        assert_eq!(plain_text("plain"), "plain");
    }
}
