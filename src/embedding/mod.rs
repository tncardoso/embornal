//! The vectors that let `recall` find a fact by its sense.
//!
//! An [`Embedder`] turns text into one vector. The memory writes that vector
//! next to each fact, and `recall` compares the vector of the question with
//! them. A fact then answers a question that shares no word with it.
//!
//! The default model is EmbeddingGemma 300M, quantised to Q8_0, and it runs in
//! this process. See [`gguf`].
//!
//! # The task prefix
//!
//! EmbeddingGemma reads a prefix that names the task. The prefix is not
//! decoration: the model puts a question and an answer in the same region of
//! the space only when it knows which of the two it reads. [`Input`] writes the
//! prefix, so a caller never writes it.

#[cfg(feature = "gguf")]
pub mod download;
#[cfg(feature = "gguf")]
pub mod gguf;

use crate::config::{Config, EmbeddingConfig, PROVIDER_GGUF, Paths};
use crate::error::{Error, Result};
use crate::memory::path::WikiPath;

/// One text to embed, together with the task that it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input<'a> {
    /// A fact that goes into the index. The path of the fact is its title.
    Document {
        path: &'a WikiPath,
        content: &'a str,
    },
    /// A text that carries its own title, such as the summary of a function
    /// under its qualified name. The code index writes these.
    Titled { title: &'a str, content: &'a str },
    /// The words that somebody searches for.
    Query(&'a str),
}

impl Input<'_> {
    /// Writes the text that the model reads, prefix included.
    pub fn prompt(&self) -> String {
        match self {
            Self::Document { path, content } => format!("title: {path} | text: {content}"),
            Self::Titled { title, content } => format!("title: {title} | text: {content}"),
            Self::Query(text) => format!("task: search result | query: {text}"),
        }
    }
}

/// Something that turns text into vectors.
///
/// Each vector that comes back holds [`Embedder::dimensions`] numbers and has
/// a length of one, so that the distance of the vector index agrees with the
/// angle between two vectors.
pub trait Embedder: Send {
    /// Embeds each input, in order.
    fn embed(&mut self, inputs: &[Input<'_>]) -> Result<Vec<Vec<f32>>>;

    /// The width of every vector that this embedder gives back.
    fn dimensions(&self) -> usize;

    /// The name that goes next to each embedding in the database.
    fn model_name(&self) -> &str;

    /// Embeds one input. This is the common case.
    fn embed_one(&mut self, input: Input<'_>) -> Result<Vec<f32>> {
        let mut vectors = self.embed(&[input])?;
        vectors
            .pop()
            .ok_or_else(|| Error::Embedding("the model gave back no vector".to_string()))
    }
}

/// The embedder of a memory, built on the first call that needs it.
///
/// Most commands never embed anything: `ls`, `cat` and `tree` only read rows.
/// Loading 300 MB of weights for them would be waste, so the weights wait
/// here until `store`, `recall` or `reindex` asks for them.
pub struct Provider {
    source: Source,
    made: Option<Box<dyn Embedder>>,
}

enum Source {
    /// This memory runs on the keyword index alone.
    Off,
    /// The weights load on the first call.
    ///
    /// The box keeps the empty arm small. A memory that never embeds is the
    /// common one: every `ls` and every client of a remote server has it.
    Gguf(Box<Weights>),
}

/// What the provider needs to find the weights and load them.
struct Weights {
    config: EmbeddingConfig,
    paths: Paths,
}

impl Provider {
    /// Reads what the configuration asks for. It loads nothing yet.
    pub fn from_config(config: &Config, paths: &Paths) -> Result<Self> {
        let source = match config.embedding.provider_name() {
            None => Source::Off,
            Some(PROVIDER_GGUF) => Source::Gguf(Box::new(Weights {
                config: config.embedding.clone(),
                paths: paths.clone(),
            })),
            Some(other) => return Err(Error::UnknownProvider(other.to_string())),
        };
        Ok(Self { source, made: None })
    }

    /// A provider that does nothing.
    pub fn off() -> Self {
        Self {
            source: Source::Off,
            made: None,
        }
    }

    /// A provider that is ready. The tests use this.
    pub fn ready(embedder: Box<dyn Embedder>) -> Self {
        Self {
            source: Source::Off,
            made: Some(embedder),
        }
    }

    /// Whether this memory runs without vectors.
    ///
    /// A caller reads this before it builds an [`Input`], so that a memory
    /// with no provider does no work at all.
    pub fn is_off(&self) -> bool {
        self.made.is_none() && matches!(self.source, Source::Off)
    }

    /// Returns the embedder, and builds it if this is the first call.
    pub fn get(&mut self) -> Result<Option<&mut (dyn Embedder + 'static)>> {
        if self.made.is_none() {
            match &self.source {
                Source::Off => return Ok(None),
                Source::Gguf(weights) => {
                    self.made = Some(build_gguf(&weights.config, &weights.paths)?);
                }
            }
        }
        Ok(self.made.as_deref_mut())
    }
}

#[cfg(feature = "gguf")]
fn build_gguf(config: &EmbeddingConfig, paths: &Paths) -> Result<Box<dyn Embedder>> {
    let weights = download::ensure(config, paths)?;
    Ok(Box::new(gguf::Gguf::load(&weights, config)?))
}

#[cfg(not(feature = "gguf"))]
fn build_gguf(_config: &EmbeddingConfig, _paths: &Paths) -> Result<Box<dyn Embedder>> {
    Err(Error::ProviderNotBuilt(PROVIDER_GGUF.to_string()))
}

/// Cuts a vector down to `dimensions` and makes its length one.
///
/// The model is trained so that the first numbers of a vector already carry
/// most of the meaning. A shorter vector therefore still works, and it makes
/// the index smaller. The cut needs the length set to one again.
pub fn shape(mut vector: Vec<f32>, dimensions: usize) -> Result<Vec<f32>> {
    if vector.len() < dimensions {
        return Err(Error::EmbeddingWidth {
            want: dimensions,
            got: vector.len(),
        });
    }
    vector.truncate(dimensions);

    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    // A vector of zeros has no direction. Leave it as it is, because dividing
    // by zero would make every number a NaN and poison the index.
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    Ok(vector)
}

/// Writes a vector the way the vector index reads it.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> WikiPath {
        WikiPath::parse(text).unwrap()
    }

    #[test]
    fn a_fact_carries_its_path_as_the_title() {
        let path = path("/projects/embornal");
        let input = Input::Document {
            path: &path,
            content: "The memory uses SQLite.",
        };
        assert_eq!(
            input.prompt(),
            "title: /projects/embornal | text: The memory uses SQLite."
        );
    }

    #[test]
    fn a_question_carries_the_search_task() {
        assert_eq!(
            Input::Query("where is the data").prompt(),
            "task: search result | query: where is the data"
        );
    }

    #[test]
    fn a_vector_comes_out_with_a_length_of_one() {
        let vector = shape(vec![3.0, 4.0], 2).unwrap();
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "{norm}");
        assert!((vector[0] - 0.6).abs() < 1e-6);
        assert!((vector[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn a_shorter_width_cuts_the_vector_and_sets_the_length_again() {
        let vector = shape(vec![1.0, 0.0, 5.0, 5.0], 2).unwrap();
        assert_eq!(vector, vec![1.0, 0.0]);
    }

    #[test]
    fn a_vector_that_is_too_short_is_refused() {
        assert!(matches!(
            shape(vec![1.0, 2.0], 4),
            Err(Error::EmbeddingWidth { want: 4, got: 2 })
        ));
    }

    #[test]
    fn a_vector_of_zeros_stays_finite() {
        let vector = shape(vec![0.0, 0.0], 2).unwrap();
        assert!(vector.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_blob_holds_four_bytes_for_each_number() {
        let blob = to_blob(&[1.0f32, 2.0]);
        assert_eq!(blob.len(), 8);
        assert_eq!(blob[..4], 1.0f32.to_le_bytes());
    }

    #[test]
    fn an_unknown_provider_is_refused() {
        let mut config = Config::default();
        config.embedding.provider = Some("magic".to_string());
        let paths = Paths::with_home("/tmp/embornal-provider");
        assert!(matches!(
            Provider::from_config(&config, &paths),
            Err(Error::UnknownProvider(name)) if name == "magic"
        ));
    }

    #[test]
    fn a_provider_that_is_off_builds_nothing() {
        let mut config = Config::default();
        config.embedding.provider = None;
        let paths = Paths::with_home("/tmp/embornal-provider");
        let mut provider = Provider::from_config(&config, &paths).unwrap();
        assert!(provider.is_off());
        assert!(provider.get().unwrap().is_none());
    }
}
