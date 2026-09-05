//! Shared chrome for `embornal dashboard`.
//!
//! [`crate::wiki`] and [`crate::code_dashboard`] each answer their own routes,
//! but they wear the same page: the same header, the same fonts, the same
//! colors. This module holds that shared frame, so that the two never drift
//! apart. Colors and type mirror the landing page,
//! `site/themes/embornal/assets/css/main.css`.

use crate::code::CodeIndex;
use crate::error::{Error, Result};
use crate::memory::api::Memory;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use std::sync::{Arc, Mutex};

/// The port that `embornal dashboard` listens on by default.
pub const DASHBOARD_PORT: u16 = 1337;

/// Starts the dashboard and blocks until it stops.
///
/// It answers two kinds of route behind one port: the wiki at `/`, and the
/// code browser at `/code`. The two never touch each other's file — the
/// memory and the code index are locked apart, so a slow read of one never
/// blocks the other.
pub fn serve(memory: Memory, code: CodeIndex, collection: String, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Serve(err.to_string()))?;

    runtime.block_on(async move {
        let wiki_state: crate::wiki::Shared = Arc::new(Mutex::new(memory));
        let code_state = crate::code_dashboard::CodeState {
            index: Arc::new(Mutex::new(code)),
            collection: Arc::new(collection),
        };

        let app = crate::wiki::router(wiki_state).merge(crate::code_dashboard::router(code_state));

        let address = format!("0.0.0.0:{port}");
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|err| Error::Serve(format!("cannot listen on {address}: {err}")))?;

        println!("the dashboard is at http://localhost:{port}");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown())
            .await
            .map_err(|err| Error::Serve(err.to_string()))
    })
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Which section of the dashboard a page belongs to. Decides which tab in
/// the header is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Wiki,
    Code,
}

/// Returns the count with the word that goes with it.
pub fn plural(count: u64, one: &str, many: &str) -> String {
    let word = if count == 1 { one } else { many };
    format!("{count} {word}")
}

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

/// Builds a search bar.
///
/// `active` highlights it in purple, for a page that already holds a query.
/// `hint` labels the badge at the right: `⌘K` focuses the bar, `esc` clears
/// it. `escape_to`, given with the `esc` hint, is where the page goes when a
/// reader presses it while the bar holds focus.
pub fn search_bar(
    action: &str,
    value: &str,
    placeholder: &str,
    active: bool,
    hint: &str,
    escape_to: Option<&str>,
) -> String {
    let class = if active { "search is-active" } else { "search" };
    let escape_attr = match escape_to {
        Some(target) => format!(" data-escape=\"{}\"", escape(target)),
        None => String::new(),
    };
    format!(
        "<form class=\"{class}\" action=\"{action}\" method=\"get\"{escape_attr}>\
         <svg width=\"18\" height=\"18\" viewBox=\"0 0 24 24\" aria-hidden=\"true\">\
         <circle cx=\"11\" cy=\"11\" r=\"7\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\"/>\
         <path d=\"M21 21l-4.3-4.3\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\"/>\
         </svg>\
         <input type=\"search\" name=\"q\" value=\"{value}\" aria-label=\"Search\" \
         placeholder=\"{placeholder}\" autofocus>\
         <span class=\"kbd\">{hint}</span></form>",
        value = escape(value),
        placeholder = escape(placeholder),
    )
}

/// Builds a generic error page: a label that names the kind of failure and
/// the message underneath it, inside the frame of whichever section asked.
pub fn error_page(status: StatusCode, tab: Tab, message: &str) -> Response {
    let title = match status {
        StatusCode::NOT_FOUND => "not found",
        StatusCode::BAD_REQUEST => "bad path",
        _ => "error",
    };
    let body = format!(
        "<div class=\"page-header\"><p class=\"label\">{}</p></div>\
         <div class=\"error-state\">{}</div>",
        escape(title),
        escape(message)
    );
    (status, Html(document(title, &body, tab))).into_response()
}

/// The header that every page shares: the wordmark, the section tabs, and
/// the local status.
fn topbar(tab: Tab) -> String {
    let (wiki, code) = match tab {
        Tab::Wiki => ("tab is-active", "tab"),
        Tab::Code => ("tab", "tab is-active"),
    };
    format!(
        "<header class=\"topbar\"><div class=\"brand\">\
         <span class=\"brand-mark\">e</span>\
         <span class=\"brand-word\"><a href=\"/\">embornal</a></span>\
         </div>\
         <nav class=\"tabs\"><a class=\"{wiki}\" href=\"/\">Wiki</a>\
         <a class=\"{code}\" href=\"/code\">Code</a></nav>\
         <div class=\"status\"><span class=\"status-dot\"></span>served locally</div>\
         </header>"
    )
}

/// Focuses the search box on Ctrl-K / Cmd-K, which every search bar
/// advertises. A bar that also advertises `esc` sends the page to the place
/// that its `data-escape` attribute names.
const SCRIPT: &str = "\
document.addEventListener('keydown', function (event) {\
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {\
    var input = document.querySelector('.search input');\
    if (input) { event.preventDefault(); input.focus(); }\
  }\
  if (event.key === 'Escape') {\
    var form = document.activeElement && document.activeElement.closest('form.search[data-escape]');\
    if (form) { window.location.href = form.dataset.escape; }\
  }\
});";

/// Wraps the body of a page in the shared frame.
pub fn document(title: &str, body: &str, tab: Tab) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} — embornal</title>\
         <link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\
         <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\
         <link href=\"https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=Space+Grotesk:wght@400;600;700&display=swap\" rel=\"stylesheet\">\
         <style>{STYLE}</style></head>\
         <body>{topbar}<main class=\"page\">{body}</main>\
         <script>{SCRIPT}</script></body></html>",
        title = escape(title),
        topbar = topbar(tab)
    )
}

const STYLE: &str = "
:root {
  --white: #fff;
  --ink: #10111f;
  --purple: #7041c6;
  --lavender: #bba5ee;
  --mist: #f4f2fa;
  --slate: #5f6170;
  --dim: #c9cad2;
  --mono: 'IBM Plex Mono', ui-monospace, SFMono-Regular, Consolas, monospace;
  --sans: 'Space Grotesk', Inter, system-ui, sans-serif;
  --border: color-mix(in srgb, var(--ink) 12%, transparent);
  --border-card: color-mix(in srgb, var(--ink) 14%, transparent);
  --border-row: color-mix(in srgb, var(--ink) 10%, transparent);
  --border-row-soft: color-mix(in srgb, var(--ink) 8%, transparent);
  --border-search: color-mix(in srgb, var(--ink) 16%, transparent);
}
* { box-sizing: border-box; }
body { margin: 0; color: var(--ink); background: var(--mist); font-family: var(--sans); -webkit-font-smoothing: antialiased; }
a { color: inherit; text-decoration: none; }

.topbar { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; row-gap: 8px; height: 72px; padding: 0 40px; background: var(--white); border-bottom: 1px solid var(--border); }
.brand { display: flex; align-items: center; gap: 14px; }
.brand-mark { display: flex; align-items: center; justify-content: center; width: 34px; height: 34px; flex-shrink: 0; border-radius: 8px; background: var(--ink); color: var(--white); font: 700 19px/24px var(--sans); }
.brand-word { font: 700 18px/22px var(--sans); letter-spacing: -.02em; }
.brand-badge { padding: 4px 9px; border-radius: 5px; background: var(--mist); color: var(--slate); font: 500 11px/14px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.tabs { display: flex; align-items: center; gap: 4px; }
.tab { padding: 9px 16px; border-radius: 8px; color: var(--slate); font: 600 15px/18px var(--sans); }
.tab.is-active { background: var(--mist); color: var(--ink); }
.status { display: flex; align-items: center; gap: 8px; color: var(--slate); font: 400 12px/16px var(--mono); }
.status-dot { width: 7px; height: 7px; flex-shrink: 0; border-radius: 50%; background: var(--purple); }

.page { max-width: 1440px; margin: 0 auto; padding: 0 40px 48px; }
.page-header { display: flex; flex-direction: column; gap: 14px; padding-top: 36px; }
.trail { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; font: 400 13px/16px var(--mono); }
.trail a { color: var(--purple); }
.trail a:hover { text-decoration: underline; }
.trail .sep { color: var(--slate); }
.trail .current { color: var(--ink); font-weight: 600; }
.path-title { margin: 0; color: var(--ink); font: 600 34px/42px var(--mono); letter-spacing: -.01em; word-break: break-word; }
.meta-line { margin: 0; color: var(--slate); font: 400 13px/16px var(--mono); }
.label { margin: 0; color: var(--purple); font: 600 12px/16px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.hint { color: var(--slate); font: 400 12px/16px var(--mono); }

.search { display: flex; align-items: center; gap: 12px; margin-top: 24px; padding: 13px 16px; border-radius: 12px; border: 1.5px solid var(--border-search); background: var(--white); color: var(--slate); }
.search svg { flex-shrink: 0; color: inherit; }
.search.is-active { border-color: var(--purple); color: var(--purple); }
.search input { flex: 1; min-width: 0; border: 0; outline: 0; background: none; color: var(--ink); font: 400 14px/18px var(--mono); }
.search input::placeholder { color: var(--slate); }
.kbd { flex-shrink: 0; padding: 4px 8px; border-radius: 5px; background: var(--mist); color: var(--slate); font: 400 11px/14px var(--mono); }

.card { background: var(--white); border: 1.5px solid var(--border-card); border-radius: 14px; overflow: hidden; }
.empty-row { padding: 24px 20px; color: var(--slate); font: 400 15px/23px var(--sans); font-style: italic; }
.empty-state, .error-state { margin-top: 28px; padding: 48px 20px; border: 1.5px solid var(--border-card); border-radius: 14px; background: var(--white); color: var(--slate); font: 400 15px/23px var(--sans); font-style: italic; text-align: center; }
.empty-state code, .empty-row code { font-family: var(--mono); font-style: normal; padding: 2px 6px; border-radius: 4px; background: var(--mist); }
.empty-note { margin: 0; color: var(--slate); font: 400 15px/24px var(--sans); font-style: italic; }
.title-row { display: flex; align-items: center; gap: 14px; }

/* -- the wiki -- */

ul.facts { list-style: none; margin: 0; padding: 0; }
ul.facts li { display: flex; flex-direction: column; gap: 9px; padding: 18px 20px; border-bottom: 1px solid var(--border-row); }
ul.facts li:last-child { border-bottom: 0; }
.fact-where { align-self: flex-start; color: var(--purple); font: 400 12px/16px var(--mono); }
.fact-content { margin: 0; color: var(--ink); font: 400 15px/23px var(--sans); }
.fact-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; color: var(--slate); font: 400 12px/16px var(--mono); }
.tag { padding: 2px 7px; border-radius: 5px; background: var(--mist); }

.body { display: flex; align-items: flex-start; gap: 24px; margin-top: 28px; }
.facts-col { flex: 2 1 520px; min-width: 0; }
.sidebar-col { flex: 1 1 320px; max-width: 400px; display: flex; flex-direction: column; gap: 20px; }
.facts-head { display: flex; align-items: center; justify-content: space-between; margin: 0 0 12px; }

.below-head { padding: 16px 18px 12px; border-bottom: 1px solid var(--border-row); color: var(--purple); font: 600 12px/16px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.below-row { display: flex; align-items: center; justify-content: space-between; padding: 13px 18px; border-bottom: 1px solid var(--border-row-soft); }
.below-row:last-child { border-bottom: 0; }
.below-name { color: var(--purple); font: 400 14px/18px var(--mono); }
.below-count { flex-shrink: 0; color: var(--slate); font: 400 12px/16px var(--mono); }

.signal-card { display: flex; flex-direction: column; gap: 6px; padding: 20px 22px; border-radius: 14px; background: var(--ink); box-shadow: 8px 8px 0 var(--lavender); }
.signal-label { color: var(--lavender); font: 600 11px/14px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.signal-value { color: var(--white); font: 700 40px/48px var(--sans); letter-spacing: -.02em; }
.signal-caption { color: var(--dim); font: 400 12px/16px var(--mono); }

/* -- the code browser -- */

.kind-chip { flex-shrink: 0; padding: 4px 9px; border-radius: 5px; background: var(--mist); color: var(--slate); font: 600 11px/14px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.title-badge { padding: 5px 10px; border-radius: 6px; background: var(--purple); color: var(--white); font: 600 11px/14px var(--mono); letter-spacing: .08em; text-transform: uppercase; }

.tree-col { flex: 0 0 340px; min-width: 0; }
.tree-head { display: flex; align-items: center; justify-content: space-between; margin: 0 0 12px; }
.tree-card { padding: 8px 0; font: 400 13px/16px var(--mono); }
details.tree-dir > summary { display: flex; align-items: center; gap: 8px; padding: 7px 14px; list-style: none; cursor: pointer; }
details.tree-dir > summary::-webkit-details-marker { display: none; }
details.tree-dir > summary::marker { content: ''; }
.tree-toggle { width: 14px; flex-shrink: 0; text-align: center; color: var(--slate); font-size: 10px; }
summary .tree-toggle::before { content: '▸'; }
details.tree-dir[open] > summary .tree-toggle::before { content: '▾'; }
.tree-dot { width: 7px; height: 7px; flex-shrink: 0; border-radius: 50%; background: var(--purple); }
.tree-dot.is-waiting { background: none; border: 1.5px solid var(--slate); }
.tree-name { color: var(--ink); }
.tree-name.is-dir { font-weight: 600; }
.tree-children { margin-left: 7px; padding-left: 10px; border-left: 1px solid var(--border-row-soft); }
.tree-file { display: flex; align-items: center; gap: 8px; padding: 7px 14px; }
.tree-file.is-current { background: var(--mist); border-left: 3px solid var(--purple); padding-left: 11px; }
.tree-file.is-current .tree-name { color: var(--purple); font-weight: 600; }
.tree-legend { display: flex; align-items: center; gap: 16px; margin-top: 12px; color: var(--slate); font: 400 12px/16px var(--mono); }
.tree-legend span { display: inline-flex; align-items: center; gap: 6px; }

.detail-col { flex: 1 1 640px; min-width: 0; }
.overview-card { display: flex; flex-direction: column; gap: 10px; padding: 20px 22px; }
.overview-meta { display: flex; align-items: center; gap: 10px; }
.overview-path { color: var(--slate); font: 400 13px/16px var(--mono); }
.overview-title { color: var(--ink); font: 600 19px/27px var(--sans); }
.overview-body { color: var(--slate); font: 400 15px/24px var(--sans); }

.definitions-head { display: flex; align-items: center; justify-content: space-between; margin: 22px 0 12px; }
ul.definitions { list-style: none; margin: 0; padding: 0; }
a.definition-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 13px 20px; border-bottom: 1px solid var(--border-row); color: var(--ink); }
ul.definitions li:last-child a.definition-row { border-bottom: 0; }
a.definition-row.is-current { background: var(--mist); border-left: 3px solid var(--purple); padding-left: 17px; }
.definition-name { display: flex; align-items: center; gap: 10px; min-width: 0; }
.definition-name .name { font: 400 14px/18px var(--mono); }
a.definition-row.is-current .definition-name .name { color: var(--purple); font-weight: 600; }
.definition-line { flex-shrink: 0; color: var(--slate); font: 400 12px/16px var(--mono); }

.selected-card { margin-top: 20px; padding: 22px 24px; border: 1.5px solid var(--purple); border-radius: 14px; background: var(--white); display: flex; flex-direction: column; gap: 12px; }
.selected-head { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
.selected-summary { margin: 0; color: var(--ink); font: 600 18px/26px var(--sans); }
.selected-description { margin: 0; color: var(--slate); font: 400 15px/24px var(--sans); }
.selected-footer { display: flex; align-items: center; gap: 8px; padding-top: 12px; margin-top: 2px; border-top: 1px solid var(--border-row); color: var(--slate); font: 400 12px/16px var(--mono); }

.eyebrow-line { margin: 0; color: var(--purple); font: 600 13px/16px var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.query-title { margin: 6px 0 0; color: var(--ink); font: 700 32px/40px var(--sans); letter-spacing: -.02em; word-break: break-word; }

ul.results { list-style: none; margin: 0; padding: 0; }
li.result-row { display: flex; flex-direction: column; gap: 8px; padding: 18px 22px; border-bottom: 1px solid var(--border-row); }
li.result-row:last-child { border-bottom: 0; }
.result-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.result-name { display: flex; align-items: center; gap: 10px; min-width: 0; }
.result-name .name { color: var(--ink); font: 600 15px/18px var(--mono); }
.result-score { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
.score-track { width: 56px; height: 4px; flex-shrink: 0; border-radius: 2px; overflow: hidden; background: var(--mist); }
.score-fill { height: 100%; background: var(--purple); }
.result-where { color: var(--slate); font: 400 12px/16px var(--mono); }
.result-summary { color: var(--slate); font: 400 14px/22px var(--sans); }

@media (max-width: 960px) {
  .body { flex-direction: column; }
  .sidebar-col { max-width: none; }
  .tree-col { flex-basis: auto; width: 100%; }
}
@media (max-width: 640px) {
  .topbar, .page { padding-left: 20px; padding-right: 20px; }
  .tabs { display: none; }
  .path-title { font-size: 26px; line-height: 32px; }
  .query-title { font-size: 24px; line-height: 30px; }
}
";

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
    fn the_document_carries_the_title_once_escaped() {
        let html = document("/a & b", "<p>x</p>", Tab::Wiki);
        assert!(html.contains("<title>/a &amp; b — embornal</title>"));
        assert!(html.contains("<p>x</p>"));
        assert!(!html.contains("/a & b"));
    }

    #[test]
    fn the_active_tab_carries_its_own_class_and_the_other_does_not() {
        let wiki = document("t", "", Tab::Wiki);
        assert!(wiki.contains("<a class=\"tab is-active\" href=\"/\">Wiki</a>"));
        assert!(wiki.contains("<a class=\"tab\" href=\"/code\">Code</a>"));

        let code = document("t", "", Tab::Code);
        assert!(code.contains("<a class=\"tab\" href=\"/\">Wiki</a>"));
        assert!(code.contains("<a class=\"tab is-active\" href=\"/code\">Code</a>"));
    }

    #[test]
    fn an_active_search_bar_carries_its_class_and_its_escape_target() {
        let plain = search_bar("/search", "", "recall", false, "⌘K", None);
        assert!(plain.contains("class=\"search\""));
        assert!(!plain.contains("data-escape"));

        let active = search_bar(
            "/code/search",
            "token",
            "recall",
            true,
            "esc",
            Some("/code"),
        );
        assert!(active.contains("class=\"search is-active\""));
        assert!(active.contains("data-escape=\"/code\""));
        assert!(active.contains("value=\"token\""));
    }

    #[test]
    fn an_error_page_names_its_status() {
        let response = error_page(StatusCode::NOT_FOUND, Tab::Code, "gone");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
