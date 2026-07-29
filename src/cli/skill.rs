//! The `embornal skill` command.
//!
//! The command writes the instructions that teach an agent to use the memory.
//! The text goes to standard output, so it drops into a skill file:
//!
//! ```sh
//! embornal skill > .claude/skills/memory/SKILL.md
//! ```

use crate::cli::write_error;
use crate::error::Result;
use clap::Args;
use std::io::Write;

/// The instructions, as they go into a skill file.
pub const SKILL: &str = r#"---
name: memory
description: Read and write your long term memory. Use at startup, when you start a task, and whenever you learn something worth keeping.
---

# Memory

Your memory is provided by `$HOME/.embornal/embornal`.

## At startup

1. Run `embornal memory cat /memory` to fetch relevant information.
2. Then run `embornal memory cat /` to fetch root memories.

## While working

Register new memories with `embornal memory store [PATH] [FACT]`.

Navigate paths with `embornal memory ls [PATH]` or `embornal memory tree [PATH]`.

Store atomic, short facts, whenever you learn something new, or something
worth keeping happens. That covers a task worth real effort, a fact or insight
the user teaches you, anything you learn about their life (even indirectly),
any event of lasting effect.

Avoid redundant memories.

## When starting a new task

Recall relevant memories with `embornal memory recall [SEARCH TERMS]`.

## If you are a subagent

Skip everything above. Let updates happen in the main agent.
"#;

#[derive(Debug, Args)]
pub struct SkillArgs {}

/// `embornal skill`
pub fn run(_args: SkillArgs, out: &mut impl Write) -> Result<()> {
    write!(out, "{SKILL}").map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut buffer = Vec::new();
        run(SkillArgs {}, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn the_skill_carries_a_front_matter_that_names_it() {
        let skill = text();
        assert!(skill.starts_with("---\nname: memory\n"));
        assert_eq!(skill.matches("---").count(), 2);
    }

    #[test]
    fn the_skill_names_each_command_that_it_asks_for() {
        let skill = text();
        for command in [
            "embornal memory cat /memory",
            "embornal memory cat /",
            "embornal memory store [PATH] [FACT]",
            "embornal memory ls [PATH]",
            "embornal memory tree [PATH]",
            "embornal memory recall [SEARCH TERMS]",
        ] {
            assert!(skill.contains(command), "{command} is missing");
        }
    }

    #[test]
    fn the_skill_holds_the_four_moments() {
        let skill = text();
        for heading in [
            "## At startup",
            "## While working",
            "## When starting a new task",
            "## If you are a subagent",
        ] {
            assert!(skill.contains(heading), "{heading} is missing");
        }
    }

    #[test]
    fn the_skill_tells_a_subagent_to_stand_back() {
        assert!(text().contains("Let updates happen in the main agent."));
    }

    #[test]
    fn the_skill_stays_short() {
        // The text is instructions, not a manual. A reader must take it in at
        // one glance.
        let lines = text().lines().count();
        assert!(lines < 60, "the skill grew to {lines} lines");
    }

    #[test]
    fn the_skill_ends_with_one_newline() {
        let skill = text();
        assert!(skill.ends_with('\n'));
        assert!(!skill.ends_with("\n\n"));
    }
}
