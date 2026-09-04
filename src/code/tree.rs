//! The tree of one file, and the hashes of its nodes.
//!
//! What holds what comes from the spans and from nothing else: a definition
//! whose bytes sit inside the bytes of another is its child. Every grammar
//! agrees on that, so no language needs a rule of its own here.

use crate::code::lang::Language;
use crate::code::node::{ContentHash, NodeKind, PoolKey};
use crate::code::parse::Parsed;
use std::collections::HashMap;

/// The separator between the parts of a qualified name.
pub const SEPARATOR: &str = "::";

/// One node, before it reaches the database.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// The place of the parent in the list, or `None` for the top of it.
    pub parent: Option<usize>,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub content_hash: ContentHash,
    pub pool_key: PoolKey,
}

/// Builds the nodes of one file.
///
/// The first node is the file itself, and every definition below it follows,
/// each after the node that holds it.
///
/// A file that the grammar could not read gives one node and no child. See
/// [`Parsed::parse_errors`].
pub fn build_file(rel_path: &str, source: &[u8], parsed: &Parsed) -> Vec<Built> {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path).to_string();

    // The hash of a file is the hash of its bytes, and that already moves when
    // any definition inside it moves. A directory is the only node that must
    // hash its children, because it holds no bytes of its own.
    let file_hash = ContentHash::of_bytes(source);
    let mut nodes = vec![Built {
        parent: None,
        kind: NodeKind::File,
        name,
        qualified_name: rel_path.to_string(),
        start_line: Some(1),
        end_line: Some(line_count(source)),
        pool_key: PoolKey::new(rel_path, &file_hash),
        content_hash: file_hash,
    }];

    if parsed.parse_errors {
        return nodes;
    }

    // The stack holds the nodes that are still open, with the byte at which
    // each of them ends. A definition belongs to the innermost one that has
    // not ended yet.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();

    for definition in &parsed.definitions {
        while let Some(&(_, end)) = open.last() {
            if end <= definition.start_byte {
                open.pop();
            } else {
                break;
            }
        }

        let parent = open.last().map(|&(at, _)| at).unwrap_or(0);
        let qualified_name = unique(
            format!(
                "{}{SEPARATOR}{}",
                nodes[parent].qualified_name, definition.name
            ),
            &mut taken,
        );

        let hash = ContentHash::of_bytes(&source[definition.start_byte..definition.end_byte]);
        nodes.push(Built {
            parent: Some(parent),
            kind: definition.kind,
            name: definition.name.clone(),
            pool_key: PoolKey::new(&qualified_name, &hash),
            qualified_name,
            start_line: Some(definition.start_line),
            end_line: Some(definition.end_line),
            content_hash: hash,
        });
        open.push((nodes.len() - 1, definition.end_byte));
    }

    nodes
}

/// Builds the node of a directory or of the root.
///
/// These are the only nodes that hash their children: they hold no bytes of
/// their own. The caller gives the hashes in a fixed order, which it takes
/// from the names of the children, so that one tree hashes the same way on
/// every machine.
pub fn build_directory<'a>(
    rel_path: &str,
    kind: NodeKind,
    children: impl IntoIterator<Item = &'a ContentHash>,
) -> Built {
    let hash = ContentHash::of_children(children);
    Built {
        parent: None,
        kind,
        name: rel_path.rsplit('/').next().unwrap_or(rel_path).to_string(),
        qualified_name: rel_path.to_string(),
        start_line: None,
        end_line: None,
        pool_key: PoolKey::new(rel_path, &hash),
        content_hash: hash,
    }
}

/// Keeps two nodes of one file from answering to the same name.
///
/// A language can hold two definitions that the index cannot tell apart: two
/// overloads in TypeScript, a name that a grammar reports twice. The name of a
/// node must be unique inside its collection, so the second one and every one
/// after it carry a number.
fn unique(name: String, taken: &mut HashMap<String, usize>) -> String {
    match taken.get_mut(&name) {
        Some(count) => {
            *count += 1;
            format!("{name}#{count}")
        }
        None => {
            taken.insert(name.clone(), 1);
            name
        }
    }
}

/// The number of the last line of a file.
fn line_count(source: &[u8]) -> u32 {
    let lines = source.iter().filter(|byte| **byte == b'\n').count();
    // A file that does not end with a newline still holds a last line.
    let trailing = usize::from(!source.is_empty() && !source.ends_with(b"\n"));
    (lines + trailing).max(1) as u32
}

/// The name that a file of this language answers to, for a caller that has
/// only the path.
pub fn language_of(rel_path: &str) -> Option<Language> {
    Language::of_file(std::path::Path::new(rel_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::parse::parse;

    fn build(rel_path: &str, source: &str) -> Vec<Built> {
        let language = language_of(rel_path).unwrap();
        let parsed = parse(source.as_bytes(), language).unwrap();
        build_file(rel_path, source.as_bytes(), &parsed)
    }

    /// The tree as `qualified_name` lines, each indented by its depth.
    fn shape(nodes: &[Built]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                let mut depth = 0;
                let mut at = node.parent;
                while let Some(parent) = at {
                    depth += 1;
                    at = nodes[parent].parent;
                }
                format!("{}{}", "  ".repeat(depth), node.qualified_name)
            })
            .collect()
    }

    #[test]
    fn a_definition_inside_another_becomes_its_child() {
        let nodes = build(
            "src/a.rs",
            "mod inner {\n    struct M;\n    impl M {\n        fn open() {}\n    }\n}\nfn free() {}\n",
        );
        assert_eq!(
            shape(&nodes),
            vec![
                "src/a.rs",
                "  src/a.rs::inner",
                "    src/a.rs::inner::M",
                "    src/a.rs::inner::M#2",
                "      src/a.rs::inner::M#2::open",
                "  src/a.rs::free",
            ]
        );
    }

    #[test]
    fn a_definition_that_follows_another_is_its_sibling() {
        let nodes = build("src/a.rs", "fn one() {}\nfn two() {}\n");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[1].parent, Some(0));
        assert_eq!(nodes[2].parent, Some(0));
    }

    #[test]
    fn two_definitions_of_one_name_are_told_apart() {
        // `struct M` and `impl M` both want `src/a.rs::M`, and a name must be
        // unique inside a collection.
        let nodes = build("src/a.rs", "struct M;\nimpl M {}\n");
        let names: Vec<&str> = nodes
            .iter()
            .map(|node| node.qualified_name.as_str())
            .collect();
        assert_eq!(names, vec!["src/a.rs", "src/a.rs::M", "src/a.rs::M#2"]);
    }

    #[test]
    fn the_file_hashes_its_own_bytes() {
        let nodes = build("src/a.rs", "fn one() {}\n");
        assert_eq!(
            nodes[0].content_hash,
            ContentHash::of_bytes(b"fn one() {}\n")
        );
    }

    #[test]
    fn a_node_hashes_the_bytes_of_its_own_span() {
        let nodes = build("src/a.rs", "fn one() {}\n");
        assert_eq!(nodes[1].content_hash, ContentHash::of_bytes(b"fn one() {}"));
    }

    #[test]
    fn editing_one_function_leaves_its_siblings_alone() {
        let before = build("src/a.rs", "fn one() { 1 }\nfn two() { 2 }\n");
        let after = build("src/a.rs", "fn one() { 111 }\nfn two() { 2 }\n");

        // The file moved, because its bytes moved.
        assert_ne!(before[0].content_hash, after[0].content_hash);
        // The function that changed moved with it.
        assert_ne!(before[1].content_hash, after[1].content_hash);
        // Its sibling did not, so its summary stays in the pool.
        assert_eq!(before[2].content_hash, after[2].content_hash);
        assert_eq!(before[2].pool_key, after[2].pool_key);
    }

    #[test]
    fn a_function_that_moves_to_another_file_takes_no_summary_with_it() {
        // The pool key holds the qualified name, and the path is part of it.
        let here = build("src/a.rs", "fn one() { 1 }\n");
        let there = build("src/b.rs", "fn one() { 1 }\n");
        assert_eq!(here[1].content_hash, there[1].content_hash);
        assert_ne!(here[1].pool_key, there[1].pool_key);
    }

    #[test]
    fn a_file_that_the_grammar_could_not_read_holds_no_child() {
        let source = "fn a() {\n<<<<<<< HEAD\n=======\n>>>>>>> b\n}\n";
        let parsed = parse(source.as_bytes(), Language::Rust).unwrap();
        assert!(parsed.parse_errors);

        let nodes = build_file("src/a.rs", source.as_bytes(), &parsed);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, NodeKind::File);
        // It still hashes, so a later edit that fixes the file reopens it.
        assert_eq!(
            nodes[0].content_hash,
            ContentHash::of_bytes(source.as_bytes())
        );
    }

    #[test]
    fn an_empty_file_is_one_node() {
        let nodes = build("src/a.rs", "");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].end_line, Some(1));
    }

    #[test]
    fn the_last_line_counts_even_without_a_newline_to_close_it() {
        assert_eq!(build("src/a.rs", "fn a() {}")[0].end_line, Some(1));
        assert_eq!(build("src/a.rs", "fn a() {}\n")[0].end_line, Some(1));
        assert_eq!(build("src/a.rs", "\nfn a() {}")[0].end_line, Some(2));
    }

    #[test]
    fn a_directory_follows_the_files_that_it_holds() {
        let one = ContentHash::of_bytes(b"one");
        let two = ContentHash::of_bytes(b"two");
        let before = build_directory("src", NodeKind::Dir, [&one, &two]);

        assert_eq!(before.name, "src");
        assert_eq!(before.start_line, None);
        assert_eq!(
            before.content_hash,
            build_directory("src", NodeKind::Dir, [&one, &two]).content_hash
        );

        let other = ContentHash::of_bytes(b"other");
        let after = build_directory("src", NodeKind::Dir, [&one, &other]);
        assert_ne!(before.content_hash, after.content_hash);
    }

    #[test]
    fn a_directory_that_holds_the_same_files_under_another_path_is_another_node() {
        let one = ContentHash::of_bytes(b"one");
        let here = build_directory("src", NodeKind::Dir, [&one]);
        let there = build_directory("tests", NodeKind::Dir, [&one]);
        assert_eq!(here.content_hash, there.content_hash);
        assert_ne!(here.pool_key, there.pool_key);
    }

    #[test]
    fn the_tree_of_a_file_holds_together_in_python_as_well() {
        let nodes = build(
            "app/store.py",
            "class Store:\n    def open(self):\n        def inner():\n            pass\n",
        );
        assert_eq!(
            shape(&nodes),
            vec![
                "app/store.py",
                "  app/store.py::Store",
                "    app/store.py::Store::open",
                "      app/store.py::Store::open::inner",
            ]
        );
    }
}
