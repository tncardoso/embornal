//! The configuration file.
//!
//! Embornal keeps its files in three directories, because they hold three
//! different kinds of thing:
//!
//! - the configuration, in `$XDG_CONFIG_HOME/embornal`;
//! - the memory itself, in `$XDG_DATA_HOME/embornal`;
//! - the weights of the embedding model, in `$XDG_CACHE_HOME/embornal`.
//!
//! The weights are a download that the tool can make again, and the memory is
//! the one thing that a backup must hold. The split says which is which.
//!
//! `EMBORNAL_HOME` puts the three in one directory, which is what the tests
//! use.

use crate::error::{Error, Result};
use crate::memory::acl::Subject;
use crate::memory::fact::OrderBy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The name of the directory that Embornal makes below each XDG directory.
pub const DIR_NAME: &str = "embornal";

/// Where Embornal kept everything before the three directories.
pub const LEGACY_DIR_NAME: &str = ".embornal";

/// The variable that puts the three directories in one.
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

/// The name of the configuration file.
pub const CONFIG_FILE: &str = "config.yaml";

/// The name of the database file.
pub const DATABASE_FILE: &str = "memory.db";

/// The files that SQLite keeps next to the database while it runs.
const DATABASE_SIDE_FILES: [&str; 2] = ["memory.db-wal", "memory.db-shm"];

/// Where Embornal keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
}

impl Paths {
    /// Reads `EMBORNAL_HOME`, and falls back to the three XDG directories.
    ///
    /// A memory that an older build wrote to `$HOME/.embornal` moves to the
    /// new places here. See [`Paths::adopt`].
    pub fn discover() -> Result<Self> {
        if let Some(home) = std::env::var_os(HOME_ENV) {
            return Ok(Self::with_home(PathBuf::from(home)));
        }

        let paths = Self {
            config: dirs::config_dir().ok_or(Error::NoHome)?.join(DIR_NAME),
            data: dirs::data_dir().ok_or(Error::NoHome)?.join(DIR_NAME),
            cache: dirs::cache_dir().ok_or(Error::NoHome)?.join(DIR_NAME),
        };

        let legacy = dirs::home_dir().ok_or(Error::NoHome)?.join(LEGACY_DIR_NAME);
        for moved in paths.adopt(&legacy)? {
            eprintln!("embornal: moved {moved}");
        }
        Ok(paths)
    }

    /// Puts the three directories in one. `EMBORNAL_HOME` and the tests use
    /// this.
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            config: home.clone(),
            data: home.clone(),
            cache: home,
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join(CONFIG_FILE)
    }

    pub fn database_file(&self) -> PathBuf {
        self.data.join(DATABASE_FILE)
    }

    /// Where the weights of the embedding model stay.
    pub fn model_dir(&self) -> PathBuf {
        self.cache.join("models")
    }

    /// Creates the directories that are absent.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.config, &self.data, &self.cache] {
            std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                path: dir.clone(),
                source,
            })?;
        }
        Ok(())
    }

    /// Takes the configuration and the memory out of an older directory.
    ///
    /// The function moves a file only when the new place is still empty, so a
    /// memory that already lives in the new place is never written over. The
    /// weights stay behind: they are a cache, and the tool downloads them
    /// again.
    ///
    /// It gives back one line for each file that it moved. It removes the
    /// older directory only when that directory becomes empty.
    pub fn adopt(&self, legacy: &Path) -> Result<Vec<String>> {
        if !legacy.is_dir() {
            return Ok(Vec::new());
        }

        let mut moved = Vec::new();
        let plan = std::iter::once((CONFIG_FILE, self.config_file()))
            .chain(std::iter::once((DATABASE_FILE, self.database_file())))
            .chain(
                DATABASE_SIDE_FILES
                    .iter()
                    .map(|name| (*name, self.data.join(name))),
            );

        for (name, target) in plan {
            let source = legacy.join(name);
            if !source.exists() || target.exists() {
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            move_file(&source, &target)?;
            moved.push(format!("{} to {}", source.display(), target.display()));
        }

        // The directory goes only when nothing is left in it. Anything else
        // would throw away a file that this build does not know about.
        std::fs::remove_dir(legacy).ok();
        Ok(moved)
    }
}

/// Moves one file, across file systems if it must.
///
/// `rename` fails when the two places sit on different file systems, and the
/// home of a user and its cache often do. The copy is the answer to that, and
/// the original goes only after the copy arrives.
fn move_file(source: &Path, target: &Path) -> Result<()> {
    if std::fs::rename(source, target).is_ok() {
        return Ok(());
    }
    std::fs::copy(source, target).map_err(|err| Error::Io {
        path: source.to_path_buf(),
        source: err,
    })?;
    std::fs::remove_file(source).map_err(|err| Error::Io {
        path: source.to_path_buf(),
        source: err,
    })
}

/// The whole configuration file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The database file. The default is `memory.db` next to this file.
    pub database: Option<PathBuf>,

    /// Who the command line says it is. Access control reads this.
    ///
    /// A memory on a server ignores this: there the token says who asks.
    pub subject: Subject,

    /// The server that holds the memory.
    ///
    /// With this, the commands do their work there and this machine keeps no
    /// facts. Without it, the memory is the file of this machine.
    pub server: Option<ServerConfig>,

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

/// The server that holds the memory.
///
/// The token says who the client is, so a client with somebody else's token
/// is that person. Keep it as you would keep a key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Where the server answers, such as `https://memory.example.com`.
    pub url: String,
    /// The token itself.
    pub token: Option<String>,
    /// A file that holds the token, and nothing else.
    ///
    /// Use this to keep the token out of a configuration file that a backup
    /// or a repository might carry.
    pub token_file: Option<PathBuf>,
}

impl ServerConfig {
    /// Reads the token, from the file when the file is what holds it.
    pub fn secret(&self) -> Result<String> {
        if let Some(path) = &self.token_file {
            let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            return Ok(text.trim().to_string());
        }
        match self.token.as_deref().map(str::trim) {
            Some(secret) if !secret.is_empty() => Ok(secret.to_string()),
            _ => Err(Error::BadArgument(format!(
                "the server {} needs a token: put `token` or `token_file` \
                 below `server` in the configuration",
                self.url
            ))),
        }
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
    /// Which share of the facts a word may hold before it says nothing.
    ///
    /// A word such as "the" reaches almost every fact of a memory in English.
    /// It therefore tells no fact from another, and a question that holds it
    /// would drag the whole memory into the answer.
    ///
    /// The count runs through the index, so this needs no list of words and
    /// it works in each language that the memory holds.
    ///
    /// A value of 1.0 or above keeps every word.
    pub keyword_ceiling: f64,
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
            keyword_ceiling: 0.5,
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
    fn the_weights_live_below_the_cache_directory() {
        let config = Config::default();
        assert_eq!(
            config
                .embedding
                .weights_file(&Paths::with_home("/tmp/embornal-test")),
            PathBuf::from("/tmp/embornal-test/models/embeddinggemma-300M-Q8_0.gguf")
        );
        assert_eq!(
            config
                .embedding
                .weights_file(&split(Path::new("/tmp/embornal-split"))),
            PathBuf::from("/tmp/embornal-split/cache/models/embeddinggemma-300M-Q8_0.gguf")
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

    #[test]
    fn one_home_holds_the_three_kinds_of_file() {
        let paths = Paths::with_home("/tmp/embornal-test");
        assert_eq!(paths.config_dir(), Path::new("/tmp/embornal-test"));
        assert_eq!(paths.data_dir(), Path::new("/tmp/embornal-test"));
        assert_eq!(paths.cache_dir(), Path::new("/tmp/embornal-test"));
    }

    /// Builds three separate directories below one root, the way the XDG
    /// directories sit apart from each other.
    fn split(root: &Path) -> Paths {
        Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
        }
    }

    #[test]
    fn each_kind_of_file_goes_to_its_own_directory() {
        let paths = split(Path::new("/tmp/embornal-split"));
        assert_eq!(
            paths.config_file(),
            PathBuf::from("/tmp/embornal-split/config/config.yaml")
        );
        assert_eq!(
            paths.database_file(),
            PathBuf::from("/tmp/embornal-split/data/memory.db")
        );
        assert_eq!(
            paths.model_dir(),
            PathBuf::from("/tmp/embornal-split/cache/models")
        );
    }

    /// Builds a directory that no other test touches.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("embornal-adopt-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_older_directory_gives_up_the_memory_and_the_configuration() {
        let root = scratch("moves");
        let legacy = root.join(".embornal");
        std::fs::create_dir_all(legacy.join("models")).unwrap();
        std::fs::write(legacy.join("config.yaml"), "recall:\n  limit: 7\n").unwrap();
        std::fs::write(legacy.join("memory.db"), b"the facts").unwrap();
        std::fs::write(legacy.join("models/weights.gguf"), b"heavy").unwrap();

        let paths = split(&root);
        paths.ensure().unwrap();
        let moved = paths.adopt(&legacy).unwrap();

        assert_eq!(moved.len(), 2, "{moved:?}");
        assert_eq!(std::fs::read(paths.database_file()).unwrap(), b"the facts");
        assert!(paths.config_file().exists());
        assert!(!legacy.join("memory.db").exists());
        // The weights are a cache, so they stay where they are and the older
        // directory stays with them.
        assert!(legacy.join("models/weights.gguf").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_memory_that_is_already_in_the_new_place_is_never_written_over() {
        let root = scratch("keeps");
        let legacy = root.join(".embornal");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("memory.db"), b"the older facts").unwrap();

        let paths = split(&root);
        paths.ensure().unwrap();
        std::fs::write(paths.database_file(), b"the facts in use").unwrap();

        let moved = paths.adopt(&legacy).unwrap();

        assert!(moved.is_empty(), "{moved:?}");
        assert_eq!(
            std::fs::read(paths.database_file()).unwrap(),
            b"the facts in use"
        );
        // The file that did not move stays where it is, so that nothing is
        // lost.
        assert!(legacy.join("memory.db").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_older_directory_goes_when_nothing_is_left_in_it() {
        let root = scratch("empties");
        let legacy = root.join(".embornal");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("memory.db"), b"the facts").unwrap();

        let paths = split(&root);
        paths.ensure().unwrap();
        paths.adopt(&legacy).unwrap();

        assert!(!legacy.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_absent_older_directory_asks_for_no_work() {
        let root = scratch("absent");
        let paths = split(&root);
        assert!(paths.adopt(&root.join(".embornal")).unwrap().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
