//! The command line.
//!
//! This module reads the arguments and hands the work to the memory. Each
//! group of commands lives in its own module:
//!
//! - [`memory`]: everything below `embornal memory`.
//! - [`skill`]: the instructions that teach an agent to use the memory.
//!
//! [`table`] holds the output shape that the commands share.

pub mod memory;
pub mod skill;
pub mod table;

use crate::config::{Config, Paths};
use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::api::Memory;
use clap::{Parser, Subcommand};
use std::io::Write;

#[derive(Debug, Parser)]
#[command(name = "embornal", version, about = "A toolkit for agents")]
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
    /// Writes the instructions that teach an agent to use the memory.
    Skill(skill::SkillArgs),
}

/// Runs the command and writes to `out`.
pub fn run(cli: Cli, out: &mut impl Write) -> Result<()> {
    match cli.command {
        // The skill is text. It touches no file, so it answers even before a
        // memory exists.
        Command::Skill(args) => skill::run(args, out),

        Command::Memory(command) => {
            let handle = open_memory(cli.as_subject)?;
            memory::run(command, handle, out)
        }
    }
}

/// Opens the memory of the home directory.
fn open_memory(subject: Option<String>) -> Result<Memory> {
    let paths = Paths::discover()?;
    paths.ensure()?;

    let mut config = Config::load(&paths.config_file())?;
    if let Some(name) = subject {
        config.subject = Subject::new(name);
    }
    Memory::open(&paths, config)
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
