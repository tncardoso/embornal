//! The languages that the index reads.
//!
//! Each language brings a grammar and a query. The query lists definitions
//! and nothing else: no reference, no scope. What holds what comes from the
//! spans, in [`super::tree`], and that works the same way in every language
//! here.

use crate::code::node::NodeKind;
use std::fmt;
use std::path::Path;

/// A language that this build reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Tsx,
}

/// Every language, for the walk and for the tests.
pub const ALL: [Language; 6] = [
    Language::Rust,
    Language::Python,
    Language::Go,
    Language::JavaScript,
    Language::TypeScript,
    Language::Tsx,
];

impl Language {
    /// Reads the extension of a file.
    ///
    /// A file with no extension, or with one that no grammar here reads, gives
    /// `None`, and the walk passes over it.
    pub fn of_file(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Some(match extension {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "go" => Self::Go,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            _ => return None,
        })
    }

    /// The name that goes in the `language` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }

    /// Reads the name back.
    pub fn parse(text: &str) -> Option<Self> {
        ALL.into_iter().find(|language| language.as_str() == text)
    }

    /// The grammar itself.
    pub fn grammar(&self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    /// The query that lists the definitions of this language.
    ///
    /// TSX reads the query of TypeScript: the two grammars differ in what they
    /// accept, and not in what a definition looks like.
    pub fn query(&self) -> &'static str {
        match self {
            Self::Rust => include_str!("queries/rust.scm"),
            Self::Python => include_str!("queries/python.scm"),
            Self::Go => include_str!("queries/go.scm"),
            Self::JavaScript => include_str!("queries/javascript.scm"),
            Self::TypeScript | Self::Tsx => include_str!("queries/typescript.scm"),
        }
    }

    /// Joins a name with the extra word that tells it from another of the same
    /// name.
    ///
    /// Two languages need this, for the same reason and in different words. In
    /// Rust, `impl Memory` and `impl Display for Memory` are two blocks over
    /// one type. In Go, two types of one package can both hold a method called
    /// `String`.
    pub fn compose(&self, name: &str, context: &str) -> String {
        match self {
            Self::Rust => format!("{name} as {context}"),
            Self::Go => format!("{context}.{name}"),
            _ => name.to_string(),
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reads the name that a capture of a query carries.
///
/// The queries share one small vocabulary, so that the code that walks them
/// knows nothing about any single language.
pub fn capture_kind(capture: &str) -> Option<NodeKind> {
    Some(match capture {
        "definition.module" => NodeKind::Module,
        "definition.class" => NodeKind::Class,
        "definition.impl" => NodeKind::Impl,
        "definition.function" => NodeKind::Function,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_extension_reaches_its_grammar() {
        for (file, language) in [
            ("src/main.rs", Language::Rust),
            ("a/b.py", Language::Python),
            ("stub.pyi", Language::Python),
            ("cmd/main.go", Language::Go),
            ("web/app.js", Language::JavaScript),
            ("web/app.mjs", Language::JavaScript),
            ("web/view.jsx", Language::JavaScript),
            ("web/app.ts", Language::TypeScript),
            ("web/view.tsx", Language::Tsx),
        ] {
            assert_eq!(Language::of_file(Path::new(file)), Some(language), "{file}");
        }
    }

    #[test]
    fn a_file_that_no_grammar_reads_gives_nothing() {
        for file in ["README.md", "Makefile", "a.tar.gz", ".gitignore"] {
            assert_eq!(Language::of_file(Path::new(file)), None, "{file}");
        }
    }

    #[test]
    fn each_name_survives_a_trip_through_its_word() {
        for language in ALL {
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
        assert_eq!(Language::parse("cobol"), None);
    }

    #[test]
    fn every_grammar_loads_and_every_query_compiles() {
        // The grammars are C, and a query is text. Neither is checked when
        // this crate builds, so the check belongs here.
        for language in ALL {
            let grammar = language.grammar();
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&grammar)
                .unwrap_or_else(|err| panic!("{language}: {err}"));
            tree_sitter::Query::new(&grammar, language.query())
                .unwrap_or_else(|err| panic!("{language}: {err}"));
        }
    }

    #[test]
    fn every_capture_of_every_query_is_a_word_that_the_index_knows() {
        for language in ALL {
            let query = tree_sitter::Query::new(&language.grammar(), language.query()).unwrap();
            for capture in query.capture_names() {
                assert!(
                    capture_kind(capture).is_some() || matches!(*capture, "name" | "context"),
                    "{language} uses the unknown capture @{capture}"
                );
            }
        }
    }

    #[test]
    fn each_language_tells_two_blocks_of_one_name_apart_in_its_own_words() {
        assert_eq!(
            Language::Rust.compose("Memory", "Display"),
            "Memory as Display"
        );
        assert_eq!(
            Language::Go.compose("String", "Database"),
            "Database.String"
        );
        // The other languages have nothing to add.
        assert_eq!(Language::Python.compose("run", "ignored"), "run");
    }
}
