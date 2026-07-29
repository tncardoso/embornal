//! Wiki links.
//!
//! The content of a fact can point at another path in the `[[/path]]` form.
//! The memory stores the text as it comes and reads the links when it shows
//! the fact, so a link that points nowhere costs nothing until somebody
//! follows it.

use crate::memory::path::WikiPath;

/// A piece of the content of a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Plain text.
    Text(&'a str),
    /// A link that reads as a path.
    Link {
        /// The path that the link points at.
        target: WikiPath,
        /// What the reader sees. This is the text inside the brackets.
        label: &'a str,
    },
    /// Something in brackets that is not a path. The memory shows it as it is
    /// written, because a writer must be able to write `[[TODO]]`.
    Broken(&'a str),
}

/// Cuts the content into text and links.
pub fn parse(content: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = content;

    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("]]") else {
            break;
        };
        let inner = &after_open[..close];

        if !inner.is_empty() {
            if open > 0 {
                segments.push(Segment::Text(&rest[..open]));
            }
            match WikiPath::parse(inner) {
                Ok(target) => segments.push(Segment::Link {
                    target,
                    label: inner,
                }),
                Err(_) => segments.push(Segment::Broken(inner)),
            }
            rest = &after_open[close + 2..];
        } else {
            // An empty pair of brackets is plain text.
            let cut = open + 4;
            segments.push(Segment::Text(&rest[..cut]));
            rest = &rest[cut..];
        }
    }

    if !rest.is_empty() {
        segments.push(Segment::Text(rest));
    }
    segments
}

/// Returns each path that the content points at, in the order of the text and
/// with no repeats.
pub fn targets(content: &str) -> Vec<WikiPath> {
    let mut found: Vec<WikiPath> = Vec::new();
    for segment in parse(content) {
        if let Segment::Link { target, .. } = segment
            && !found.contains(&target)
        {
            found.push(target);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> WikiPath {
        WikiPath::parse(s).unwrap()
    }

    #[test]
    fn reads_content_with_no_link() {
        assert_eq!(parse("plain text"), vec![Segment::Text("plain text")]);
        assert!(targets("plain text").is_empty());
    }

    #[test]
    fn cuts_a_link_out_of_the_text() {
        let segments = parse("see [[/projects/embornal]] for more");
        assert_eq!(
            segments,
            vec![
                Segment::Text("see "),
                Segment::Link {
                    target: path("/projects/embornal"),
                    label: "/projects/embornal"
                },
                Segment::Text(" for more"),
            ]
        );
    }

    #[test]
    fn reads_a_link_that_starts_the_content() {
        let segments = parse("[[/a]] holds the answer");
        assert_eq!(
            segments,
            vec![
                Segment::Link {
                    target: path("/a"),
                    label: "/a"
                },
                Segment::Text(" holds the answer"),
            ]
        );
    }

    #[test]
    fn reads_more_than_one_link() {
        assert_eq!(
            targets("[[/a]] and [[/b]] and [[/a]] again"),
            vec![path("/a"), path("/b")]
        );
    }

    #[test]
    fn folds_the_target_but_keeps_the_label() {
        let segments = parse("[[/Projects/Embornal]]");
        assert_eq!(
            segments,
            vec![Segment::Link {
                target: path("/projects/embornal"),
                label: "/Projects/Embornal"
            }]
        );
    }

    #[test]
    fn keeps_something_that_is_not_a_path() {
        assert_eq!(parse("[[TODO]]"), vec![Segment::Broken("TODO")]);
        assert!(targets("[[TODO]]").is_empty());
    }

    #[test]
    fn an_open_bracket_with_no_close_is_text() {
        assert_eq!(parse("[[/a"), vec![Segment::Text("[[/a")]);
        assert_eq!(
            parse("text [[/a and more"),
            vec![Segment::Text("text [[/a and more")]
        );
    }

    #[test]
    fn empty_brackets_are_text() {
        assert_eq!(parse("[[]]"), vec![Segment::Text("[[]]")]);
    }

    #[test]
    fn the_pieces_rebuild_the_content() {
        let content = "a [[/x]] b [[TODO]] c";
        let rebuilt: String = parse(content)
            .iter()
            .map(|segment| match segment {
                Segment::Text(text) => (*text).to_string(),
                Segment::Link { label, .. } => format!("[[{label}]]"),
                Segment::Broken(text) => format!("[[{text}]]"),
            })
            .collect();
        assert_eq!(rebuilt, content);
    }
}
