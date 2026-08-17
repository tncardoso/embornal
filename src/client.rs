//! The client of a server.
//!
//! With a `server` section in the configuration, each command of `embornal
//! memory` goes here instead of to a file. The client holds no facts and no
//! embedding model: the server writes the facts, the server reads the access
//! rules, and the server turns a question into a vector.
//!
//! A client thus needs none of the weights, so a build with
//! `--no-default-features` is the one to install on a machine that only asks.
//!
//! When the server does not answer, the commands stop and say so. They never
//! fall back to a memory on this machine: two memories that drift apart with
//! nothing to bring them together again is worse than a command that fails.

use crate::config::{RecallConfig, ServerConfig};
use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::api::{CatOptions, Listing, RecallOptions, TreeNode, TreeOptions};
use crate::memory::backend::MemoryApi;
use crate::memory::fact::{Fact, FactId, NewFact, ScoredFact};
use crate::memory::path::WikiPath;
use crate::memory::tag::TagSet;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// A memory that lives on a server.
pub struct Client {
    /// The address of the API, with no slash at the end.
    root: String,
    /// The word that goes into the `Authorization` header.
    authorization: String,
    agent: ureq::Agent,
    /// What the server said about itself, once it was asked.
    known: Option<crate::api::WhoAmI>,
}

impl Client {
    /// Builds a client from the `server` section of the configuration.
    pub fn open(config: &ServerConfig) -> Result<Self> {
        let url = config.url.trim().trim_end_matches('/');
        if url.is_empty() {
            return Err(Error::BadArgument(
                "the `server` section of the configuration needs a `url`".to_string(),
            ));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(Error::BadArgument(format!(
                "the server url {url} must start with http:// or https://"
            )));
        }

        Ok(Self {
            root: format!("{url}{}", crate::api::API_ROOT),
            authorization: format!("{}{}", crate::api::BEARER, config.secret()?),
            // A refusal of the server is an answer, not a failure of the
            // wire: its body says what went wrong, and the client shows those
            // words. Without this the body goes and the reader learns only a
            // number.
            agent: ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .http_status_as_error(false)
                    .build(),
            ),
            known: None,
        })
    }

    /// Asks the server who this client is, one time.
    ///
    /// The answer also says which build answers. A client and a server of
    /// different builds still work, and the client says so, because a warning
    /// that a person can read beats a failure that nobody can.
    fn known(&mut self) -> Result<&crate::api::WhoAmI> {
        if self.known.is_none() {
            let said: crate::api::WhoAmI = self.get("/whoami", &[])?;
            if said.version != env!("CARGO_PKG_VERSION") {
                eprintln!(
                    "embornal: this is {}, and the server is {}",
                    env!("CARGO_PKG_VERSION"),
                    said.version
                );
            }
            self.known = Some(said);
        }
        Ok(self.known.as_ref().expect("the answer is here"))
    }

    /// Asks the server a question.
    fn get<T: DeserializeOwned>(&self, tail: &str, query: &[(&str, String)]) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{tail}", self.root))
            .header("authorization", &self.authorization);
        for (key, value) in query {
            request = request.query(*key, value);
        }
        self.read(request.call())
    }

    /// Sends the server something to write.
    fn post<B: serde::Serialize, T: DeserializeOwned>(&self, tail: &str, body: &B) -> Result<T> {
        self.read(
            self.agent
                .post(format!("{}{tail}", self.root))
                .header("authorization", &self.authorization)
                .send_json(body),
        )
    }

    /// Turns the answer of the server into the answer of the memory.
    ///
    /// A failure of the server comes back as the failure that a memory on
    /// this machine would give, so a person reads the same words either way.
    fn read<T: DeserializeOwned>(
        &self,
        answer: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T> {
        let mut response = match answer {
            Ok(response) => response,
            Err(err) => {
                return Err(Error::ServerUnreachable {
                    url: self.root.clone(),
                    reason: err.to_string(),
                });
            }
        };

        let status = response.status().as_u16();
        let body =
            response
                .body_mut()
                .read_to_string()
                .map_err(|err| Error::ServerUnreachable {
                    url: self.root.clone(),
                    reason: err.to_string(),
                })?;

        if status >= 400 {
            return Err(Error::Server {
                url: self.root.clone(),
                status,
                message: message_of(&body),
            });
        }
        serde_json::from_str(&body).map_err(|err| Error::Server {
            url: self.root.clone(),
            status,
            message: format!("the answer of the server cannot be read: {err}"),
        })
    }
}

/// Reads the words of a failure out of the body of an answer.
///
/// The server writes `{"error": "..."}`. A body that is not that shape comes
/// back as it is, because a message that a person can read is better than one
/// that this code threw away.
fn message_of(body: &str) -> String {
    #[derive(Deserialize)]
    struct Failure {
        error: String,
    }
    match serde_json::from_str::<Failure>(body) {
        Ok(failure) => failure.error,
        Err(_) => body.trim().to_string(),
    }
}

impl MemoryApi for Client {
    fn store(&mut self, request: NewFact) -> Result<Fact> {
        self.post("/facts", &request)
    }

    fn ls(&mut self, path: &WikiPath) -> Result<Listing> {
        self.get("/ls", &[("path", path.to_string())])
    }

    fn tree(&mut self, path: &WikiPath, options: TreeOptions) -> Result<TreeNode> {
        self.get(
            "/tree",
            &[
                ("path", path.to_string()),
                ("dirs_only", options.dirs_only.to_string()),
            ],
        )
    }

    fn cat(&mut self, path: &WikiPath, options: CatOptions) -> Result<Vec<Fact>> {
        let mut query = vec![
            ("path", path.to_string()),
            ("order_by", options.order_by.to_string()),
            ("recall", options.reinforce.to_string()),
        ];
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        self.get("/cat", &query)
    }

    fn recall(&mut self, query: Option<&str>, options: RecallOptions) -> Result<Vec<ScoredFact>> {
        let mut fields = vec![("limit", options.limit.to_string())];
        if let Some(text) = query {
            fields.push(("q", text.to_string()));
        }
        if let Some(under) = &options.under {
            fields.push(("under", under.to_string()));
        }
        self.get("/recall", &fields)
    }

    fn effective_tags(&mut self, fact: FactId) -> Result<TagSet> {
        self.get("/tags", &[("fact", fact.0.to_string())])
    }

    fn whoami(&mut self) -> Result<Subject> {
        Ok(self.known()?.subject.clone())
    }

    fn recall_defaults(&mut self) -> Result<RecallConfig> {
        Ok(self.known()?.recall.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(url: &str) -> ServerConfig {
        ServerConfig {
            url: url.to_string(),
            token: Some("emb_a_b".to_string()),
            token_file: None,
        }
    }

    #[test]
    fn the_client_builds_the_address_of_the_api() {
        let client = Client::open(&config("http://example.com/")).unwrap();
        assert_eq!(client.root, "http://example.com/api/v1");
        // A url with no slash at the end says the same thing.
        let client = Client::open(&config("http://example.com")).unwrap();
        assert_eq!(client.root, "http://example.com/api/v1");
    }

    #[test]
    fn the_token_goes_into_the_header_as_a_bearer() {
        let client = Client::open(&config("http://example.com")).unwrap();
        assert_eq!(client.authorization, "Bearer emb_a_b");
    }

    #[test]
    fn a_url_that_is_not_http_is_refused() {
        for url in ["", "   ", "example.com", "ftp://example.com"] {
            assert!(Client::open(&config(url)).is_err(), "{url}");
        }
        assert!(Client::open(&config("https://example.com")).is_ok());
    }

    #[test]
    fn a_server_with_no_token_says_which_key_to_write() {
        let opened = Client::open(&ServerConfig {
            url: "http://example.com".to_string(),
            token: None,
            token_file: None,
        });
        let said = match opened {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a server with no token has no way in"),
        };
        assert!(said.contains("token"), "{said}");
        assert!(said.contains("token_file"), "{said}");
    }

    #[test]
    fn the_token_can_come_out_of_a_file() {
        let dir = std::env::temp_dir().join("embornal-token-file");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "emb_from_a_file\n").unwrap();

        let client = Client::open(&ServerConfig {
            url: "http://example.com".to_string(),
            token: None,
            token_file: Some(file),
        })
        .unwrap();
        assert_eq!(client.authorization, "Bearer emb_from_a_file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failure_of_the_server_keeps_its_words() {
        assert_eq!(
            message_of(r#"{"error":"alice may not read /a"}"#),
            "alice may not read /a"
        );
        // A body of another shape is still the best thing to show.
        assert_eq!(
            message_of("  a path must start with '/'  "),
            "a path must start with '/'"
        );
        assert_eq!(message_of(""), "");
    }
}
