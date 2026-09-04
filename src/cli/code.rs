//! Everything below `embornal code`.
//!
//! The commands read the arguments, hand the work to [`CodeIndex`], and write
//! the answer. No command decides what a node or a summary is; that belongs to
//! the index itself.

use crate::cli::table::{Align, Table};
use crate::cli::tree::{Branch, print_tree};
use crate::cli::write_error;
use crate::code::api::CodeIndex;
use crate::code::node::NodeKind;
use crate::code::queue::{Batch, Written};
use crate::code::repo;
use crate::error::{Error, Result};
use clap::{Args, Subcommand};
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum CodeCommand {
    /// Reads the repository and brings the index up to date.
    Index(IndexArgs),
    /// Says how many nodes wait for a summary.
    Status(CollectionArgs),
    /// Gives the next file to describe.
    Next(NextArgs),
    /// Takes the summaries that an agent wrote.
    Describe(DescribeArgs),
    /// Draws the tree of the index.
    Tree(TreeArgs),
    /// Shows one node and what is known about it.
    Cat(CatArgs),
    /// Searches the summaries by word and by sense.
    Recall(RecallArgs),
    /// Writes the instructions that teach an agent to use the code index.
    Bootstrap(crate::cli::bootstrap::BootstrapArgs),
}

/// The arguments that say which index to work on.
#[derive(Debug, Args)]
pub struct CollectionArgs {
    /// The directory inside the repository. The default is where you are.
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// The name of the index. The default is the path of the repository, so a
    /// repository has one index and nobody must name it. Another name over the
    /// same repository is a fork, and it shares every summary that is written.
    #[arg(long, value_name = "NAME")]
    pub collection: Option<String>,
}

#[derive(Debug, Args)]
pub struct IndexArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// Reads every file again, whatever its hash says. Use it after a change
    /// to a grammar or to a query.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct NextArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// Writes the batch as JSON, which is what an agent reads.
    #[arg(long)]
    pub json: bool,
    /// Gives the root of the repository as well.
    ///
    /// The hash of the root follows every file, so it waits again after every
    /// commit. Without this flag the queue empties, and `code status` therefore
    /// means something.
    #[arg(long)]
    pub update_root: bool,
    /// How many batches to fetch at once, for handing work to several agents
    /// in parallel.
    ///
    /// The queue is a read query, not a lease: nothing marks a batch as
    /// taken, so two agents that ask before either describes back can see the
    /// same file. The default of one keeps the old output shape, a single
    /// object or `null`. Above one, `--json` always writes an array, even
    /// when only one batch came back.
    #[arg(long, default_value_t = 1)]
    pub batches: usize,
}

#[derive(Debug, Args)]
pub struct DescribeArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// The node to describe. Leave it out and give `--stdin` for a whole file.
    pub id: Option<String>,
    /// One line that says what the code does. About 140 characters.
    #[arg(long)]
    pub summary: Option<String>,
    /// What the code does and how, in a short paragraph.
    #[arg(long)]
    pub description: Option<String>,
    /// Reads a JSON array of `{id, summary, description}` from standard input.
    /// Multi-line text does not survive the quoting of a shell, so a batch
    /// comes this way.
    #[arg(long)]
    pub stdin: bool,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// The directory to draw. The default is the whole repository.
    #[arg(default_value = "")]
    pub under: String,
    /// How far down to draw.
    #[arg(long, value_name = "N")]
    pub depth: Option<u32>,
}

#[derive(Debug, Args)]
pub struct CatArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// The qualified name, such as `src/memory/api.rs::Memory::recall`, or the
    /// id that `next` gave.
    pub name: String,
}

#[derive(Debug, Args)]
pub struct RecallArgs {
    #[command(flatten)]
    pub which: CollectionArgs,
    /// What to search for.
    pub query: String,
    /// How many answers to give.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Keep one kind of node only, such as `function` or `class`.
    #[arg(long, value_name = "KIND")]
    pub kind: Option<String>,
}

/// Runs one `embornal code` command.
pub fn run(
    command: CodeCommand,
    open: impl FnOnce() -> Result<CodeIndex>,
    subject: &str,
    out: &mut impl Write,
) -> Result<()> {
    // The bootstrap is text. It touches no file, so it answers even before an
    // index exists.
    if let CodeCommand::Bootstrap(_) = command {
        return crate::cli::bootstrap::section(crate::cli::bootstrap::CODE, out);
    }

    let mut index = open()?;
    match command {
        CodeCommand::Bootstrap(_) => unreachable!("answered above"),
        CodeCommand::Index(args) => index_command(args, &mut index, out),
        CodeCommand::Status(args) => status(args, &index, out),
        CodeCommand::Next(args) => next(args, &index, out),
        CodeCommand::Describe(args) => describe(args, &mut index, subject, out),
        CodeCommand::Tree(args) => tree(args, &index, out),
        CodeCommand::Cat(args) => cat(args, &index, out),
        CodeCommand::Recall(args) => recall(args, &mut index, out),
    }
}

/// The repository and the name of the index that a command works on.
fn which(args: &CollectionArgs) -> Result<(PathBuf, String)> {
    let start = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir().map_err(|source| Error::Io {
            path: PathBuf::from("."),
            source,
        })?,
    };
    let root = repo::discover(&start)?;
    let name = args
        .collection
        .clone()
        .unwrap_or_else(|| repo::default_collection(&root));
    Ok((root, name))
}

/// `embornal code index`
fn index_command(args: IndexArgs, index: &mut CodeIndex, out: &mut impl Write) -> Result<()> {
    let (root, name) = which(&args.which)?;
    let report = index.index(&root, &name, args.all)?;

    writeln!(
        out,
        "collection {name}: {} files, {} parsed, {} removed, {} nodes, {} stale",
        report.files_seen, report.files_parsed, report.files_removed, report.nodes, report.stale
    )
    .map_err(write_error)?;
    if report.parse_errors > 0 {
        writeln!(
            out,
            "{} files that no grammar could read",
            report.parse_errors
        )
        .map_err(write_error)?;
    }
    Ok(())
}

/// `embornal code status`
fn status(args: CollectionArgs, index: &CodeIndex, out: &mut impl Write) -> Result<()> {
    let (_, name) = which(&args)?;
    let rows = index.status(&name)?;

    let mut table = Table::new(&[
        ("Kind", Align::Left),
        ("Nodes", Align::Right),
        ("Stale", Align::Right),
    ]);
    for (kind, nodes, stale) in &rows {
        table.row([kind.clone(), nodes.to_string(), stale.to_string()]);
    }
    table.render(out)
}

/// `embornal code next`
fn next(args: NextArgs, index: &CodeIndex, out: &mut impl Write) -> Result<()> {
    let (_, name) = which(&args.which)?;

    // One batch keeps the shape that every agent already reads: a single
    // object, or `null`. More than one always answers as an array, so a
    // caller that asks for several never has to guess which shape it got.
    if args.batches <= 1 {
        let Some(batch) = index.next(&name, args.update_root)? else {
            if args.json {
                writeln!(out, "null").map_err(write_error)?;
            } else {
                writeln!(out, "nothing waits").map_err(write_error)?;
            }
            return Ok(());
        };

        return if args.json {
            let text = serde_json::to_string_pretty(&batch)
                .map_err(|err| Error::BadArgument(err.to_string()))?;
            writeln!(out, "{text}").map_err(write_error)
        } else {
            print_batch(&batch, out)
        };
    }

    let batches = index.next_batch(&name, args.update_root, args.batches)?;
    if args.json {
        let text = serde_json::to_string_pretty(&batches)
            .map_err(|err| Error::BadArgument(err.to_string()))?;
        writeln!(out, "{text}").map_err(write_error)
    } else if batches.is_empty() {
        writeln!(out, "nothing waits").map_err(write_error)
    } else {
        for batch in &batches {
            print_batch(batch, out)?;
        }
        Ok(())
    }
}

/// Writes a batch for a reader, not for an agent.
fn print_batch(batch: &Batch, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{:?} {}", batch.kind, batch.rel_path).map_err(write_error)?;
    for item in &batch.nodes {
        let lines = match item.lines {
            Some([from, to]) => format!(" {from}-{to}"),
            None => String::new(),
        };
        writeln!(out, "  {} {} {}{lines}", item.id, item.kind, item.name).map_err(write_error)?;
    }
    Ok(())
}

/// `embornal code describe`
fn describe(
    args: DescribeArgs,
    index: &mut CodeIndex,
    subject: &str,
    out: &mut impl Write,
) -> Result<()> {
    let (_, name) = which(&args.which)?;

    let written: Vec<Written> = if args.stdin {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|source| Error::Io {
                path: PathBuf::from("<stdin>"),
                source,
            })?;
        serde_json::from_str(&text).map_err(|err| {
            Error::BadArgument(format!(
                "standard input must hold a JSON array of {{id, summary, description}}: {err}"
            ))
        })?
    } else {
        let (Some(id), Some(summary), Some(description)) =
            (args.id, args.summary, args.description)
        else {
            return Err(Error::BadArgument(
                "give an ID with --summary and --description, or --stdin with a JSON array".into(),
            ));
        };
        vec![Written {
            id,
            summary,
            description,
        }]
    };

    let count = index.describe(&name, &written, subject)?;
    writeln!(out, "described {count}").map_err(write_error)
}

/// `embornal code tree`
fn tree(args: TreeArgs, index: &CodeIndex, out: &mut impl Write) -> Result<()> {
    let (_, name) = which(&args.which)?;
    let tree = crate::code::api::tree(index.database(), &name, &args.under, args.depth)?;
    print_tree(&tree, out)
}

/// `embornal code cat`
fn cat(args: CatArgs, index: &CodeIndex, out: &mut impl Write) -> Result<()> {
    let (_, name) = which(&args.which)?;
    let node = index.cat(&name, &args.name)?;

    writeln!(out, "# {}", node.qualified_name).map_err(write_error)?;
    let lines = match (node.start_line, node.end_line) {
        (Some(from), Some(to)) => format!(":{from}-{to}"),
        _ => String::new(),
    };
    writeln!(out, "{} {}{lines}", node.kind, node.rel_path).map_err(write_error)?;

    match (node.summary, node.description) {
        (Some(summary), Some(description)) => {
            writeln!(out).map_err(write_error)?;
            writeln!(out, "{summary}").map_err(write_error)?;
            writeln!(out).map_err(write_error)?;
            writeln!(out, "{description}").map_err(write_error)?;
            if let (Some(author), Some(at)) = (node.author, node.written_at) {
                writeln!(out).map_err(write_error)?;
                writeln!(out, "-- {author}, {at}").map_err(write_error)?;
            }
        }
        _ => {
            writeln!(out).map_err(write_error)?;
            writeln!(out, "no summary yet: run `embornal code next`").map_err(write_error)?;
        }
    }
    Ok(())
}

/// `embornal code recall`
fn recall(args: RecallArgs, index: &mut CodeIndex, out: &mut impl Write) -> Result<()> {
    let (_, name) = which(&args.which)?;
    let kind = match args.kind.as_deref() {
        Some(text) => Some(
            NodeKind::parse(text)
                .ok_or_else(|| Error::BadArgument(format!("'{text}' is not a kind of node")))?,
        ),
        None => None,
    };

    let hits = index.recall(&name, &args.query, args.limit, kind)?;
    let mut table = Table::new(&[
        ("Score", Align::Right),
        ("Kind", Align::Left),
        ("Name", Align::Left),
        ("Summary", Align::Left),
    ]);
    for hit in &hits {
        table.row([
            format!("{:.3}", hit.score),
            hit.kind.clone(),
            hit.qualified_name.clone(),
            hit.summary.clone(),
        ]);
    }
    table.render(out)
}

/// Says how a node of the index is drawn in a tree.
impl Branch for crate::code::api::TreeNode {
    fn root_label(&self) -> String {
        if self.rel_path.is_empty() {
            "/".to_string()
        } else {
            self.rel_path.clone()
        }
    }

    fn label(&self) -> String {
        self.name.clone()
    }

    /// A node that carries no summary yet is marked, so that the tree says
    /// where the work is.
    fn mark(&self) -> &'static str {
        if self.described { "" } else { "*" }
    }

    fn children(&self) -> &[Self] {
        &self.children
    }
}
