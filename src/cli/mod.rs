//! The command line.
//!
//! This module reads the arguments and hands the work to the memory. Each
//! group of commands lives in its own module:
//!
//! - [`bootstrap`]: the bootstrap instructions for agents to use the memory.
//! - [`memory`]: everything below `embornal memory`.
//! - [`token`]: the secrets that let a client reach a server.
//!
//! [`table`] holds the output shape that the commands share.

pub mod bootstrap;
pub mod memory;
pub mod table;
pub mod token;

use crate::client::Client;
use crate::config::{Config, Paths};
use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::api::Memory;
use crate::memory::backend::Backend;
use clap::{Parser, Subcommand};
use std::io::Write;

/// The banner shown before the help text.
const BANNER: &str = include_str!("banner.ans");

#[derive(Debug, Parser)]
#[command(
    name = "embornal",
    version,
    about = "A toolkit for agents",
    before_help = BANNER
)]
pub struct Cli {
    /// Who asks. Access control reads this.
    #[arg(long, global = true, value_name = "SUBJECT")]
    pub as_subject: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read and write the memory.
    #[command(subcommand)]
    Memory(memory::MemoryCommand),
    /// Make and stop the tokens that let a client reach a server.
    #[command(subcommand)]
    Token(token::TokenCommand),
    /// Puts this memory behind HTTP, so that other machines can use it.
    Serve(ServeArgs),
    /// Writes the bootstrap instructions for agents. Add to ~/.claude/AGENTS.md
    Bootstrap(bootstrap::BootstrapArgs),
}

/// Runs the command and writes to `out`.
pub fn run(cli: Cli, out: &mut impl Write) -> Result<()> {
    match cli.command {
        // The bootstrap is text. It touches no file, so it answers even before a
        // memory exists.
        Command::Bootstrap(args) => bootstrap::run(args, out),

        Command::Memory(command) => memory::run(command, open(cli.as_subject)?, out),

        // The tokens live in the file, and the first one cannot come through
        // a server, so these commands always work on the memory of this
        // machine.
        Command::Token(command) => {
            token::run(command, open(cli.as_subject)?.into_local("token")?, out)
        }

        Command::Serve(args) => serve(args, open(None)?.into_local("serve")?),
    }
}

/// The arguments of `embornal serve`.
#[derive(Debug, clap::Args)]
pub struct ServeArgs {
    /// The port to listen on.
    #[arg(long, default_value_t = crate::api::SERVE_PORT)]
    pub port: u16,
    /// The address to listen on.
    ///
    /// The default answers this machine only. Give `0.0.0.0` to answer the
    /// network, and put a proxy in front of it for TLS: this server speaks
    /// HTTP, and a token on a plain wire is a token that somebody can read.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: std::net::IpAddr,
}

/// `embornal serve`
fn serve(args: ServeArgs, memory: Memory) -> Result<()> {
    crate::api::serve(memory, std::net::SocketAddr::new(args.bind, args.port))
}

/// Opens the memory that this machine is set up to use.
///
/// A `server` section in the configuration makes this a client, and the
/// commands then do their work there. Without that section, the memory is the
/// file of this machine.
fn open(subject: Option<String>) -> Result<Backend> {
    let paths = Paths::discover()?;
    paths.ensure()?;

    let mut config = Config::load(&paths.config_file())?;

    if let Some(server) = &config.server {
        // On a server the token says who asks, and nothing else does. The
        // local subject flag has no effect in client mode.
        return Ok(Backend::Remote(Box::new(Client::open(server)?)));
    }

    if let Some(name) = subject {
        config.subject = Subject::parse(&name)?;
    }
    Ok(Backend::Local(Box::new(Memory::open(&paths, config)?)))
}

/// Turns an error of the writer into an error of the tool.
pub(crate) fn write_error(source: std::io::Error) -> Error {
    Error::Io {
        path: std::path::PathBuf::from("<stdout>"),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_subject_flag_is_global() {
        let cli =
            Cli::try_parse_from(["embornal", "--as-subject", "agent", "memory", "ls"]).unwrap();
        assert_eq!(cli.as_subject.as_deref(), Some("agent"));
    }
}
