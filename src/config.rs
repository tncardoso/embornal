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
pub const DEFAULT_DIMENSIONS: usize = 768;

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
    pub dimensions: usize,
    /// The name of the model. It is written next to each embedding, so that a
    /// later change of model is visible.
    pub model: Option<String>,
    /// Who produces the vectors. No provider means that the memory stores the
    /// facts and leaves the embedding column empty.
    pub provider: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            dimensions: DEFAULT_DIMENSIONS,
            model: None,
            provider: None,
        }
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
