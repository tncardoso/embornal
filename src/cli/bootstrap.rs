//! The `embornal bootstrap` command.
//!
//! The command writes the instructions that teach an agent to use Embornal.
//! The text goes to standard output, and should be added to the global
//! AGENTS.md:
//!
//! ```sh
//! embornal bootstrap >> ~/.claude/AGENTS.md
//! ```
//!
//! One tool at a time is served by `embornal memory bootstrap` and
//! `embornal code bootstrap`, for an agent that uses one of them and not the
//! other.

use crate::cli::write_error;
use crate::error::Result;
use clap::Args;
use std::io::Write;

/// How an agent uses the memory.
pub const MEMORY: &str = include_str!("../prompts/bootstrap_memory.txt");

/// How an agent uses the code index.
pub const CODE: &str = include_str!("../prompts/bootstrap_code.txt");

#[derive(Debug, Args)]
pub struct BootstrapArgs {}

/// `embornal bootstrap`
///
/// The whole text: every tool, in the order that a reader meets them.
pub fn run(_args: BootstrapArgs, out: &mut impl Write) -> Result<()> {
    section(MEMORY, out)?;
    writeln!(out).map_err(write_error)?;
    section(CODE, out)
}

/// Writes one section, with exactly one newline at the end of it.
pub fn section(text: &str, out: &mut impl Write) -> Result<()> {
    write!(out, "{}", text.trim_end()).map_err(write_error)?;
    writeln!(out).map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut buffer = Vec::new();
        run(BootstrapArgs {}, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    fn part(source: &str) -> String {
        let mut buffer = Vec::new();
        section(source, &mut buffer).unwrap();
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
        assert!(
            text()
                .contains("Subagents should never update memories. Leave that for the main agent")
        );
    }

    #[test]
    fn the_bootstrap_stays_short() {
        // The text is instructions, not a manual. The code section is the
        // longer of the two because it teaches a loop and a standard for
        // writing, and the worked example is what makes that standard land.
        for (name, source, limit) in [("memory", MEMORY, 45), ("code", CODE, 60)] {
            let lines = part(source).lines().count();
            assert!(lines < limit, "the {name} section grew to {lines} lines");
        }
        let lines = text().lines().count();
        assert!(lines < 110, "the bootstrap grew to {lines} lines");
    }

    #[test]
    fn the_whole_text_holds_one_section_for_each_tool() {
        let bootstrap = text();
        let headings: Vec<&str> = bootstrap
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect();
        assert_eq!(headings, vec!["## Memory", "## Code"]);
    }

    #[test]
    fn each_tool_can_be_asked_for_on_its_own() {
        assert!(part(MEMORY).starts_with("## Memory\n"));
        assert!(!part(MEMORY).contains("## Code"));

        assert!(part(CODE).starts_with("## Code\n"));
        assert!(!part(CODE).contains("## Memory"));
    }

    #[test]
    fn the_code_section_names_each_command_of_the_loop() {
        let code = part(CODE);
        for command in [
            "embornal code index",
            "embornal code next --json",
            "embornal code describe --stdin",
            "embornal code recall <query>",
            "embornal code cat <name>",
        ] {
            assert!(code.contains(command), "{command} is missing");
        }
    }

    #[test]
    fn the_code_section_asks_for_a_description_that_stands_on_its_own() {
        let code = part(CODE);
        assert!(code.contains("140 characters"));
        assert!(code.contains("stands on its own"));
        assert!(code.contains("has not opened the file"));
        assert!(code.contains("Write in English."));

        // A description must say what the node belongs to, where it is used
        // and when to reach for it. A description of the line alone is the
        // failure that this text exists to prevent.
        for demand in [
            "What it belongs to",
            "Where it is used",
            "When to reach for it",
            "concrete facts",
        ] {
            assert!(code.contains(demand), "the text does not ask for: {demand}");
        }
    }

    #[test]
    fn the_code_section_shows_a_bad_description_beside_a_good_one() {
        // The rule is abstract and the example is not, so the example is the
        // part that a reader copies. Both halves must survive an edit.
        let code = part(CODE);
        assert!(code.contains("Do not write:"));
        assert!(code.contains("function that adds nodes to a reusable linked list"));
        assert!(code.contains("part of the generic linked list type"));
        assert!(code.contains("`users.rs`") && code.contains("`process.rs`"));
        assert!(code.contains("Use this type when"));
    }

    #[test]
    fn the_code_section_says_who_finds_the_call_sites() {
        // The index keeps no call graph, so a description that names where
        // code is used can only come from an agent that searched for it.
        let code = part(CODE);
        assert!(code.contains("The index does not work"));
        assert!(code.contains("Never guess where code is used. Search for it."));
    }

    #[test]
    fn the_code_section_says_that_the_payload_holds_no_source() {
        assert!(part(CODE).contains("The payload holds no source."));
    }

    #[test]
    fn the_bootstrap_ends_with_one_newline() {
        let bootstrap = text();
        assert!(bootstrap.ends_with('\n'));
        assert!(!bootstrap.ends_with("\n\n"));

        for source in [MEMORY, CODE] {
            let section = part(source);
            assert!(section.ends_with('\n'));
            assert!(!section.ends_with("\n\n"));
        }
    }
}
