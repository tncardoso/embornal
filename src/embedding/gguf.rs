//! The embedding model that runs in this process.
//!
//! The model is a GGUF file that llama.cpp reads. The default is
//! EmbeddingGemma 300M at Q8_0, which is the same model that qmd uses.
//!
//! The weights load one time and stay for the life of the process. A command
//! line pays that load one time; `embornal memory serve` pays it one time for
//! every search that it answers.

use crate::config::EmbeddingConfig;
use crate::embedding::{Embedder, Input, shape};
use crate::error::{Error, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

/// The smallest cache that the model gets, in tokens.
///
/// A cache that fits the text exactly would be built again for almost every
/// text. This floor lets one cache serve a whole group of short facts.
const MIN_CACHE_TOKENS: u32 = 512;

/// The variable that lets llama.cpp speak. It helps when a model does not
/// load, or when it refuses a text.
const LOG_ENV: &str = "EMBORNAL_LLAMA_LOG";

/// llama.cpp asks for one start. It gives back a value that proves the start
/// happened, and every later call wants a reference to it.
static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();

fn backend() -> Result<&'static LlamaBackend> {
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    let mut started =
        LlamaBackend::init().map_err(|err| Error::Embedding(format!("llama.cpp: {err}")))?;
    // llama.cpp writes what it loads to the error stream. The commands here
    // have their own words to say, so it stays quiet unless somebody asks.
    if std::env::var_os(LOG_ENV).is_none() {
        started.void_logs();
    }
    Ok(BACKEND.get_or_init(|| started))
}

/// A loaded GGUF model.
pub struct Gguf {
    model: LlamaModel,
    /// The width that the memory writes. It is at most the width of the model.
    dimensions: usize,
    /// How many tokens of one text the model reads.
    context_tokens: usize,
    model_name: String,
}

impl Gguf {
    /// Reads the weights.
    pub fn load(weights: &Path, config: &EmbeddingConfig) -> Result<Self> {
        let backend = backend()?;
        let model = LlamaModel::load_from_file(backend, weights, &LlamaModelParams::default())
            .map_err(|err| {
                Error::Embedding(format!("cannot read {}: {err}", weights.display()))
            })?;

        let width = usize::try_from(model.n_embd())
            .map_err(|_| Error::Embedding("the model reports no width".to_string()))?;
        if width < config.dimensions {
            return Err(Error::EmbeddingWidth {
                want: config.dimensions,
                got: width,
            });
        }

        Ok(Self {
            model,
            dimensions: config.dimensions,
            context_tokens: config.context_tokens.max(1) as usize,
            model_name: config.model_name().to_string(),
        })
    }

    /// Turns each text into tokens, and cuts what is too long.
    fn tokenize(&self, inputs: &[Input<'_>]) -> Result<Vec<Vec<LlamaToken>>> {
        inputs
            .iter()
            .map(|input| {
                let mut tokens = self
                    .model
                    .str_to_token(&input.prompt(), AddBos::Always)
                    .map_err(|err| Error::Embedding(format!("cannot read the text: {err}")))?;
                tokens.truncate(self.context_tokens);
                if tokens.is_empty() {
                    return Err(Error::Embedding("the text holds no token".to_string()));
                }
                Ok(tokens)
            })
            .collect()
    }

}

impl Embedder for Gguf {
    /// Reads each text and gives back one vector for each of them.
    ///
    /// The texts share one cache, which is built one time for the whole call.
    /// Each text is then a run of its own, and the cache is emptied between
    /// two of them so that no text reads the one before it.
    ///
    /// One text for each run is not an choice of speed. llama.cpp cannot pool
    /// more than one sequence for this model: the dense layers that
    /// EmbeddingGemma puts after the pooling refuse a batch that holds two
    /// sequences, and the process stops. One sequence for each run is the
    /// shape that works.
    fn embed(&mut self, inputs: &[Input<'_>]) -> Result<Vec<Vec<f32>>> {
        let texts = self.tokenize(inputs)?;
        let Some(longest) = texts.iter().map(Vec::len).max() else {
            return Ok(Vec::new());
        };

        let seats = u32::try_from(longest)
            .map_err(|_| Error::Embedding("the text is too long".to_string()))?
            .max(MIN_CACHE_TOKENS);
        let params = LlamaContextParams::default()
            .with_embeddings(true)
            .with_n_ctx(NonZeroU32::new(seats))
            .with_n_batch(seats)
            .with_n_ubatch(seats)
            .with_n_seq_max(1);

        let mut context = self
            .model
            .new_context(backend()?, params)
            .map_err(|err| Error::Embedding(format!("cannot start the model: {err}")))?;

        let mut batch = LlamaBatch::new(longest, 1);
        let mut vectors = Vec::with_capacity(texts.len());
        for tokens in &texts {
            batch.clear();
            batch
                .add_sequence(tokens, 0, false)
                .map_err(|err| Error::Embedding(format!("cannot fill the batch: {err}")))?;
            context.clear_kv_cache();
            context
                .decode(&mut batch)
                .map_err(|err| Error::Embedding(format!("the model refused the text: {err}")))?;

            let vector = context
                .embeddings_seq_ith(0)
                .map_err(|err| Error::Embedding(format!("no vector came back: {err}")))?;
            vectors.push(shape(vector.to_vec(), self.dimensions)?);
        }
        Ok(vectors)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
