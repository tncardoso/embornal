//! Which files of a repository the index reads.
//!
//! The walk follows `.gitignore`, because what git does not track is almost
//! never what somebody asks the index about: a build directory, a cache, a
//! downloaded dependency.

use crate::code::lang::Language;
use crate::config::CodeConfig;
use std::path::{Path, PathBuf};

/// One file that the index reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where the file is.
    pub path: PathBuf,
    /// The path below the root, with `/` between the parts on every platform.
    /// This is what a node carries, so an index moves with its repository.
    pub rel_path: String,
    pub language: Language,
}

/// Lists the files of one repository, in a fixed order.
///
/// The order is the order of the paths, so that a directory hashes the same
/// way on every machine and on every run.
pub fn walk(root: &Path, config: &CodeConfig) -> Vec<Found> {
    let mut found = Vec::new();

    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .parents(false)
        .build()
        .flatten()
    {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(language) = Language::of_file(path) else {
            continue;
        };
        // A file above the limit is almost always generated or vendored, and
        // its nodes would be nodes that nobody asks about.
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > config.max_file_bytes)
        {
            continue;
        }
        let Some(rel_path) = relative(root, path) else {
            continue;
        };
        found.push(Found {
            path: path.to_path_buf(),
            rel_path,
            language,
        });
    }

    found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    found
}

/// Writes the path below the root with `/` between the parts.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let mut text = String::new();
    for part in rest.components() {
        if !text.is_empty() {
            text.push('/');
        }
        text.push_str(part.as_os_str().to_str()?);
    }
    (!text.is_empty()).then_some(text)
}

/// The directories that hold a file, from the top down.
///
/// `src/memory/api.rs` gives `src` and `src/memory`. The root is not in the
/// list: it is the collection itself.
pub fn ancestors(rel_path: &str) -> Vec<String> {
    let mut parts: Vec<&str> = rel_path.split('/').collect();
    parts.pop();

    let mut out = Vec::with_capacity(parts.len());
    let mut path = String::new();
    for part in parts {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        out.push(path.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("embornal-walk-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn paths(root: &Path, config: &CodeConfig) -> Vec<String> {
        walk(root, config)
            .into_iter()
            .map(|found| found.rel_path)
            .collect()
    }

    #[test]
    fn the_walk_gives_the_files_that_a_grammar_reads_in_a_fixed_order() {
        let root = scratch("order");
        write(&root, "src/b.rs", "fn b() {}");
        write(&root, "src/a.rs", "fn a() {}");
        write(&root, "README.md", "# a repository");
        write(&root, "app/main.py", "def a(): pass");

        assert_eq!(
            paths(&root, &CodeConfig::default()),
            vec!["app/main.py", "src/a.rs", "src/b.rs"]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_walk_follows_gitignore() {
        let root = scratch("ignored");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write(&root, ".gitignore", "target/\ngenerated.rs\n");
        write(&root, "src/a.rs", "fn a() {}");
        write(&root, "src/generated.rs", "fn g() {}");
        write(&root, "target/debug/build.rs", "fn b() {}");

        assert_eq!(paths(&root, &CodeConfig::default()), vec!["src/a.rs"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_hidden_directory_stays_out() {
        let root = scratch("hidden");
        write(&root, "src/a.rs", "fn a() {}");
        write(&root, ".cache/b.rs", "fn b() {}");

        assert_eq!(paths(&root, &CodeConfig::default()), vec!["src/a.rs"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_above_the_limit_stays_out() {
        let root = scratch("large");
        write(&root, "src/small.rs", "fn a() {}");
        write(&root, "src/large.rs", &"// padding\n".repeat(200));

        let config = CodeConfig {
            max_file_bytes: 100,
            ..CodeConfig::default()
        };
        assert_eq!(paths(&root, &config), vec!["src/small.rs"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_walk_says_which_grammar_reads_each_file() {
        let root = scratch("languages");
        write(&root, "a.rs", "");
        write(&root, "b.py", "");
        write(&root, "c.tsx", "");

        let languages: Vec<Language> = walk(&root, &CodeConfig::default())
            .into_iter()
            .map(|found| found.language)
            .collect();
        assert_eq!(
            languages,
            vec![Language::Rust, Language::Python, Language::Tsx]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_repository_gives_no_file() {
        let root = scratch("empty");
        assert!(paths(&root, &CodeConfig::default()).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_names_the_directories_that_hold_it() {
        assert_eq!(
            ancestors("src/memory/api.rs"),
            vec!["src".to_string(), "src/memory".to_string()]
        );
        // A file at the top of the repository sits under no directory.
        assert!(ancestors("main.rs").is_empty());
    }
}
