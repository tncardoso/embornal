//! The `embornal bootstrap` command.
//!
//! The command writes the instructions that teach an agent to use the memory.
//! The text goes to standard output, and should be added to the global AGENTS.md:
//!
//! ```sh
//! embornal bootstrap >> ~/.claude/AGENTS.md
//! ```

use crate::cli::write_error;
use crate::error::Result;
use clap::Args;
use std::io::Write;

/// The bootstrap instructions, read from the prompts file.
pub const BOOTSTRAP: &str = include_str!("../prompts/bootstrap.txt");

#[derive(Debug, Args)]
pub struct BootstrapArgs {}

/// `embornal bootstrap`
pub fn run(_args: BootstrapArgs, out: &mut impl Write) -> Result<()> {
    write!(out, "{BOOTSTRAP}").map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut buffer = Vec::new();
        run(BootstrapArgs {}, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn the_bootstrap_opens_with_the_memory_heading() {
        let bootstrap = text();
        assert!(bootstrap.starts_with("## Memory\n"), "{bootstrap}");
    }

    #[test]
    fn the_bootstrap_names_each_command_that_it_asks_for() {
        let bootstrap = text();
        for command in [
            "embornal memory cat /memory",
            "embornal memory cat /",
            "embornal memory store <path> <fact>",
            "embornal memory ls <path>",
            "embornal memory tree <path>",
            "embornal memory recall <query>",
        ] {
            assert!(bootstrap.contains(command), "{command} is missing");
        }
    }

    #[test]
    fn the_bootstrap_holds_the_four_moments() {
        let bootstrap = text();
        for heading in [
            "## Memory",
            "### At startup",
            "### While working",
            "### When starting a new task",
            "### Instructions",
        ] {
            assert!(bootstrap.contains(heading), "{heading} is missing");
        }
    }

    #[test]
    fn the_bootstrap_tells_a_subagent_to_stand_back() {
        assert!(text().contains(
            "Subagents should never update memories. Leave that for the main agent"
        ));
    }

    #[test]
    fn the_bootstrap_stays_short() {
        // The text is instructions, not a manual. A reader must take it in at
        // one glance.
        let lines = text().lines().count();
        assert!(lines < 60, "the bootstrap grew to {lines} lines");
    }

    #[test]
    fn the_bootstrap_ends_with_one_newline() {
        let bootstrap = text();
        assert!(bootstrap.ends_with('\n'));
        assert!(!bootstrap.ends_with("\n\n"));
    }
}
