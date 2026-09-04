//! The tree that the commands draw.
//!
//! The memory draws a tree of paths, and the code index draws a tree of
//! directories, files and definitions. The lines and the elbows are the same
//! in both, so the drawing lives here and each command says only what a node
//! is called.

use crate::cli::write_error;
use crate::error::Result;
use std::io::Write;

/// A node that [`print_tree`] can draw.
///
/// The top of a tree and the nodes below it are named differently: the top
/// carries its whole path, because nothing above it says where it sits, and a
/// node below it carries its own name alone.
pub trait Branch {
    /// What the top line shows.
    fn root_label(&self) -> String;

    /// What a line below the top shows.
    fn label(&self) -> String;

    /// A mark that follows the name, such as `*` for a path that holds facts
    /// of its own. Most nodes carry nothing.
    fn mark(&self) -> &'static str {
        ""
    }

    fn children(&self) -> &[Self]
    where
        Self: Sized;
}

/// Prints a tree.
pub fn print_tree<T: Branch>(tree: &T, out: &mut impl Write) -> Result<()> {
    writeln!(out, "{}{}", tree.root_label(), tree.mark()).map_err(write_error)?;
    print_branches(tree, "", out)
}

/// Writes the nodes below one node.
fn print_branches<T: Branch>(node: &T, prefix: &str, out: &mut impl Write) -> Result<()> {
    let children = node.children();
    let last = children.len().saturating_sub(1);
    for (index, child) in children.iter().enumerate() {
        let (elbow, next) = if index == last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };

        writeln!(out, "{prefix}{elbow}{}{}", child.label(), child.mark()).map_err(write_error)?;
        print_branches(child, &format!("{prefix}{next}"), out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Node {
        name: &'static str,
        starred: bool,
        children: Vec<Node>,
    }

    impl Branch for Node {
        fn root_label(&self) -> String {
            format!("<{}>", self.name)
        }
        fn label(&self) -> String {
            self.name.to_string()
        }
        fn mark(&self) -> &'static str {
            if self.starred { "*" } else { "" }
        }
        fn children(&self) -> &[Self] {
            &self.children
        }
    }

    fn node(name: &'static str, starred: bool, children: Vec<Node>) -> Node {
        Node {
            name,
            starred,
            children,
        }
    }

    fn text(tree: &Node) -> String {
        let mut buffer = Vec::new();
        print_tree(tree, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn the_top_is_named_apart_from_the_nodes_below_it() {
        let tree = node("a", false, vec![node("b", false, vec![])]);
        assert_eq!(text(&tree), "<a>\n└── b\n");
    }

    #[test]
    fn the_last_child_closes_its_level() {
        let tree = node(
            "top",
            false,
            vec![
                node("one", true, vec![node("deep", false, vec![])]),
                node("two", false, vec![]),
            ],
        );
        assert_eq!(
            text(&tree),
            "<top>\n\
             ├── one*\n\
             │   └── deep\n\
             └── two\n"
        );
    }

    #[test]
    fn a_tree_of_one_node_holds_one_line() {
        assert_eq!(text(&node("alone", true, vec![])), "<alone>*\n");
    }
}
