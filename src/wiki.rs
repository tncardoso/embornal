//! The wiki server.
//!
//! `embornal dashboard` shows the memory as a small wiki. Each path is a page
//! that holds its facts and the paths below it. A `[[/link]]` in a fact
//! becomes a link to that page.
//!
//! This server reads. It answers one person, and it has no login. The server
//! that many people share is [`crate::api`], which asks for a token.
//!
//! The frame around the page — the header, the fonts, the colors — is
//! [`crate::dashboard`], which [`crate::code_dashboard`] wears as well.

use crate::dashboard::{self, Tab};
use crate::error::{Error, Result};
use crate::memory::api::{CatOptions, Memory, RecallOptions};
use crate::memory::fact::Fact;
use crate::memory::link::{self, Segment};
use crate::memory::path::{PathEntry, WikiPath};
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
pub type Shared = Arc<Mutex<Memory>>;

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
        Err(err) => dashboard::error_page(StatusCode::BAD_REQUEST, Tab::Wiki, &err.to_string()),
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
        Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Wiki,
                    &err.to_string(),
                );
            }
    };

    let mut list = String::new();
    for hit in &hits {
        let tags = match memory.effective_tags(hit.fact.id) {
            Ok(tags) => tags,
            Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Wiki,
                    &err.to_string(),
                );
            }
        };
        list.push_str(&format!(
            "<li><a class=\"fact-where\" href=\"{}\">{}</a><p class=\"fact-content\">{}</p>{}</li>",
            dashboard::escape(hit.fact.path.as_str()),
            dashboard::escape(hit.fact.path.as_str()),
            render_content(&hit.fact.content),
            // The strength is the one that the fact had when the search found
            // it. The recall that follows lifts it.
            fact_meta(hit.signal_strength, hit.fact.created_at, &tags)
        ));
    }
    let results = if list.is_empty() {
        "<p class=\"empty-row\">No facts found.</p>".to_string()
    } else {
        format!("<ul class=\"facts\">{list}</ul>")
    };

    let title = if query.q.trim().is_empty() {
        "search".to_string()
    } else {
        format!("search: {}", query.q)
    };
    let body = format!(
        "{header}{search}<div class=\"card\">{results}</div>",
        header = search_header(&query.q, hits.len()),
        search = dashboard::search_bar(
            "/search",
            &query.q,
            "recall — search facts by word or meaning",
            false,
            "⌘K",
            None,
        ),
    );
    Html(dashboard::document(&title, &body, Tab::Wiki)).into_response()
}

/// Builds the header of the search page: a label, the query, and the count.
fn search_header(query: &str, count: usize) -> String {
    let title = if query.trim().is_empty() {
        "Every fact".to_string()
    } else {
        format!("“{query}”")
    };
    format!(
        "<div class=\"page-header\"><p class=\"label\">Search</p>\
         <h1 class=\"path-title\">{}</h1><p class=\"meta-line\">{}</p></div>",
        dashboard::escape(&title),
        dashboard::plural(count as u64, "fact", "facts")
    )
}

/// Builds the page of one path.
fn render_page(state: &Shared, path: WikiPath) -> Response {
    let mut memory = state.lock().expect("the memory lock is never poisoned");

    let listing = match memory.ls(&path) {
        Ok(listing) => listing,
        Err(Error::PathNotFound(_)) => {
            return dashboard::error_page(
                StatusCode::NOT_FOUND,
                Tab::Wiki,
                &format!("{path} holds nothing yet"),
            );
        }
        Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Wiki,
                    &err.to_string(),
                );
            }
    };

    // Reading a page is not a recall: the page shows each fact of the path at
    // once, so it says nothing about which fact was useful.
    let mut facts = match memory.cat(&path, CatOptions::default()) {
        Ok(facts) => facts,
        Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Wiki,
                    &err.to_string(),
                );
            }
    };
    // `cat` reads oldest first, like a document read top to bottom. The page
    // reads like a feed, so it shows the newest fact first.
    facts.reverse();

    let tags = match tags_of(&memory, &facts) {
        Ok(tags) => tags,
        Err(err) => {
                return dashboard::error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Tab::Wiki,
                    &err.to_string(),
                );
            }
    };

    let now = Utc::now();
    let path_strength = strength(&facts, now);
    let header = page_header(
        &path,
        listing.fact_count,
        listing.subtree_fact_count,
        listing.children.len() as u64,
        path_strength,
    );

    let search = dashboard::search_bar(
        "/search",
        "",
        "recall — search facts by word or meaning",
        false,
        "⌘K",
        None,
    );
    let body = if facts.is_empty() && listing.children.is_empty() {
        format!("{header}{search}<div class=\"empty-state\">This path holds nothing.</div>")
    } else {
        format!(
            "{header}{search}<div class=\"body\">{}{}</div>",
            facts_panel(&facts, &tags, now),
            sidebar_panel(&listing.children, facts.len(), path_strength),
        )
    };

    Html(dashboard::document(path.as_str(), &body, Tab::Wiki)).into_response()
}

/// Builds the trail, the path title and the metadata line at the top of a
/// page.
fn page_header(
    path: &WikiPath,
    fact_count: u64,
    subtree_fact_count: u64,
    child_count: u64,
    strength: Option<f64>,
) -> String {
    format!(
        "<div class=\"page-header\">{}<h1 class=\"path-title\">{}</h1>{}</div>",
        breadcrumbs(path),
        dashboard::escape(path.as_str()),
        metadata(fact_count, subtree_fact_count, child_count, strength)
    )
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
        dashboard::plural(fact_count, "fact", "facts"),
        format!("{} total", dashboard::plural(subtree_fact_count, "fact", "facts")),
        dashboard::plural(child_count, "child", "children"),
    ];
    if let Some(strength) = strength {
        parts.push(format!("signal {strength:.3}"));
    }
    format!("<p class=\"meta-line\">{}</p>", parts.join(" · "))
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

/// Builds the "Facts" column: its header and the card that holds the list.
///
/// A path with no fact still shows the card, with a line that says so, so
/// that the column never collapses to nothing next to the sidebar.
fn facts_panel(facts: &[Fact], tags: &[TagSet], now: DateTime<Utc>) -> String {
    let list = render_facts(facts, tags, now);
    let list = if list.is_empty() {
        "<p class=\"empty-row\">No facts yet.</p>".to_string()
    } else {
        list
    };
    format!(
        "<section class=\"facts-col\"><div class=\"facts-head\">\
         <p class=\"label\">Facts</p><span class=\"hint\">newest first</span></div>\
         <div class=\"card\">{list}</div></section>"
    )
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
            "<li><p class=\"fact-content\">{}</p>{}</li>",
            render_content(&fact.content),
            fact_meta(fact.signal.strength_at(now), fact.created_at, tags)
        ));
    }
    html.push_str("</ul>");
    html
}

/// Builds the line that says what one fact is.
///
/// The strength goes from 1.000 for a fact that somebody just read to 0.000
/// for a fact that the memory almost lost. The date is the day on which
/// somebody wrote the fact. Each tag that decides who reads the fact, which
/// include the tags that the fact takes from the paths above it, gets its own
/// badge. A fact with no tag stops at the day.
fn fact_meta(strength: f64, created_at: DateTime<Utc>, tags: &TagSet) -> String {
    let mut html = format!("signal {strength:.3} · {}", created_at.format("%Y-%m-%d"));
    if !tags.is_empty() {
        html.push_str(" · ");
        for tag in tags.iter() {
            html.push_str(&format!(
                "<span class=\"tag\">{}</span>",
                dashboard::escape(&tag.to_string())
            ));
        }
    }
    format!("<div class=\"fact-meta\">{html}</div>")
}

/// Builds the sidebar: the "Below" card, the signal card, or neither.
///
/// A path with no child below it has no "Below" card. A path with no fact has
/// no signal, so it has no signal card either. With nothing to show, the
/// sidebar itself is absent, and the facts column takes the row alone.
fn sidebar_panel(children: &[PathEntry], fact_count: usize, strength: Option<f64>) -> String {
    let mut cards = String::new();
    if !children.is_empty() {
        cards.push_str(&below_card(children));
    }
    if let Some(strength) = strength {
        cards.push_str(&signal_card(strength, fact_count as u64));
    }
    if cards.is_empty() {
        return String::new();
    }
    format!("<aside class=\"sidebar-col\">{cards}</aside>")
}

/// Builds the card that lists the paths one step below the current path.
fn below_card(children: &[PathEntry]) -> String {
    let mut html = String::from("<div class=\"card\"><div class=\"below-head\">Below</div>");
    for entry in children {
        let name = entry.path.segment().unwrap_or("/");
        html.push_str(&format!(
            "<a class=\"below-row\" href=\"{}\"><span class=\"below-name\">{}</span>\
             <span class=\"below-count\">{} · {} total</span></a>",
            dashboard::escape(entry.path.as_str()),
            dashboard::escape(name),
            entry.fact_count,
            entry.subtree_fact_count
        ));
    }
    html.push_str("</div>");
    html
}

/// Builds the card that shows the mean signal of the path.
fn signal_card(strength: f64, fact_count: u64) -> String {
    format!(
        "<div class=\"signal-card\"><div class=\"signal-label\">Path signal</div>\
         <div class=\"signal-value\">{strength:.3}</div>\
         <div class=\"signal-caption\">mean freshness of {}</div></div>",
        dashboard::plural(fact_count, "fact", "facts")
    )
}

/// Turns the content of a fact into HTML, with the links followed.
pub fn render_content(content: &str) -> String {
    let mut html = String::new();
    for segment in link::parse(content) {
        match segment {
            Segment::Text(text) => html.push_str(&dashboard::escape(text)),
            Segment::Link { target, label } => html.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                dashboard::escape(target.as_str()),
                dashboard::escape(label)
            )),
            // A pair of brackets that is not a path stays as it was written.
            Segment::Broken(text) => html.push_str(&dashboard::escape(&format!("[[{text}]]"))),
        }
    }
    html
}

/// Builds the trail from the root down to the path.
///
/// Every step but the last links to its own page. The last step is the page
/// itself, so it carries no link.
fn breadcrumbs(path: &WikiPath) -> String {
    let mut html = String::from("<nav class=\"trail\">");
    let chain = path.ancestry();
    for (index, step) in chain.iter().enumerate() {
        if index > 0 {
            html.push_str("<span class=\"sep\">/</span>");
        }
        let label = step.segment().unwrap_or("root");
        if index + 1 == chain.len() {
            html.push_str(&format!(
                "<span class=\"current\">{}</span>",
                dashboard::escape(label)
            ));
        } else {
            html.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                dashboard::escape(step.as_str()),
                dashboard::escape(label)
            ));
        }
    }
    html.push_str("</nav>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tag::Tag;
    use crate::memory::{FactId, PathId, Signal};
    use ulid::Ulid;

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
        assert!(dashboard::escape("/\"onload=\"").contains("&quot;"));
    }

    #[test]
    fn the_trail_links_each_step_but_the_last() {
        let html = breadcrumbs(&WikiPath::parse("/a/b").unwrap());
        assert!(html.contains("<a href=\"/\">root</a>"));
        assert!(html.contains("<a href=\"/a\">a</a>"));
        assert!(html.contains("<span class=\"current\">b</span>"));
        assert!(!html.contains("<a href=\"/a/b\">"));
    }

    #[test]
    fn the_trail_of_the_root_holds_the_root_only() {
        let html = breadcrumbs(&WikiPath::root());
        assert!(html.contains("<span class=\"current\">root</span>"));
        assert!(!html.contains("<a href"));
    }

    #[test]
    fn the_metadata_holds_the_counts_and_the_signal() {
        assert_eq!(
            metadata(3, 5, 2, Some(0.8127)),
            "<p class=\"meta-line\">3 facts · 5 facts total · 2 children · signal 0.813</p>"
        );
    }

    #[test]
    fn the_metadata_uses_the_singular_for_one() {
        assert_eq!(
            metadata(1, 1, 1, None),
            "<p class=\"meta-line\">1 fact · 1 fact total · 1 child</p>"
        );
    }

    #[test]
    fn a_path_with_no_fact_has_no_signal() {
        assert_eq!(
            metadata(0, 0, 4, None),
            "<p class=\"meta-line\">0 facts · 0 facts total · 4 children</p>"
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
            html.contains("<div class=\"fact-meta\">signal 1.000 · 2026-07-28</div>"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"fact-meta\">signal 0.000 · 2026-07-28</div>"),
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
            fact_meta(1.0, written, &tags),
            "<div class=\"fact-meta\">signal 1.000 · 2026-07-28 · \
             <span class=\"tag\">kind=note</span><span class=\"tag\">visibility=private</span></div>"
        );
    }

    #[test]
    fn a_fact_with_no_tag_shows_the_signal_and_the_date_only() {
        let written = "2026-07-28T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            fact_meta(0.5, written, &TagSet::new()),
            "<div class=\"fact-meta\">signal 0.500 · 2026-07-28</div>"
        );
    }

    #[test]
    fn a_path_with_no_fact_holds_no_list() {
        assert_eq!(render_facts(&[], &[], Utc::now()), "");
    }

    #[test]
    fn facts_panel_shows_a_message_when_there_is_no_fact() {
        let html = facts_panel(&[], &[], Utc::now());
        assert!(html.contains("No facts yet."), "{html}");
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

    fn entry(path: &str, fact_count: u64, subtree_fact_count: u64) -> PathEntry {
        PathEntry {
            path: WikiPath::parse(path).unwrap(),
            fact_count,
            subtree_fact_count,
            child_count: 0,
        }
    }

    #[test]
    fn below_card_links_to_each_child_with_its_counts() {
        let children = [entry("/memory", 3, 12), entry("/server", 0, 1)];
        let html = below_card(&children);

        assert!(html.contains("<div class=\"below-head\">Below</div>"));
        assert!(html.contains(
            "<a class=\"below-row\" href=\"/memory\"><span class=\"below-name\">memory</span>\
             <span class=\"below-count\">3 · 12 total</span></a>"
        ));
        assert!(html.contains("<span class=\"below-count\">0 · 1 total</span>"));
    }

    #[test]
    fn signal_card_shows_the_mean_and_the_caption() {
        let html = signal_card(0.8127, 7);
        assert!(html.contains("<div class=\"signal-value\">0.813</div>"));
        assert!(html.contains("mean freshness of 7 facts"));

        let html = signal_card(1.0, 1);
        assert!(html.contains("mean freshness of 1 fact"));
    }

    #[test]
    fn the_sidebar_is_empty_when_there_is_nothing_to_show() {
        assert_eq!(sidebar_panel(&[], 0, None), "");
    }

    #[test]
    fn the_sidebar_holds_only_the_cards_that_apply() {
        let children = [entry("/a", 1, 1)];

        let html = sidebar_panel(&children, 3, Some(0.5));
        assert!(html.contains("below-head") && html.contains("signal-card"));

        let html = sidebar_panel(&[], 3, Some(0.5));
        assert!(!html.contains("below-head") && html.contains("signal-card"));

        let html = sidebar_panel(&children, 0, None);
        assert!(html.contains("below-head") && !html.contains("signal-card"));
    }

}
