//! What tree-sitter finds in one file.
//!
//! The result is a flat list of definitions with their spans. Nothing here
//! says what holds what: [`super::tree`] reads that off the spans, which works
//! the same way in every language and asks no grammar for a scope.

use crate::code::lang::{Language, capture_kind};
use crate::code::node::NodeKind;
use crate::error::Result;
use std::collections::HashMap;
use tree_sitter::StreamingIterator;

/// One definition that a grammar found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub kind: NodeKind,
    pub name: String,
    pub start_byte: usize,
    pub end_byte: usize,
    /// Lines count from one, the way an editor shows them.
    pub start_line: u32,
    pub end_line: u32,
}

/// What one file gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The definitions, parents before children, and each parent before the
    /// definitions that come after it in the file.
    pub definitions: Vec<Definition>,
    /// Whether the grammar failed somewhere in the file.
    ///
    /// A file that carries this gives up no definition at all. A grammar that
    /// stumbles still answers, with a shape that follows the part it could
    /// read, and an index that took that would claim a structure that is not
    /// there. Saying "this file, and nothing inside it" is the honest answer.
    pub parse_errors: bool,
}

/// Reads one file.
pub fn parse(source: &[u8], language: Language) -> Result<Parsed> {
    let grammar = language.grammar();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|err| crate::error::Error::UnsupportedLanguage(format!("{language}: {err}")))?;

    let Some(tree) = parser.parse(source, None) else {
        return Ok(Parsed {
            definitions: Vec::new(),
            parse_errors: true,
        });
    };
    if tree.root_node().has_error() {
        return Ok(Parsed {
            definitions: Vec::new(),
            parse_errors: true,
        });
    }

    let query = tree_sitter::Query::new(&grammar, language.query())
        .map_err(|err| crate::error::Error::UnsupportedLanguage(format!("{language}: {err}")))?;
    let names = query.capture_names();

    // Two patterns can reach one node: in Rust, an `impl` block matches both
    // with and without its trait. Keying on the node joins what the two say
    // instead of giving one definition twice.
    let mut found: HashMap<usize, Draft> = HashMap::new();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source);

    while let Some(matched) = matches.next() {
        let captures = matched.captures();

        // The definition of a match is registered before its name and its
        // context are read, so that the order in which the captures arrive
        // does not decide whether a name finds its definition.
        let mut owner = None;
        for capture in captures {
            let Some(kind) = capture_kind(names[capture.index as usize]) else {
                continue;
            };
            let node = capture.node;
            found.entry(node.id()).or_insert_with(|| Draft {
                kind,
                name: String::new(),
                context: None,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
            });
            owner = Some(node.id());
        }

        // A match with no definition capture says nothing that the index can
        // hang a name on.
        let Some(owner) = owner else { continue };
        let Some(draft) = found.get_mut(&owner) else {
            continue;
        };
        for capture in captures {
            match names[capture.index as usize] {
                "name" => draft.name = text(source, capture.node.byte_range()),
                "context" => draft.context = Some(text(source, capture.node.byte_range())),
                _ => {}
            }
        }
    }

    let mut definitions: Vec<Definition> = found
        .into_values()
        .filter(|draft| !draft.name.is_empty())
        .map(|draft| Definition {
            kind: draft.kind,
            name: match &draft.context {
                Some(context) => language.compose(&draft.name, context),
                None => draft.name,
            },
            start_byte: draft.start_byte,
            end_byte: draft.end_byte,
            start_line: draft.start_line,
            end_line: draft.end_line,
        })
        .collect();

    // A parent starts before its child and ends after it, so this order puts
    // every parent in front of what it holds. The tree relies on that.
    definitions.sort_by(|a, b| {
        a.start_byte
            .cmp(&b.start_byte)
            .then_with(|| b.end_byte.cmp(&a.end_byte))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(Parsed {
        definitions,
        parse_errors: false,
    })
}

/// One definition while its captures are still arriving.
struct Draft {
    kind: NodeKind,
    name: String,
    context: Option<String>,
    start_byte: usize,
    end_byte: usize,
    start_line: u32,
    end_line: u32,
}

fn text(source: &[u8], range: std::ops::Range<usize>) -> String {
    String::from_utf8_lossy(&source[range]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definitions as `kind name`, in the order that `parse` gives them.
    fn shape(source: &str, language: Language) -> Vec<String> {
        parse(source.as_bytes(), language)
            .unwrap()
            .definitions
            .iter()
            .map(|def| format!("{} {}", def.kind, def.name))
            .collect()
    }

    #[test]
    fn rust_gives_its_modules_types_blocks_and_functions() {
        let source = r#"
mod inner {
    pub struct Memory { field: u8 }
    pub enum Kind { One }
    pub trait Speaks { fn speak(&self); }
    impl Memory {
        pub fn open() -> Self { Self { field: 0 } }
    }
    impl Speaks for Memory {
        fn speak(&self) {}
    }
}
pub fn free() {}
"#;
        assert_eq!(
            shape(source, Language::Rust),
            vec![
                "module inner",
                "class Memory",
                "class Kind",
                "class Speaks",
                "impl Memory",
                "function open",
                "impl Memory as Speaks",
                "function speak",
                "function free",
            ]
        );
    }

    #[test]
    fn two_blocks_over_one_type_answer_to_two_names() {
        // Without the trait in the name, `impl Memory` and `impl Display for
        // Memory` would collide in one file.
        let names = shape(
            "struct M; impl M { fn a(&self) {} } impl Display for M { fn fmt(&self) {} }",
            Language::Rust,
        );
        assert!(names.contains(&"impl M".to_string()), "{names:?}");
        assert!(
            names.contains(&"impl M as Display".to_string()),
            "{names:?}"
        );
    }

    #[test]
    fn python_gives_its_classes_and_functions() {
        let source = "\
class Store:
    def open(self):
        def inner():
            pass
        return inner

def free():
    pass
";
        assert_eq!(
            shape(source, Language::Python),
            vec![
                "class Store",
                "function open",
                "function inner",
                "function free",
            ]
        );
    }

    #[test]
    fn a_decorated_python_function_still_arrives() {
        assert_eq!(
            shape("@cache\ndef slow():\n    pass\n", Language::Python),
            vec!["function slow"]
        );
    }

    #[test]
    fn go_puts_the_receiver_in_the_name_of_a_method() {
        let source = "\
package store

type Database struct{ path string }
type Client struct{ url string }

func (d *Database) String() string { return d.path }
func (c Client) String() string { return c.url }
func Open() *Database { return nil }
";
        let names = shape(source, Language::Go);
        // Two types of one package both hold a `String`, and the receiver is
        // what tells them apart.
        assert!(
            names.contains(&"function Database.String".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"function Client.String".to_string()),
            "{names:?}"
        );
        assert!(names.contains(&"function Open".to_string()), "{names:?}");
        assert!(names.contains(&"class Database".to_string()), "{names:?}");
    }

    #[test]
    fn javascript_gives_a_function_that_a_constant_holds() {
        let source = "\
class View {
  render() { return 1 }
}
function free() {}
const arrow = () => 2;
";
        assert_eq!(
            shape(source, Language::JavaScript),
            vec![
                "class View",
                "function render",
                "function free",
                "function arrow",
            ]
        );
    }

    #[test]
    fn typescript_gives_its_interfaces_and_type_aliases() {
        let source = "\
interface Shape { area(): number }
type Id = string;
export function open(): void {}
export const load = async (): Promise<void> => {};
";
        let names = shape(source, Language::TypeScript);
        assert!(names.contains(&"class Shape".to_string()), "{names:?}");
        assert!(names.contains(&"class Id".to_string()), "{names:?}");
        assert!(names.contains(&"function open".to_string()), "{names:?}");
        assert!(names.contains(&"function load".to_string()), "{names:?}");
    }

    #[test]
    fn tsx_reads_the_query_of_typescript() {
        let names = shape(
            "export const Card = (props: P) => <div>{props.title}</div>;",
            Language::Tsx,
        );
        assert_eq!(names, vec!["function Card"]);
    }

    #[test]
    fn a_parent_arrives_before_what_it_holds() {
        let parsed = parse(
            b"mod outer { fn inner() {} }\nfn after() {}",
            Language::Rust,
        )
        .unwrap();
        let spans: Vec<(usize, usize)> = parsed
            .definitions
            .iter()
            .map(|def| (def.start_byte, def.end_byte))
            .collect();
        // `outer` holds `inner`, so it comes first and ends after it.
        assert!(
            spans[0].0 <= spans[1].0 && spans[0].1 >= spans[1].1,
            "{spans:?}"
        );
        // `after` sits beside `outer`, and comes last.
        assert!(spans[2].0 >= spans[0].1, "{spans:?}");
    }

    #[test]
    fn the_lines_of_a_definition_count_from_one() {
        let parsed = parse(b"\n\nfn a() {\n}\n", Language::Rust).unwrap();
        let def = &parsed.definitions[0];
        assert_eq!((def.start_line, def.end_line), (3, 4));
    }

    #[test]
    fn a_file_that_the_grammar_cannot_read_gives_up_nothing() {
        // A merge conflict, or a dialect that this grammar does not cover.
        let parsed = parse(
            b"fn a() {\n<<<<<<< HEAD\n    one()\n=======\n    two()\n>>>>>>> other\n}\n",
            Language::Rust,
        )
        .unwrap();
        assert!(parsed.parse_errors);
        // Not a partial shape: the index never claims a structure that the
        // grammar could not read.
        assert!(parsed.definitions.is_empty());
    }

    #[test]
    fn a_declaration_with_no_body_is_not_a_definition() {
        // A summary says what code does, from what its body does. A method
        // that a trait only declares has no body, so a summary of it could
        // repeat its name and nothing more. The trait carries the contract.
        let names = shape("trait Speaks { fn speak(&self); }", Language::Rust);
        assert_eq!(names, vec!["class Speaks"]);

        let names = shape("interface Shape { area(): number }", Language::TypeScript);
        assert_eq!(names, vec!["class Shape"]);
    }

    #[test]
    fn a_method_on_a_pointer_names_the_type_that_it_belongs_to() {
        // `func (d *Database)` and `func (d Database)` are methods of one
        // type, so the star does not reach the name.
        let names = shape(
            "package a
func (d *Database) Close() {}
",
            Language::Go,
        );
        assert_eq!(names, vec!["function Database.Close"]);
    }

    #[test]
    fn an_empty_file_parses_and_holds_nothing() {
        let parsed = parse(b"", Language::Rust).unwrap();
        assert!(!parsed.parse_errors);
        assert!(parsed.definitions.is_empty());
    }

    #[test]
    fn a_name_that_is_not_ascii_survives() {
        let parsed = parse("def memória():\n    pass\n".as_bytes(), Language::Python).unwrap();
        assert_eq!(parsed.definitions[0].name, "memória");
    }
}
