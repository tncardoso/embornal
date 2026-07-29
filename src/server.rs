//! The wiki server.
//!
//! `embornal memory serve` shows the memory as a small wiki. Each path is a
//! page that holds its facts and the paths below it. A `[[/link]]` in a fact
//! becomes a link to that page.

use crate::error::{Error, Result};
use crate::memory::api::{CatOptions, Memory, RecallOptions};
use crate::memory::fact::Fact;
use crate::memory::link::{self, Segment};
use crate::memory::path::WikiPath;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

/// The memory that the handlers share.
///
/// One SQLite connection cannot serve two threads at once, so the handlers
/// take turns. The work of one request is short, and this server answers one
/// person, not a crowd.
type Shared = Arc<Mutex<Memory>>;

/// Starts the server and blocks until it stops.
pub fn serve(memory: Memory, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Serve(err.to_string()))?;

    runtime.block_on(async move {
        let state: Shared = Arc::new(Mutex::new(memory));
        let app = router(state);

        let address = format!("0.0.0.0:{port}");
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|err| Error::Serve(format!("cannot listen on {address}: {err}")))?;

        println!("the wiki is at http://localhost:{port}");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown())
            .await
            .map_err(|err| Error::Serve(err.to_string()))
    })
}

/// Builds the routes.
pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/search", get(search))
        .route("/{*path}", get(page))
        .with_state(state)
}

async fn index(State(state): State<Shared>) -> Response {
    render_page(&state, WikiPath::root())
}

async fn page(State(state): State<Shared>, Path(rest): Path<String>) -> Response {
    match WikiPath::parse(&format!("/{rest}")) {
        Ok(path) => render_page(&state, path),
        Err(err) => error_page(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(State(state): State<Shared>, Query(query): Query<SearchQuery>) -> Response {
    let mut memory = state.lock().expect("the memory lock is never poisoned");
    let hits = memory.recall(
        Some(query.q.as_str()).filter(|q| !q.trim().is_empty()),
        RecallOptions {
            limit: 30,
            under: None,
            // A search in the browser is a real recall.
            reinforce: true,
        },
    );

    match hits {
        Ok(hits) => {
            let mut body = String::new();
            body.push_str(&search_form(&query.q));
            body.push_str(&format!("<p class=\"count\">{} facts</p>", hits.len()));
            body.push_str("<ul class=\"facts\">");
            for hit in &hits {
                body.push_str(&format!(
                    "<li>{}<div class=\"where\"><a href=\"{}\">{}</a></div></li>",
                    render_content(&hit.fact.content),
                    escape(hit.fact.path.as_str()),
                    escape(hit.fact.path.as_str())
                ));
            }
            body.push_str("</ul>");
            Html(document(&format!("search: {}", query.q), &body)).into_response()
        }
        Err(err) => error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

/// Builds the page of one path.
fn render_page(state: &Shared, path: WikiPath) -> Response {
    let mut memory = state.lock().expect("the memory lock is never poisoned");

    let listing = match memory.ls(&path) {
        Ok(listing) => listing,
        Err(Error::PathNotFound(_)) => {
            return error_page(StatusCode::NOT_FOUND, &format!("{path} holds nothing yet"));
        }
        Err(err) => return error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };

    // Reading a page is not a recall: the page shows each fact of the path at
    // once, so it says nothing about which fact was useful.
    let facts = match memory.cat(&path, CatOptions::default()) {
        Ok(facts) => facts,
        Err(err) => return error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };

    let mut body = String::new();
    body.push_str(&search_form(""));
    body.push_str(&breadcrumbs(&path));
    body.push_str(&render_facts(&facts));

    if !listing.children.is_empty() {
        body.push_str("<h2>Below</h2><ul class=\"children\">");
        for entry in &listing.children {
            let name = entry.path.segment().unwrap_or("/");
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a> <span class=\"count\">{} facts</span></li>",
                escape(entry.path.as_str()),
                escape(name),
                entry.fact_count
            ));
        }
        body.push_str("</ul>");
    }

    if facts.is_empty() && listing.children.is_empty() {
        body.push_str("<p class=\"empty\">This path holds nothing.</p>");
    }

    Html(document(path.as_str(), &body)).into_response()
}

fn render_facts(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }
    let mut html = String::from("<ul class=\"facts\">");
    for fact in facts {
        html.push_str(&format!("<li>{}</li>", render_content(&fact.content)));
    }
    html.push_str("</ul>");
    html
}

/// Turns the content of a fact into HTML, with the links followed.
pub fn render_content(content: &str) -> String {
    let mut html = String::new();
    for segment in link::parse(content) {
        match segment {
            Segment::Text(text) => html.push_str(&escape(text)),
            Segment::Link { target, label } => html.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(target.as_str()),
                escape(label)
            )),
            // A pair of brackets that is not a path stays as it was written.
            Segment::Broken(text) => html.push_str(&escape(&format!("[[{text}]]"))),
        }
    }
    html
}

/// Builds the trail from the root down to the path.
fn breadcrumbs(path: &WikiPath) -> String {
    let mut html = String::from("<nav class=\"trail\">");
    let chain = path.ancestry();
    for (index, step) in chain.iter().enumerate() {
        if index > 0 {
            html.push_str(" / ");
        }
        let label = step.segment().unwrap_or("root");
        if index + 1 == chain.len() {
            html.push_str(&format!("<strong>{}</strong>", escape(label)));
        } else {
            html.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(step.as_str()),
                escape(label)
            ));
        }
    }
    html.push_str("</nav>");
    html
}

fn search_form(value: &str) -> String {
    format!(
        "<form class=\"search\" action=\"/search\" method=\"get\">\
         <input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"recall\" autofocus>\
         </form>",
        escape(value)
    )
}

fn error_page(status: StatusCode, message: &str) -> Response {
    let body = format!("<p class=\"error\">{}</p>", escape(message));
    (status, Html(document("not found", &body))).into_response()
}

/// Wraps the body in a page.
fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} — embornal</title><style>{STYLE}</style></head>\
         <body><header><a class=\"home\" href=\"/\">embornal</a></header>\
         <main><h1>{title}</h1>{body}</main></body></html>",
        title = escape(title)
    )
}

const STYLE: &str = "
:root { color-scheme: light dark; }
body { font: 16px/1.6 system-ui, sans-serif; max-width: 44rem; margin: 0 auto; padding: 1.5rem; }
header { border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent); padding-bottom: .5rem; margin-bottom: 1.5rem; }
a.home { font-weight: 600; text-decoration: none; }
h1 { font-size: 1.4rem; font-family: ui-monospace, monospace; }
h2 { font-size: 1rem; text-transform: uppercase; letter-spacing: .05em; opacity: .6; margin-top: 2rem; }
nav.trail { font-family: ui-monospace, monospace; font-size: .9rem; margin-bottom: 1rem; }
ul.facts { list-style: none; padding: 0; }
ul.facts li { padding: .6rem 0; border-bottom: 1px solid color-mix(in srgb, currentColor 12%, transparent); }
ul.children { list-style: none; padding: 0; font-family: ui-monospace, monospace; }
ul.children li { padding: .2rem 0; }
.count, .where { opacity: .55; font-size: .85rem; }
.empty, .error { opacity: .6; font-style: italic; }
form.search { margin-bottom: 1.5rem; }
form.search input { width: 100%; padding: .5rem .7rem; font: inherit; border-radius: .4rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent); background: transparent; color: inherit; }
";

/// Makes a string safe to put into HTML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_what_a_browser_would_read_as_markup() {
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("a & b"), "a &amp; b");
        assert_eq!(escape("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(escape("it's"), "it&#39;s");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn a_link_becomes_an_anchor() {
        assert_eq!(
            render_content("see [[/a/b]] now"),
            "see <a href=\"/a/b\">/a/b</a> now"
        );
    }

    #[test]
    fn a_link_keeps_the_label_that_the_writer_wrote() {
        assert_eq!(
            render_content("[[/Projects/Embornal]]"),
            "<a href=\"/projects/embornal\">/Projects/Embornal</a>"
        );
    }

    #[test]
    fn brackets_that_are_not_a_path_stay_as_text() {
        assert_eq!(render_content("[[TODO]]"), "[[TODO]]");
    }

    #[test]
    fn content_cannot_carry_markup_into_the_page() {
        let html = render_content("<img src=x onerror=alert(1)> and [[/a]]");
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;img"));
        assert!(html.contains("<a href=\"/a\">"));
    }

    #[test]
    fn a_path_that_looks_like_markup_cannot_reach_the_page() {
        // The path type refuses these, and the escape is the second wall.
        assert!(WikiPath::parse("/<script>").is_err());
        assert!(escape("/\"onload=\"").contains("&quot;"));
    }

    #[test]
    fn the_trail_links_each_step_but_the_last() {
        let html = breadcrumbs(&WikiPath::parse("/a/b").unwrap());
        assert!(html.contains("<a href=\"/\">root</a>"));
        assert!(html.contains("<a href=\"/a\">a</a>"));
        assert!(html.contains("<strong>b</strong>"));
        assert!(!html.contains("<a href=\"/a/b\">"));
    }

    #[test]
    fn the_trail_of_the_root_holds_the_root_only() {
        let html = breadcrumbs(&WikiPath::root());
        assert!(html.contains("<strong>root</strong>"));
        assert!(!html.contains("<a href"));
    }

    #[test]
    fn the_document_carries_the_title_once_escaped() {
        let html = document("/a & b", "<p>x</p>");
        assert!(html.contains("<title>/a &amp; b — embornal</title>"));
        assert!(html.contains("<p>x</p>"));
        assert!(!html.contains("/a & b"));
    }
}
