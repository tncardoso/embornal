//! The wiki server.
//!
//! `embornal memory wiki` shows the memory as a small wiki. Each path is a
//! page that holds its facts and the paths below it. A `[[/link]]` in a fact
//! becomes a link to that page.
//!
//! This server reads. It answers one person, and it has no login. The server
//! that many people share is [`crate::api`], which asks for a token.

use crate::error::{Error, Result};
use crate::memory::api::{CatOptions, Memory, RecallOptions};
use crate::memory::fact::Fact;
use crate::memory::link::{self, Segment};
use crate::memory::path::WikiPath;
use crate::memory::tag::TagSet;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

/// The memory that the handlers share.
///
/// One SQLite connection cannot serve two threads at once, so the handlers
/// take turns. The work of one request is short, and this server answers one
/// person, not a crowd.
type Shared = Arc<Mutex<Memory>>;

/// Starts the server and blocks until it stops.
pub fn wiki(memory: Memory, port: u16) -> Result<()> {
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

    let hits = match hits {
        Ok(hits) => hits,
        Err(err) => return error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };

    let mut body = String::new();
    body.push_str(&search_form(&query.q));
    body.push_str(&format!("<p class=\"count\">{} facts</p>", hits.len()));
    body.push_str("<ul class=\"facts\">");
    for hit in &hits {
        let tags = match memory.effective_tags(hit.fact.id) {
            Ok(tags) => tags,
            Err(err) => return error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
        };
        body.push_str(&format!(
            "<li>{}<div class=\"where\"><a href=\"{}\">{}</a></div>{}</li>",
            render_content(&hit.fact.content),
            escape(hit.fact.path.as_str()),
            escape(hit.fact.path.as_str()),
            // The strength is the one that the fact had when the search found
            // it. The recall that follows lifts it.
            about(hit.signal_strength, hit.fact.created_at, &tags)
        ));
    }
    body.push_str("</ul>");
    Html(document(&format!("search: {}", query.q), &body)).into_response()
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

    let tags = match tags_of(&memory, &facts) {
        Ok(tags) => tags,
        Err(err) => return error_page(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };

    let now = Utc::now();
    let mut body = String::new();
    body.push_str(&search_form(""));
    body.push_str(&breadcrumbs(&path));
    body.push_str(&metadata(
        listing.fact_count,
        listing.subtree_fact_count,
        listing.children.len() as u64,
        strength(&facts, now),
    ));
    body.push_str(&render_facts(&facts, &tags, now));

    if !listing.children.is_empty() {
        body.push_str("<h2>Below</h2><ul class=\"children\">");
        for entry in &listing.children {
            let name = entry.path.segment().unwrap_or("/");
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a> <span class=\"count\">{} · {} total</span></li>",
                escape(entry.path.as_str()),
                escape(name),
                plural(entry.fact_count, "fact", "facts"),
                plural(entry.subtree_fact_count, "fact", "facts"),
            ));
        }
        body.push_str("</ul>");
    }

    if facts.is_empty() && listing.children.is_empty() {
        body.push_str("<p class=\"empty\">This path holds nothing.</p>");
    }

    Html(document(path.as_str(), &body)).into_response()
}

/// Builds the line that says what the path holds.
///
/// The page shows its direct fact count and the total for its full subtree.
/// It also shows the paths one step below it. The signal is the strength of
/// the path, which a path with no fact does not have.
fn metadata(
    fact_count: u64,
    subtree_fact_count: u64,
    child_count: u64,
    strength: Option<f64>,
) -> String {
    let mut parts = vec![
        plural(fact_count, "fact", "facts"),
        format!("{} total", plural(subtree_fact_count, "fact", "facts")),
        plural(child_count, "child", "children"),
    ];
    if let Some(strength) = strength {
        parts.push(format!("signal {strength:.3}"));
    }
    format!("<p class=\"meta\">{}</p>", parts.join(" · "))
}

/// Returns the count with the word that goes with it.
fn plural(count: u64, one: &str, many: &str) -> String {
    let word = if count == 1 { one } else { many };
    format!("{count} {word}")
}

/// Reads the tags of each fact, in the order of the facts.
fn tags_of(memory: &Memory, facts: &[Fact]) -> Result<Vec<TagSet>> {
    facts
        .iter()
        .map(|fact| memory.effective_tags(fact.id))
        .collect()
}

/// Returns the strength of a path at `now`.
///
/// A path holds many facts, each with its own strength. The mean says how
/// fresh the path is as a whole. A path with no fact has no strength.
fn strength(facts: &[Fact], now: DateTime<Utc>) -> Option<f64> {
    if facts.is_empty() {
        return None;
    }
    let total: f64 = facts.iter().map(|fact| fact.signal.strength_at(now)).sum();
    Some(total / facts.len() as f64)
}

/// Builds the list of the facts of one path.
///
/// Each fact carries its own strength at `now`, because a path can hold a
/// fact that somebody reads each day next to one that the memory almost lost.
/// The tags come in the same order as the facts.
fn render_facts(facts: &[Fact], tags: &[TagSet], now: DateTime<Utc>) -> String {
    if facts.is_empty() {
        return String::new();
    }
    let mut html = String::from("<ul class=\"facts\">");
    for (fact, tags) in facts.iter().zip(tags) {
        html.push_str(&format!(
            "<li>{}{}</li>",
            render_content(&fact.content),
            about(fact.signal.strength_at(now), fact.created_at, tags)
        ));
    }
    html.push_str("</ul>");
    html
}

/// Builds the line that says what one fact is.
///
/// The strength goes from 1.000 for a fact that somebody just read to 0.000
/// for a fact that the memory almost lost. The date is the day on which
/// somebody wrote the fact. The tags are the ones that decide who reads it,
/// which include the tags that the fact takes from the paths above it. A fact
/// with no tag shows no tags.
fn about(strength: f64, created_at: DateTime<Utc>, tags: &TagSet) -> String {
    let mut parts = vec![
        format!("signal {strength:.3}"),
        created_at.format("%Y-%m-%d").to_string(),
    ];
    if !tags.is_empty() {
        parts.push(
            tags.iter()
                .map(|tag| escape(&tag.to_string()))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    format!("<div class=\"about\">{}</div>", parts.join(" · "))
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
.count, .where, .about { opacity: .55; font-size: .85rem; }
.about { font-family: ui-monospace, monospace; margin-top: .2rem; }
p.meta { font-family: ui-monospace, monospace; font-size: .85rem; opacity: .55; margin: 0 0 1rem; }
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
    use crate::memory::tag::Tag;
    use crate::memory::{FactId, PathId, Signal};
    use ulid::Ulid;

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
    fn the_metadata_holds_the_counts_and_the_signal() {
        assert_eq!(
            metadata(3, 5, 2, Some(0.8127)),
            "<p class=\"meta\">3 facts · 5 facts total · 2 children · signal 0.813</p>"
        );
    }

    #[test]
    fn the_metadata_uses_the_singular_for_one() {
        assert_eq!(
            metadata(1, 1, 1, None),
            "<p class=\"meta\">1 fact · 1 fact total · 1 child</p>"
        );
    }

    #[test]
    fn a_path_with_no_fact_has_no_signal() {
        assert_eq!(
            metadata(0, 0, 4, None),
            "<p class=\"meta\">0 facts · 0 facts total · 4 children</p>"
        );
    }

    #[test]
    fn each_fact_carries_its_own_signal_and_date() {
        let now = Utc::now();
        let written = "2026-07-28T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let fresh = fact(Signal::new(now), written);
        let old = fact(Signal::new(now - chrono::Duration::days(365)), written);

        let html = render_facts(&[fresh, old], &[TagSet::new(), TagSet::new()], now);
        assert!(
            html.contains("<div class=\"about\">signal 1.000 · 2026-07-28</div>"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"about\">signal 0.000 · 2026-07-28</div>"),
            "{html}"
        );
    }

    #[test]
    fn a_fact_carries_the_tags_that_it_holds() {
        let written = "2026-07-28T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut tags = TagSet::new();
        tags.insert(Tag::parse("visibility=private").unwrap());
        tags.insert(Tag::parse("kind=note").unwrap());

        assert_eq!(
            about(1.0, written, &tags),
            "<div class=\"about\">signal 1.000 · 2026-07-28 · kind=note visibility=private</div>"
        );
    }

    #[test]
    fn a_fact_with_no_tag_shows_the_signal_and_the_date_only() {
        let written = "2026-07-28T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            about(0.5, written, &TagSet::new()),
            "<div class=\"about\">signal 0.500 · 2026-07-28</div>"
        );
    }

    #[test]
    fn a_path_with_no_fact_holds_no_list() {
        assert_eq!(render_facts(&[], &[], Utc::now()), "");
    }

    #[test]
    fn the_signal_of_a_path_is_the_mean_of_its_facts() {
        let now = Utc::now();
        let fresh = fact(Signal::new(now), now);
        let old = fact(Signal::new(now - chrono::Duration::days(365)), now);

        assert!(strength(&[], now).is_none());
        assert!((strength(std::slice::from_ref(&fresh), now).unwrap() - 1.0).abs() < 1e-6);

        let mean = strength(&[fresh, old.clone()], now).unwrap();
        let weak = strength(std::slice::from_ref(&old), now).unwrap();
        assert!(weak < mean && mean < 1.0);
    }

    /// Builds a fact that carries the signal and the date that a test needs.
    fn fact(signal: Signal, created_at: DateTime<Utc>) -> Fact {
        Fact {
            id: FactId(1),
            ulid: Ulid::generate(),
            path_id: PathId(1),
            path: WikiPath::parse("/a").unwrap(),
            content: "one".to_string(),
            owner: "cli".to_string(),
            created_at,
            signal,
            supersedes_id: None,
            deleted_at: None,
            embedding_model: None,
        }
    }

    #[test]
    fn the_document_carries_the_title_once_escaped() {
        let html = document("/a & b", "<p>x</p>");
        assert!(html.contains("<title>/a &amp; b — embornal</title>"));
        assert!(html.contains("<p>x</p>"));
        assert!(!html.contains("/a & b"));
    }
}
