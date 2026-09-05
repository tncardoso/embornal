//! The code browser.
//!
//! `embornal dashboard` shows one code index under `/code`: the tree of a
//! repository on the left, and on the right, what is known about the
//! directory or the file that a reader picked. A file also lists its own
//! definitions, and one of them sits expanded below the list.
//!
//! The frame around the page is [`crate::dashboard`], which [`crate::wiki`]
//! wears as well.

use crate::code::CodeIndex;
use crate::code::api::{self, Described, Hit, TreeNode};
use crate::dashboard::{self, Tab};
use crate::error::Error;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The code index and the collection that the handlers read.
///
/// One index can hold more than one collection, but one running dashboard
/// always shows one of them: the one that `embornal dashboard` resolved from
/// `--path` and `--collection` when it started.
#[derive(Clone)]
pub struct CodeState {
    pub index: Arc<Mutex<CodeIndex>>,
    pub collection: Arc<String>,
}

/// Builds the routes, all below `/code`.
pub fn router(state: CodeState) -> Router {
    Router::new()
        .route("/code", get(browse_root))
        .route("/code/search", get(search))
        .route("/code/{*rel_path}", get(browse))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct BrowseQuery {
    def: Option<String>,
}

async fn browse_root(State(state): State<CodeState>, Query(query): Query<BrowseQuery>) -> Response {
    render_browse(&state, "", query.def.as_deref())
}

async fn browse(
    State(state): State<CodeState>,
    Path(rel_path): Path<String>,
    Query(query): Query<BrowseQuery>,
) -> Response {
    render_browse(&state, &rel_path, query.def.as_deref())
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(State(state): State<CodeState>, Query(query): Query<SearchQuery>) -> Response {
    let mut index = state
        .index
        .lock()
        .expect("the code index lock is never poisoned");
    let collection = state.collection.as_str();

    let (hits, elapsed) = if query.q.trim().is_empty() {
        (Vec::new(), None)
    } else {
        let start = Instant::now();
        match index.recall(collection, &query.q, None, None) {
            Ok(hits) => (hits, Some(start.elapsed())),
            Err(Error::NoSuchCollection(_)) => return not_indexed_page(collection),
            Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Code,
                    &err.to_string(),
                );
            }
        }
    };

    let active = !query.q.trim().is_empty();
    let header = search_header(collection, &query.q, &hits, elapsed);
    let bar = dashboard::search_bar(
        "/code/search",
        &query.q,
        "recall — search code by name or meaning",
        active,
        if active { "esc" } else { "⌘K" },
        active.then_some("/code"),
    );

    let results = if query.q.trim().is_empty() {
        "<p class=\"empty-row\">Type a word, or a whole sentence, to search the code.</p>"
            .to_string()
    } else if hits.is_empty() {
        "<p class=\"empty-row\">No matches.</p>".to_string()
    } else {
        result_rows(&hits)
    };

    let body = format!("{header}{bar}<div class=\"card\">{results}</div>");
    Html(dashboard::document("code search", &body, Tab::Code)).into_response()
}

/// Builds the header of the search page: the collection, the query, and how
/// the answer was found.
fn search_header(
    collection: &str,
    query: &str,
    hits: &[Hit],
    elapsed: Option<std::time::Duration>,
) -> String {
    let title = if query.trim().is_empty() {
        "Search the code".to_string()
    } else {
        format!("“{query}”")
    };
    let meta = match elapsed {
        Some(elapsed) => format!(
            "{} · {} · {}ms",
            dashboard::plural(hits.len() as u64, "result", "results"),
            search_mode(hits),
            elapsed.as_millis()
        ),
        None => "waiting for a question".to_string(),
    };
    format!(
        "<div class=\"page-header\"><p class=\"eyebrow-line\">Code search · collection {}</p>\
         <h1 class=\"query-title\">{}</h1><p class=\"meta-line\">{}</p></div>",
        dashboard::escape(&repo_label(collection).to_uppercase()),
        dashboard::escape(&title),
        dashboard::escape(&meta)
    )
}

/// Says which index, or which mix of the two, answered a search.
fn search_mode(hits: &[Hit]) -> &'static str {
    let keyword = hits.iter().any(|hit| hit.keyword_score.is_some());
    let vector = hits.iter().any(|hit| hit.vector_score.is_some());
    match (keyword, vector) {
        (true, true) => "mixed keyword + vector",
        (true, false) => "keyword only",
        (false, true) => "vector only",
        (false, false) => "no match",
    }
}

/// Builds the list of search results. Each row links to the definition that
/// it names, with the file and the line that hold it below the query.
fn result_rows(hits: &[Hit]) -> String {
    let mut html = String::from("<ul class=\"results\">");
    for hit in hits {
        let short_name = short_name(&hit.qualified_name);
        let line = hit
            .start_line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        let score = hit.score.clamp(0.0, 1.0) * 100.0;
        html.push_str(&format!(
            "<li><a class=\"result-row\" href=\"/code/{}?def={}\">\
             <div class=\"result-head\"><div class=\"result-name\">\
             <span class=\"name\">{}</span><span class=\"kind-chip\">{}</span></div>\
             <div class=\"result-score\"><span class=\"score-track\">\
             <span class=\"score-fill\" style=\"width:{score:.0}%\"></span></span>\
             <span class=\"hint\">{:.2}</span></div></div>\
             <div class=\"result-where\">{}{}</div>\
             <p class=\"result-summary\">{}</p></a></li>",
            dashboard::escape(&hit.rel_path),
            dashboard::escape(&hit.qualified_name),
            dashboard::escape(short_name),
            dashboard::escape(&hit.kind),
            hit.score,
            dashboard::escape(&hit.rel_path),
            dashboard::escape(&line),
            dashboard::escape(&hit.summary),
        ));
    }
    html.push_str("</ul>");
    html
}

/// Builds the page of one node: a directory, a file, or the root.
fn render_browse(state: &CodeState, rel_path: &str, def: Option<&str>) -> Response {
    let index = state
        .index
        .lock()
        .expect("the code index lock is never poisoned");
    let collection = state.collection.as_str();

    let root = match api::tree(index.database(), collection, "", None) {
        Ok(root) => root,
        Err(Error::NoSuchCollection(_)) => return not_indexed_page(collection),
        Err(err) => {
            return dashboard::error_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                Tab::Code,
                &err.to_string(),
            );
        }
    };

    let node = match index.cat(collection, rel_path) {
        Ok(node) => node,
        Err(Error::NoSuchNode(_)) => {
            return dashboard::error_page(
                StatusCode::NOT_FOUND,
                Tab::Code,
                &format!("{rel_path} holds nothing yet"),
            );
        }
        Err(err) => {
            return dashboard::error_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                Tab::Code,
                &err.to_string(),
            );
        }
    };

    let (nodes, described) = match api::subtree_status(index.database(), collection, rel_path) {
        Ok(status) => status,
        Err(err) => {
            return dashboard::error_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                Tab::Code,
                &err.to_string(),
            );
        }
    };

    let defs = if node.kind == "file" {
        match api::definitions(index.database(), collection, rel_path) {
            Ok(defs) => defs,
            Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Code,
                    &err.to_string(),
                );
            }
        }
    } else {
        Vec::new()
    };

    let header = code_header(collection, rel_path, &node.kind, nodes, described);
    let bar = dashboard::search_bar(
        "/code/search",
        "",
        "recall — search code by name or meaning",
        false,
        "⌘K",
        None,
    );
    let tree = tree_panel(&root, rel_path);
    let detail = detail_panel(&node, &defs, def);

    let body = format!("{header}{bar}<div class=\"body\">{tree}{detail}</div>");
    let title = if rel_path.is_empty() {
        repo_label(collection)
    } else {
        rel_path.to_string()
    };
    Html(dashboard::document(&title, &body, Tab::Code)).into_response()
}

/// A page for a collection that nobody has indexed yet. Not an error: the
/// dashboard starts before `embornal code index` ever ran.
fn not_indexed_page(collection: &str) -> Response {
    let body = format!(
        "<div class=\"page-header\"><p class=\"label\">Code</p>\
         <h1 class=\"path-title\">{}</h1></div>\
         <div class=\"empty-state\">Nothing indexed yet. Run <code>embornal code index</code> \
         in this repository to build it.</div>",
        dashboard::escape(&repo_label(collection))
    );
    Html(dashboard::document("code", &body, Tab::Code)).into_response()
}

/// Builds the trail, the title with its kind, and the metadata line at the
/// top of a page.
fn code_header(collection: &str, rel_path: &str, kind: &str, nodes: u64, described: u64) -> String {
    let name = if rel_path.is_empty() {
        repo_label(collection)
    } else {
        short_name(rel_path).to_string()
    };
    let waiting = nodes.saturating_sub(described);
    format!(
        "<div class=\"page-header\">{}<div class=\"title-row\">\
         <h1 class=\"path-title\">{}</h1><span class=\"title-badge\">{}</span></div>\
         <p class=\"meta-line\">{} · {described} described · {waiting} waiting · collection {}</p></div>",
        code_breadcrumbs(collection, rel_path),
        dashboard::escape(&name),
        dashboard::escape(kind),
        dashboard::plural(nodes, "node", "nodes"),
        dashboard::escape(&repo_label(collection)),
    )
}

/// Builds the trail from the repository down to the node, mirroring the
/// wiki's own trail: every step but the last links to its own page.
fn code_breadcrumbs(collection: &str, rel_path: &str) -> String {
    let mut html = String::from("<nav class=\"trail\">");
    let root_label = repo_label(collection);
    if rel_path.is_empty() {
        html.push_str(&format!(
            "<span class=\"current\">{}</span>",
            dashboard::escape(&root_label)
        ));
    } else {
        html.push_str(&format!(
            "<a href=\"/code\">{}</a>",
            dashboard::escape(&root_label)
        ));
        let mut built = String::new();
        let segments: Vec<&str> = rel_path.split('/').collect();
        for (index, segment) in segments.iter().enumerate() {
            html.push_str("<span class=\"sep\">/</span>");
            if !built.is_empty() {
                built.push('/');
            }
            built.push_str(segment);
            if index + 1 == segments.len() {
                html.push_str(&format!(
                    "<span class=\"current\">{}</span>",
                    dashboard::escape(segment)
                ));
            } else {
                html.push_str(&format!(
                    "<a href=\"/code/{}\">{}</a>",
                    dashboard::escape(&built),
                    dashboard::escape(segment)
                ));
            }
        }
    }
    html.push_str("</nav>");
    html
}

/// The name that a repository shows for itself: the last part of the path
/// that names its collection.
fn repo_label(collection: &str) -> String {
    FsPath::new(collection)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| collection.to_string())
}

/// The last segment of a qualified name or a path, such as `recall` from
/// `src/code/api.rs::CodeIndex::recall`, or `api.rs` from `src/code/api.rs`.
fn short_name(name: &str) -> &str {
    name.rsplit("::")
        .next()
        .and_then(|n| n.rsplit('/').next())
        .unwrap_or(name)
}

/// Builds the "Tree" column: the whole tree of the collection, with the
/// directories that lead to the current node held open.
fn tree_panel(root: &TreeNode, current: &str) -> String {
    let mut files = 0u64;
    count_files(root, &mut files);

    let mut card = String::from("<div class=\"tree-card\">");
    render_tree_node(root, current, &mut card);
    card.push_str("</div>");

    format!(
        "<section class=\"tree-col\"><div class=\"tree-head\"><p class=\"label\">Tree</p>\
         <span class=\"hint\">{}</span></div>{card}\
         <div class=\"tree-legend\"><span><span class=\"tree-dot\"></span>described</span>\
         <span><span class=\"tree-dot is-waiting\"></span>waiting</span></div></section>",
        dashboard::plural(files, "file", "files")
    )
}

fn count_files(node: &TreeNode, files: &mut u64) {
    if node.kind == "file" {
        *files += 1;
    }
    for child in &node.children {
        count_files(child, files);
    }
}

fn render_tree_node(node: &TreeNode, current: &str, out: &mut String) {
    let dot_class = if node.described {
        "tree-dot"
    } else {
        "tree-dot is-waiting"
    };

    if node.kind == "file" {
        let row_class = if node.rel_path == current {
            "tree-file is-current"
        } else {
            "tree-file"
        };
        out.push_str(&format!(
            "<a class=\"{row_class}\" href=\"/code/{}\">\
             <span class=\"tree-toggle\"></span><span class=\"{dot_class}\"></span>\
             <span class=\"tree-name\">{}</span></a>",
            dashboard::escape(&node.rel_path),
            dashboard::escape(&node.name)
        ));
        return;
    }

    let href = if node.rel_path.is_empty() {
        "/code".to_string()
    } else {
        format!("/code/{}", dashboard::escape(&node.rel_path))
    };
    let open = is_ancestor_or_self(&node.rel_path, current);
    out.push_str(&format!(
        "<details class=\"tree-dir\"{}><summary><span class=\"tree-toggle\"></span>\
         <span class=\"{dot_class}\"></span>\
         <span class=\"tree-name is-dir\"><a href=\"{href}\">{}</a></span></summary>\
         <div class=\"tree-children\">",
        if open { " open" } else { "" },
        dashboard::escape(&node.name)
    ));
    for child in &node.children {
        render_tree_node(child, current, out);
    }
    out.push_str("</div></details>");
}

/// Whether `dir` is `current` itself, or a directory above it.
fn is_ancestor_or_self(dir: &str, current: &str) -> bool {
    dir.is_empty() || current == dir || current.starts_with(&format!("{dir}/"))
}

/// Builds the "Detail" column: what is known about the selected node, and,
/// for a file, its definitions.
fn detail_panel(node: &Described, defs: &[Described], selected: Option<&str>) -> String {
    let mut html = format!("<div class=\"detail-col\">{}", overview_card(node));

    if node.kind == "file" {
        let described = defs.iter().filter(|def| def.summary.is_some()).count();
        html.push_str(&format!(
            "<div class=\"definitions-head\"><p class=\"label\">Definitions</p>\
             <span class=\"hint\">{described} of {} described</span></div>",
            defs.len()
        ));

        if defs.is_empty() {
            html.push_str(
                "<p class=\"empty-row\">This file defines nothing that the grammar names.</p>",
            );
        } else {
            let picked = selected
                .and_then(|name| defs.iter().find(|def| def.qualified_name == name))
                .or_else(|| defs.first());
            html.push_str(&definitions_list(
                defs,
                picked.map(|def| def.qualified_name.as_str()),
            ));
            if let Some(picked) = picked {
                html.push_str(&selected_definition_card(picked));
            }
        }
    }

    html.push_str("</div>");
    html
}

/// Builds the card that says what is known about a directory, a file, or
/// the repository itself.
fn overview_card(node: &Described) -> String {
    let path_label = if node.rel_path.is_empty() {
        "/".to_string()
    } else {
        node.rel_path.clone()
    };
    let body = match (&node.summary, &node.description) {
        (Some(summary), Some(description)) => format!(
            "<div class=\"overview-title\">{}</div><div class=\"overview-body\">{}</div>",
            dashboard::escape(summary),
            dashboard::escape(description)
        ),
        _ => "<p class=\"empty-note\">No summary yet.</p>".to_string(),
    };
    format!(
        "<div class=\"card overview-card\"><div class=\"overview-meta\">\
         <span class=\"kind-chip\">{}</span><span class=\"overview-path\">{}</span></div>{body}</div>",
        dashboard::escape(&node.kind),
        dashboard::escape(&path_label)
    )
}

/// Builds the flat list of one file's definitions, in the order they sit in
/// the file. The selected one is marked, so a reader can see which row the
/// card below expands.
fn definitions_list(defs: &[Described], selected: Option<&str>) -> String {
    let mut html = String::from("<ul class=\"definitions\">");
    for def in defs {
        let is_current = Some(def.qualified_name.as_str()) == selected;
        let row_class = if is_current {
            "definition-row is-current"
        } else {
            "definition-row"
        };
        let dot_class = if def.summary.is_some() {
            "tree-dot"
        } else {
            "tree-dot is-waiting"
        };
        let line = def
            .start_line
            .map(|line| format!("L{line}"))
            .unwrap_or_default();
        html.push_str(&format!(
            "<li><a class=\"{row_class}\" href=\"/code/{}?def={}\">\
             <span class=\"definition-name\"><span class=\"{dot_class}\"></span>\
             <span class=\"name\">{}</span><span class=\"kind-chip\">{}</span></span>\
             <span class=\"definition-line\">{}</span></a></li>",
            dashboard::escape(&def.rel_path),
            dashboard::escape(&def.qualified_name),
            dashboard::escape(short_name(&def.qualified_name)),
            dashboard::escape(&def.kind),
            dashboard::escape(&line),
        ));
    }
    html.push_str("</ul>");
    html
}

/// Builds the card that expands one definition: its full name, its lines,
/// and what an agent wrote about it.
fn selected_definition_card(def: &Described) -> String {
    let lines = match (def.start_line, def.end_line) {
        (Some(from), Some(to)) => format!("L{from}–{to}"),
        (Some(from), None) => format!("L{from}"),
        _ => String::new(),
    };
    let head = format!(
        "<div class=\"selected-head\"><span class=\"title-badge\">{}</span>\
         <span class=\"overview-path\">{}</span><span class=\"hint\">{}</span></div>",
        dashboard::escape(&def.kind),
        dashboard::escape(&def.qualified_name),
        dashboard::escape(&lines)
    );
    let body = match (&def.summary, &def.description) {
        (Some(summary), Some(description)) => {
            let mut body = format!(
                "<p class=\"selected-summary\">{}</p><p class=\"selected-description\">{}</p>",
                dashboard::escape(summary),
                dashboard::escape(description)
            );
            if let (Some(author), Some(written_at)) = (&def.author, &def.written_at) {
                let date = written_at.split('T').next().unwrap_or(written_at);
                body.push_str(&format!(
                    "<div class=\"selected-footer\"><span>written {}</span><span>·</span>\
                     <span>by {}</span></div>",
                    dashboard::escape(date),
                    dashboard::escape(author)
                ));
            }
            body
        }
        _ => "<p class=\"empty-note\">No summary yet.</p>".to_string(),
    };
    format!("<div class=\"selected-card\">{head}{body}</div>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn described(rel_path: &str, kind: &str, qualified_name: &str) -> Described {
        Described {
            qualified_name: qualified_name.to_string(),
            kind: kind.to_string(),
            rel_path: rel_path.to_string(),
            start_line: Some(10),
            end_line: Some(20),
            summary: None,
            description: None,
            written_at: None,
            author: None,
        }
    }

    #[test]
    fn repo_label_reads_the_last_segment_of_the_collection() {
        assert_eq!(repo_label("/home/a/projects/embornal"), "embornal");
        assert_eq!(repo_label("embornal"), "embornal");
    }

    #[test]
    fn short_name_reads_the_last_step_of_a_qualified_name_or_a_path() {
        assert_eq!(short_name("src/code/api.rs::CodeIndex::recall"), "recall");
        assert_eq!(short_name("src/code/api.rs"), "api.rs");
        assert_eq!(short_name("free"), "free");
    }

    #[test]
    fn the_breadcrumbs_link_every_step_but_the_last() {
        let html = code_breadcrumbs("/home/a/embornal", "src/code/api.rs");
        assert!(html.contains("<a href=\"/code\">embornal</a>"));
        assert!(html.contains("<a href=\"/code/src\">src</a>"));
        assert!(html.contains("<a href=\"/code/src/code\">code</a>"));
        assert!(html.contains("<span class=\"current\">api.rs</span>"));
        assert!(!html.contains("<a href=\"/code/src/code/api.rs\">"));
    }

    #[test]
    fn the_breadcrumbs_of_the_root_hold_the_repository_only() {
        let html = code_breadcrumbs("/home/a/embornal", "");
        assert!(html.contains("<span class=\"current\">embornal</span>"));
        assert!(!html.contains("<a href"));
    }

    #[test]
    fn a_directory_is_open_when_it_leads_to_the_current_node() {
        assert!(is_ancestor_or_self("", "src/code/api.rs"));
        assert!(is_ancestor_or_self("src", "src/code/api.rs"));
        assert!(is_ancestor_or_self("src/code", "src/code/api.rs"));
        assert!(is_ancestor_or_self("src/code", "src/code"));
        assert!(!is_ancestor_or_self("src/cli", "src/code/api.rs"));
        // "src/co" must not match "src/code" by a bare prefix.
        assert!(!is_ancestor_or_self("src/co", "src/code/api.rs"));
    }

    #[test]
    fn the_overview_card_shows_what_was_written_or_says_there_is_none() {
        let mut node = described("src/a.rs", "file", "src/a.rs");
        assert!(overview_card(&node).contains("No summary yet."));

        node.summary = Some("Reads a directory.".to_string());
        node.description = Some("Walks the tree.".to_string());
        let html = overview_card(&node);
        assert!(html.contains("Reads a directory."));
        assert!(html.contains("Walks the tree."));
    }

    #[test]
    fn the_definitions_list_marks_the_selected_row() {
        let defs = [
            described("src/a.rs", "function", "src/a.rs::one"),
            described("src/a.rs", "function", "src/a.rs::two"),
        ];
        let html = definitions_list(&defs, Some("src/a.rs::two"));
        assert!(html.contains("is-current"));
        assert!(html.contains(">one<"));
        assert!(html.contains(">two<"));
    }

    #[test]
    fn the_selected_card_carries_the_lines_and_the_full_name() {
        let mut def = described("src/a.rs", "function", "src/a.rs::one");
        let html = selected_definition_card(&def);
        assert!(html.contains("src/a.rs::one"));
        assert!(html.contains("L10–20"));
        assert!(html.contains("No summary yet."));

        def.summary = Some("Reads a token.".to_string());
        def.description = Some("It reads a token.".to_string());
        def.author = Some("default".to_string());
        def.written_at = Some("2026-08-30T12:00:00Z".to_string());
        let html = selected_definition_card(&def);
        assert!(html.contains("written 2026-08-30"));
        assert!(html.contains("by default"));
    }

    #[test]
    fn count_files_ignores_directories() {
        let tree = TreeNode {
            name: "src".to_string(),
            rel_path: "src".to_string(),
            kind: "dir".to_string(),
            described: true,
            children: vec![
                TreeNode {
                    name: "a.rs".to_string(),
                    rel_path: "src/a.rs".to_string(),
                    kind: "file".to_string(),
                    described: true,
                    children: vec![],
                },
                TreeNode {
                    name: "sub".to_string(),
                    rel_path: "src/sub".to_string(),
                    kind: "dir".to_string(),
                    described: false,
                    children: vec![TreeNode {
                        name: "b.rs".to_string(),
                        rel_path: "src/sub/b.rs".to_string(),
                        kind: "file".to_string(),
                        described: false,
                        children: vec![],
                    }],
                },
            ],
        };
        let mut files = 0;
        count_files(&tree, &mut files);
        assert_eq!(files, 2);
    }
}
