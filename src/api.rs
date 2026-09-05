//! The server that many people share.
//!
//! `embornal serve` puts the memory of this machine behind HTTP. The server
//! runs the same [`Memory`] that a memory on one machine runs, so a fact that
//! goes through the server obeys exactly the rules that a fact written here
//! obeys.
//!
//! Every request carries a token, and the token says which subject asks. The
//! client cannot say that: a subject that a client sent would let anybody read
//! the facts of anybody. See [`crate::memory::token`].
//!
//! The wiki in [`crate::wiki`] is the other server. It reads, it has no login,
//! and it answers one person.

use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::api::{CatOptions, Memory, RecallOptions, TreeOptions};
use crate::memory::backend::MemoryApi;
use crate::memory::db::SCHEMA_VERSION;
use crate::memory::fact::{FactId, NewFact, OrderBy};
use crate::memory::path::WikiPath;
use crate::memory::token;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, middleware};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// The port that `serve` listens on.
pub const SERVE_PORT: u16 = 1338;

/// Where a client sends its requests.
pub const API_ROOT: &str = "/api/v1";

/// The header that carries the token, and the word before it.
pub const AUTH_HEADER: &str = "authorization";
pub const BEARER: &str = "Bearer ";

/// What the memory of this server holds.
///
/// One SQLite connection cannot serve two threads at once, and a [`Memory`]
/// speaks for one subject at a time, so every request takes the memory in
/// turn and points it at its own caller first.
///
/// This makes the server answer one request at a time. That is enough for the
/// handful of agents that one person or one team runs, and it is the one
/// place to change when it stops being enough.
pub struct ApiState {
    memory: Mutex<Memory>,
}

impl ApiState {
    pub fn new(memory: Memory) -> Self {
        Self {
            memory: Mutex::new(memory),
        }
    }

    /// Runs `f` on the memory, as `subject` and as nobody else.
    ///
    /// Call this on a thread that may block. It waits for the lock, it talks
    /// to SQLite, and it can load the embedding model.
    fn as_subject<T>(
        &self,
        subject: &Subject,
        f: impl FnOnce(&mut Memory) -> Result<T>,
    ) -> Result<T> {
        let mut memory = self.lock();
        memory.set_subject(subject.clone())?;
        f(&mut memory)
    }

    /// Reads the subject that a token names.
    fn subject_of(&self, secret: &str) -> Result<Subject> {
        let memory = self.lock();
        token::authenticate(memory.database().conn(), secret)
    }

    /// Takes the memory, even after a request stopped while it held it.
    ///
    /// One request that fails must not shut the whole server. The memory
    /// itself stays whole: a transaction that stops rolls back, and the guard
    /// is written again before each request.
    fn lock(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// Runs the work of one request on a thread that may block, and turns the
/// answer into a response.
///
/// The memory is not async: it waits for a lock, it talks to SQLite, and it
/// can run the embedding model for hundreds of milliseconds. More than that,
/// building the access guard starts a small runtime of its own, and a thread
/// that already drives one cannot do that. So the work leaves the threads
/// that carry the requests.
async fn work<T, F>(state: Shared, caller: Caller, f: F) -> Response
where
    T: Serialize + Send + 'static,
    F: FnOnce(&mut Memory) -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || state.as_subject(&caller.0, f)).await {
        Ok(answer) => reply(answer),
        Err(err) => fail(Error::Serve(format!("the request failed: {err}"))),
    }
}

type Shared = Arc<ApiState>;

/// Starts the server and blocks until it stops.
pub fn serve(memory: Memory, address: SocketAddr) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| Error::Serve(err.to_string()))?;

    runtime.block_on(async move {
        let state = Arc::new(ApiState::new(memory));
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|err| Error::Serve(format!("cannot listen on {address}: {err}")))?;

        println!("the memory is at http://{address}{API_ROOT}");
        axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown())
            .await
            .map_err(|err| Error::Serve(err.to_string()))
    })
}

/// Builds the routes.
pub fn router(state: Shared) -> Router {
    Router::new()
        .route(&route("/whoami"), get(whoami))
        .route(&route("/facts"), post(store))
        .route(&route("/ls"), get(ls))
        .route(&route("/tree"), get(tree))
        .route(&route("/cat"), get(cat))
        .route(&route("/recall"), get(recall))
        .route(&route("/tags"), get(tags))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

fn route(tail: &str) -> String {
    format!("{API_ROOT}{tail}")
}

// ---------------------------------------------------------------------------
// Who asks
// ---------------------------------------------------------------------------

/// The subject that the token of this request names.
///
/// The handlers read this and nothing else. There is no other way into a
/// handler, so no handler can take the word of a client for who it is.
#[derive(Debug, Clone)]
struct Caller(Subject);

/// Reads the token of the request and puts the subject that it names into the
/// request.
///
/// A request with no token, or with a token that stopped, never reaches a
/// handler.
async fn authenticate(
    State(state): State<Shared>,
    mut request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let secret = match bearer(request.headers()) {
        Some(secret) => secret,
        None => {
            return fail(Error::Unauthorized(
                "this server needs an Authorization: Bearer header".to_string(),
            ));
        }
    };

    let found = tokio::task::spawn_blocking(move || state.subject_of(&secret)).await;
    match found {
        Ok(Ok(subject)) => {
            request.extensions_mut().insert(Caller(subject));
            next.run(request).await
        }
        Ok(Err(err)) => fail(err),
        Err(err) => fail(Error::Serve(format!("the request failed: {err}"))),
    }
}

/// Reads the token out of the headers.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTH_HEADER)?.to_str().ok()?;
    let secret = value.strip_prefix(BEARER)?.trim();
    if secret.is_empty() {
        return None;
    }
    Some(secret.to_string())
}

// ---------------------------------------------------------------------------
// What goes over the wire
// ---------------------------------------------------------------------------

/// What the server says about itself.
///
/// A client reads this once, so that it knows who its token makes it, and so
/// that a client and a server of different builds say so instead of failing
/// in a way that nobody can read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoAmI {
    pub subject: Subject,
    /// The version of the build that answers.
    pub version: String,
    /// The version of the schema that the memory holds.
    pub schema: i64,
    /// The values that a command uses when its flags say nothing.
    pub recall: crate::config::RecallConfig,
}

/// The one shape of a failure. A client reads this and says the same thing
/// that a memory on one machine would say.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: WikiPath,
}

#[derive(Debug, Deserialize)]
struct TreeQuery {
    path: WikiPath,
    #[serde(default)]
    dirs_only: bool,
}

#[derive(Debug, Deserialize)]
struct CatQuery {
    path: WikiPath,
    limit: Option<usize>,
    order_by: Option<OrderBy>,
    #[serde(default)]
    recall: bool,
}

#[derive(Debug, Deserialize)]
struct RecallQuery {
    q: Option<String>,
    limit: Option<usize>,
    under: Option<WikiPath>,
}

#[derive(Debug, Deserialize)]
struct TagQuery {
    fact: i64,
}

// ---------------------------------------------------------------------------
// The handlers
// ---------------------------------------------------------------------------

async fn whoami(State(state): State<Shared>, caller: Caller) -> Response {
    work(state, caller, |memory| {
        Ok(WhoAmI {
            subject: memory.subject().clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            schema: SCHEMA_VERSION,
            recall: memory.config().recall.clone(),
        })
    })
    .await
}

async fn store(
    State(state): State<Shared>,
    caller: Caller,
    Json(request): Json<NewFact>,
) -> Response {
    work(state, caller, move |memory| {
        MemoryApi::store(memory, request)
    })
    .await
}

async fn ls(
    State(state): State<Shared>,
    caller: Caller,
    Query(query): Query<PathQuery>,
) -> Response {
    work(state, caller, move |memory| {
        MemoryApi::ls(memory, &query.path)
    })
    .await
}

async fn tree(
    State(state): State<Shared>,
    caller: Caller,
    Query(query): Query<TreeQuery>,
) -> Response {
    work(state, caller, move |memory| {
        MemoryApi::tree(
            memory,
            &query.path,
            TreeOptions {
                dirs_only: query.dirs_only,
            },
        )
    })
    .await
}

async fn cat(
    State(state): State<Shared>,
    caller: Caller,
    Query(query): Query<CatQuery>,
) -> Response {
    work(state, caller, move |memory| {
        let order_by = query.order_by.unwrap_or(memory.config().recall.order_by);
        MemoryApi::cat(
            memory,
            &query.path,
            CatOptions {
                order_by,
                limit: query.limit,
                reinforce: query.recall,
            },
        )
    })
    .await
}

async fn recall(
    State(state): State<Shared>,
    caller: Caller,
    Query(query): Query<RecallQuery>,
) -> Response {
    work(state, caller, move |memory| {
        let limit = query.limit.unwrap_or(memory.config().recall.limit);
        MemoryApi::recall(
            memory,
            query.q.as_deref(),
            RecallOptions {
                limit,
                under: query.under,
                // A search that a person asked for is a real recall.
                reinforce: true,
            },
        )
    })
    .await
}

async fn tags(
    State(state): State<Shared>,
    caller: Caller,
    Query(query): Query<TagQuery>,
) -> Response {
    work(state, caller, move |memory| {
        MemoryApi::effective_tags(memory, FactId(query.fact))
    })
    .await
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// Turns the answer of the memory into the answer of the server.
fn reply<T: Serialize>(answer: Result<T>) -> Response {
    match answer {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => fail(err),
    }
}

/// Turns a failure into the answer of the server.
fn fail(err: Error) -> Response {
    (
        status_of(&err),
        Json(Failure {
            error: err.to_string(),
        }),
    )
        .into_response()
}

/// Says which code goes with a failure.
///
/// A refusal of the access rules is a 403, not a 404: the memory already
/// hides what a subject may not see, so a path that answers 404 holds nothing
/// for anybody.
pub fn status_of(err: &Error) -> StatusCode {
    match err {
        Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        Error::Denied { .. } => StatusCode::FORBIDDEN,
        Error::PathNotFound(_) => StatusCode::NOT_FOUND,
        Error::Path(_)
        | Error::Tag(_)
        | Error::Policy(_)
        | Error::BadArgument(_)
        | Error::ReservedTag(_)
        | Error::RootHoldsNoFacts
        | Error::EmptyContent => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Caller {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts.extensions.get::<Caller>().cloned().ok_or_else(|| {
            // The middleware puts this in, so a handler that reaches this
            // point is not behind the middleware. That is a mistake in the
            // routes, and it must never look like an open door.
            fail(Error::Unauthorized("no token was read".to_string()))
        })
    }
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn the_token_comes_out_of_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, HeaderValue::from_static("Bearer emb_a_b"));
        assert_eq!(bearer(&headers).as_deref(), Some("emb_a_b"));
    }

    #[test]
    fn a_header_that_is_not_a_bearer_token_gives_nothing() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);

        for value in ["", "emb_a_b", "Basic abc", "Bearer", "Bearer   "] {
            headers.insert(AUTH_HEADER, HeaderValue::from_str(value).unwrap());
            assert_eq!(bearer(&headers), None, "{value:?}");
        }
    }

    #[test]
    fn each_failure_carries_the_code_that_says_what_it_is() {
        assert_eq!(
            status_of(&Error::Unauthorized(String::new())),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(&Error::Denied {
                subject: "alice".to_string(),
                action: crate::memory::acl::Action::Read,
                path: "/a".to_string(),
            }),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(&Error::PathNotFound("/a".to_string())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status_of(&Error::EmptyContent), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_of(&Error::ReservedTag("owner".to_string())),
            StatusCode::BAD_REQUEST
        );
        // A failure that the client cannot mend is the fault of the server.
        assert_eq!(
            status_of(&Error::Serve(String::new())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn the_routes_all_sit_below_one_root() {
        assert_eq!(route("/whoami"), "/api/v1/whoami");
        assert!(API_ROOT.starts_with('/'));
        // The wiki and the memory do not share a port by accident.
        assert_ne!(SERVE_PORT, crate::dashboard::DASHBOARD_PORT);
    }
}
