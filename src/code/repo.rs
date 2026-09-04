//! Which repository a command works on.
//!
//! The agent runs `embornal code` from somewhere inside a project, and the
//! index must find the top of that project by itself. It walks up from the
//! working directory until it finds a `.git`.
//!
//! The walk stops before the home directory of the user. Some people keep
//! their dotfiles in a repository, so a `.git` sits in the home directory
//! itself; taking that as a root would index everything that the user owns.
//! When the walk ends with nothing, the working directory is the root, which
//! also covers a project that git does not track.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// Finds the root of the repository that holds `start`.
///
/// The answer is a canonical path, because it names a collection: two ways of
/// writing the same directory must reach the same index.
pub fn discover(start: &Path) -> Result<PathBuf> {
    let home = dirs::home_dir();
    discover_below(start, home.as_deref())
}

/// The walk itself, with the home directory given.
///
/// The tests call this, because a test cannot move the home of the machine
/// that runs it.
pub fn discover_below(start: &Path, home: Option<&Path>) -> Result<PathBuf> {
    let start = canonical(start)?;
    let home = home.map(canonical).transpose().unwrap_or(None);

    let mut candidate: &Path = &start;
    loop {
        // The home directory ends the walk without answering. What lies above
        // it belongs to no project of this user.
        if home.as_deref() == Some(candidate) {
            break;
        }
        if candidate.join(".git").exists() {
            return Ok(candidate.to_path_buf());
        }
        match candidate.parent() {
            Some(parent) => candidate = parent,
            None => break,
        }
    }
    Ok(start)
}

/// Resolves a path, following the links that lead to it.
fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The name that a repository gives to its collection.
///
/// A repository has one index and nobody must name it, so the name is the
/// path of the root. `--collection` gives another name over the same root,
/// which is a fork.
pub fn default_collection(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a directory that no other test touches.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("embornal-repo-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn the_walk_stops_at_the_first_git_above_the_working_directory() {
        let home = scratch("finds");
        let repo = home.join("projects/thing");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let deep = repo.join("src/memory");
        std::fs::create_dir_all(&deep).unwrap();

        assert_eq!(discover_below(&deep, Some(&home)).unwrap(), repo);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_directory_that_is_itself_the_root_answers_with_itself() {
        let home = scratch("itself");
        let repo = home.join("thing");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        assert_eq!(discover_below(&repo, Some(&home)).unwrap(), repo);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn with_no_git_anywhere_the_working_directory_is_the_root() {
        let home = scratch("nogit");
        let deep = home.join("notes/drafts");
        std::fs::create_dir_all(&deep).unwrap();

        assert_eq!(discover_below(&deep, Some(&home)).unwrap(), deep);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_repository_in_the_home_directory_never_becomes_the_root() {
        // Dotfiles in git are common. Taking the home directory as a root
        // would index everything that the user owns.
        let home = scratch("dotfiles");
        std::fs::create_dir_all(home.join(".git")).unwrap();
        let deep = home.join("notes");
        std::fs::create_dir_all(&deep).unwrap();

        assert_eq!(discover_below(&deep, Some(&home)).unwrap(), deep);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_nearest_git_wins_over_one_further_up() {
        let home = scratch("nested");
        let outer = home.join("outer");
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        let inner = outer.join("vendor/inner");
        std::fs::create_dir_all(inner.join(".git")).unwrap();
        let deep = inner.join("src");
        std::fs::create_dir_all(&deep).unwrap();

        assert_eq!(discover_below(&deep, Some(&home)).unwrap(), inner);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_git_file_of_a_worktree_counts_as_a_root() {
        // `git worktree` writes a file, not a directory, and a worktree is a
        // checkout like any other.
        let home = scratch("worktree");
        let repo = home.join("thing");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: /elsewhere\n").unwrap();

        assert_eq!(discover_below(&repo, Some(&home)).unwrap(), repo);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_walk_ends_at_the_top_when_there_is_no_home() {
        let root = scratch("nohome");
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();

        // With no home to stop at, the walk reaches the top of the file
        // system and gives back where it started.
        assert_eq!(discover_below(&deep, None).unwrap(), deep);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_that_is_not_there_says_so() {
        let missing = std::env::temp_dir().join("embornal-repo-missing-entirely");
        std::fs::remove_dir_all(&missing).ok();
        assert!(discover_below(&missing, None).is_err());
    }

    #[test]
    fn the_name_of_a_collection_is_the_path_of_its_root() {
        assert_eq!(
            default_collection(Path::new("/home/a/projects/thing")),
            "/home/a/projects/thing"
        );
    }
}
