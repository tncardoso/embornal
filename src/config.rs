//! The configuration file.
//!
//! Embornal keeps its data in `$HOME/.embornal`. `EMBORNAL_HOME` moves the
//! whole directory, which is what the tests use.

use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::fact::OrderBy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The name of the directory inside the home of the user.
pub const HOME_DIR_NAME: &str = ".embornal";

/// The variable that moves the whole directory.
pub const HOME_ENV: &str = "EMBORNAL_HOME";

/// The width of an embedding, if the configuration is silent.
///
/// This is the full width of the default model. A smaller value cuts the
/// vector down, which the model supports.
pub const DEFAULT_DIMENSIONS: usize = 768;

/// The provider that turns text into vectors inside this process.
pub const PROVIDER_GGUF: &str = "gguf";

/// The provider that does nothing. The memory then stores the facts and
/// leaves the embedding column empty.
pub const PROVIDER_NONE: &str = "none";

/// The model that a new memory uses.
pub const DEFAULT_MODEL: &str = "embeddinggemma-300M-Q8_0";

/// Where the weights of the default model come from.
pub const DEFAULT_REPO: &str = "ggml-org/embeddinggemma-300M-GGUF";

/// The file that holds the weights of the default model.
pub const DEFAULT_MODEL_FILE: &str = "embeddinggemma-300M-Q8_0.gguf";

/// How many tokens of one fact the model reads.
///
/// Facts are small, so this is far above what a fact needs. Text above this
/// length is cut.
pub const DEFAULT_CONTEXT_TOKENS: u32 = 2048;

/// The variable that turns the provider off. The tests use this.
pub const EMBEDDING_ENV: &str = "EMBORNAL_EMBEDDING";

/// Where Embornal keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    /// Reads `EMBORNAL_HOME`, and falls back to `$HOME/.embornal`.
    pub fn discover() -> Result<Self> {
        if let Some(home) = std::env::var_os(HOME_ENV) {
            return Ok(Self {
                home: PathBuf::from(home),
            });
        }
        let home = dirs::home_dir().ok_or(Error::NoHome)?.join(HOME_DIR_NAME);
        Ok(Self { home })
    }

    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn config_file(&self) -> PathBuf {
        self.home.join("config.yaml")
    }

    pub fn database_file(&self) -> PathBuf {
        self.home.join("memory.db")
    }

    /// Where the weights of the embedding model stay.
    pub fn model_dir(&self) -> PathBuf {
        self.home.join("models")
    }

    /// Creates the directory if it is absent.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home).map_err(|source| Error::Io {
            path: self.home.clone(),
            source,
        })
    }
}

/// The whole configuration file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The database file. The default is `memory.db` next to this file.
    pub database: Option<PathBuf>,

    /// Who the command line says it is. Access control reads this.
    pub subject: Subject,

    pub embedding: EmbeddingConfig,

    pub recall: RecallConfig,
}

impl Config {
    /// Reads the file. A missing file gives the defaults, because the tool
    /// must work before the user writes any configuration.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_norway::from_str(&text).map_err(|source| Error::ConfigParse {
                path: path.to_path_buf(),
                source,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::ConfigRead {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Returns the database file, after the default is applied.
    pub fn database_file(&self, paths: &Paths) -> PathBuf {
        self.database
            .clone()
            .unwrap_or_else(|| paths.database_file())
    }
}

/// How the memory turns text into vectors.
///
/// The width is fixed when the database builds its vector index. To change it
/// later, the index must be built again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingConfig {
    /// The number of dimensions of one vector.
    ///
    /// A value below the width of the model cuts each vector and makes it a
    /// unit vector again. The model supports this.
    pub dimensions: usize,
    /// The name of the model. It is written next to each embedding, so that a
    /// later change of model is visible.
    pub model: Option<String>,
    /// Who produces the vectors. `none` means that the memory stores the facts
    /// and leaves the embedding column empty.
    pub provider: Option<String>,
    /// The repository on HuggingFace that holds the weights.
    pub repo: String,
    /// The file of that repository.
    pub file: String,
    /// A file of weights that is already on this machine. With this, the
    /// memory downloads nothing.
    pub model_path: Option<PathBuf>,
    /// How many tokens of one text the model reads.
    pub context_tokens: u32,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            dimensions: DEFAULT_DIMENSIONS,
            model: Some(DEFAULT_MODEL.to_string()),
            provider: Some(PROVIDER_GGUF.to_string()),
            repo: DEFAULT_REPO.to_string(),
            file: DEFAULT_MODEL_FILE.to_string(),
            model_path: None,
            context_tokens: DEFAULT_CONTEXT_TOKENS,
        }
    }
}

impl EmbeddingConfig {
    /// Returns the name of the provider to build, or `None` when the memory
    /// must run without vectors.
    ///
    /// `EMBORNAL_EMBEDDING=off` wins over the file. The tests set it, so that
    /// they never reach for the weights.
    pub fn provider_name(&self) -> Option<&str> {
        if let Some(value) = std::env::var_os(EMBEDDING_ENV)
            && matches!(
                value.to_string_lossy().as_ref(),
                "off" | "none" | "0" | "false"
            )
        {
            return None;
        }
        match self.provider.as_deref() {
            None | Some(PROVIDER_NONE) | Some("") => None,
            Some(name) => Some(name),
        }
    }

    /// The name that goes next to each embedding in the database.
    pub fn model_name(&self) -> &str {
        self.model.as_deref().unwrap_or(DEFAULT_MODEL)
    }

    /// Where the weights are, or where they must go.
    pub fn weights_file(&self, paths: &Paths) -> PathBuf {
        self.model_path
            .clone()
            .unwrap_or_else(|| paths.model_dir().join(&self.file))
    }
}

/// How `recall` mixes the two indexes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecallConfig {
    /// How many facts a recall gives back.
    pub limit: usize,
    /// The weight of the keyword match.
    pub keyword_weight: f64,
    /// The weight of the vector match.
    pub vector_weight: f64,
    /// How near a fact must be before the vector index gives it.
    ///
    /// The vector index answers with the nearest facts, and it does that even
    /// when none of them is near. Without a limit, every search would give
    /// back the whole memory.
    ///
    /// The value runs from 1.0, for a fact that says the same as the
    /// question, through 0.0 for a fact with nothing in common, down to -1.0
    /// for a fact that says the opposite. The model holds most of its answers
    /// between 0.05 and 0.7, so a floor near the middle of that band would
    /// throw good answers away.
    pub vector_floor: f64,
    /// Which share of the nearest fact the other facts must reach.
    ///
    /// The floor above is one number for every question, but questions differ:
    /// one finds a fact that says almost the same, another finds a fact that
    /// only points the same way. This share reads the answer that the memory
    /// gave and keeps the facts that come near the best of them.
    ///
    /// The two work together. The share drops the tail of a good answer, and
    /// the floor drops a whole answer that is bad.
    pub vector_share: f64,
    /// The weight of the strength of the fact.
    pub signal_weight: f64,
    /// How `cat` sorts the facts, if the command line is silent.
    pub order_by: OrderBy,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            limit: 20,
            keyword_weight: 1.0,
            vector_weight: 1.0,
            vector_floor: 0.15,
            vector_share: 0.5,
            signal_weight: 0.5,
            order_by: OrderBy::Date,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_file_gives_the_defaults() {
        let config = Config::load(Path::new("/does/not/exist.yaml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.embedding.dimensions, 768);
        assert_eq!(config.subject.as_str(), "cli");
    }

    #[test]
    fn a_new_memory_embeds_with_the_default_model() {
        let config = Config::default();
        assert_eq!(config.embedding.provider.as_deref(), Some(PROVIDER_GGUF));
        assert_eq!(config.embedding.model_name(), DEFAULT_MODEL);
        assert_eq!(config.embedding.repo, DEFAULT_REPO);
        // The width of the index agrees with the width of the model.
        assert_eq!(config.embedding.dimensions, 768);
    }

    #[test]
    fn the_weights_live_next_to_the_database() {
        let paths = Paths::with_home("/tmp/embornal-test");
        let config = Config::default();
        assert_eq!(
            config.embedding.weights_file(&paths),
            PathBuf::from("/tmp/embornal-test/models/embeddinggemma-300M-Q8_0.gguf")
        );
    }

    #[test]
    fn a_named_file_of_weights_replaces_the_download() {
        let paths = Paths::with_home("/tmp/embornal-test");
        let mut config = Config::default();
        config.embedding.model_path = Some(PathBuf::from("/models/other.gguf"));
        assert_eq!(
            config.embedding.weights_file(&paths),
            PathBuf::from("/models/other.gguf")
        );
    }

    #[test]
    fn the_provider_can_be_turned_off_in_the_file() {
        let mut config = EmbeddingConfig::default();
        assert_eq!(config.provider_name(), Some(PROVIDER_GGUF));

        config.provider = Some(PROVIDER_NONE.to_string());
        assert_eq!(config.provider_name(), None);

        config.provider = None;
        assert_eq!(config.provider_name(), None);
    }

    #[test]
    fn reads_a_partial_file() {
        let dir = std::env::temp_dir().join("embornal-config-partial");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yaml");
        std::fs::write(&file, "embedding:\n  dimensions: 1024\n  model: voyage-3\n").unwrap();

        let config = Config::load(&file).unwrap();
        assert_eq!(config.embedding.dimensions, 1024);
        assert_eq!(config.embedding.model.as_deref(), Some("voyage-3"));
        // Everything else keeps its default.
        assert_eq!(config.recall.limit, 20);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_an_unknown_key() {
        let dir = std::env::temp_dir().join("embornal-config-unknown");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.yaml");
        std::fs::write(&file, "embeddings:\n  dimensions: 1024\n").unwrap();
        assert!(Config::load(&file).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn builds_the_file_names() {
        let paths = Paths::with_home("/tmp/embornal-test");
        assert_eq!(
            paths.config_file(),
            PathBuf::from("/tmp/embornal-test/config.yaml")
        );
        assert_eq!(
            paths.database_file(),
            PathBuf::from("/tmp/embornal-test/memory.db")
        );
        assert_eq!(
            Config::default().database_file(&paths),
            paths.database_file()
        );
    }
}
