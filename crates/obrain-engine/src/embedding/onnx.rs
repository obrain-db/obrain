//! ONNX Runtime embedding model implementation.

use obrain_common::utils::error::{Error, Result};
use ort::session::Session;
use parking_lot::Mutex;
use std::path::Path;
use tokenizers::Tokenizer;

use super::EmbeddingModel;

/// An embedding model backed by an ONNX session and a Hugging Face tokenizer.
///
/// Load from local files using [`from_files()`](Self::from_files). The model
/// must accept `input_ids` and `attention_mask` as i64 tensors and produce a
/// pooled embedding output.
pub struct OnnxEmbeddingModel {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dimensions: usize,
    name: String,
    batch_size: usize,
    /// Whether the model declares a `token_type_ids` input (BERT-style exports).
    with_token_type_ids: bool,
}

// SAFETY: Tokenizer is !Send+!Sync by default, but our usage is safe because
// we protect the session with a Mutex and never share mutable tokenizer state
// after construction.
#[allow(unsafe_code)]
unsafe impl Send for OnnxEmbeddingModel {}
#[allow(unsafe_code)]
unsafe impl Sync for OnnxEmbeddingModel {}

impl std::fmt::Debug for OnnxEmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxEmbeddingModel")
            .field("name", &self.name)
            .field("dimensions", &self.dimensions)
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

impl OnnxEmbeddingModel {
    /// Loads a pre-configured embedding model, downloading from HuggingFace Hub if needed.
    ///
    /// For preset models, the ONNX model and tokenizer are automatically downloaded
    /// on first use and cached locally. See [`EmbeddingModelConfig`](super::EmbeddingModelConfig)
    /// for available presets.
    #[cfg(feature = "embed")]
    pub fn from_config(config: super::EmbeddingModelConfig) -> Result<Self> {
        Self::from_config_with_options(config, super::EmbeddingOptions::default())
    }

    /// Loads a pre-configured embedding model with custom options.
    ///
    /// See [`EmbeddingOptions`](super::EmbeddingOptions) for batch size and thread configuration.
    #[cfg(feature = "embed")]
    pub fn from_config_with_options(
        config: super::EmbeddingModelConfig,
        options: super::EmbeddingOptions,
    ) -> Result<Self> {
        let resolved = super::download::resolve(&config)?;

        let mut session = Session::builder(ort_environnement()?)
            .map_err(|e| Error::Internal(format!("Failed to create ONNX session builder: {e}")))?
            .with_inter_threads(options.inter_threads)
            .map_err(|e| Error::Internal(format!("Failed to set inter threads: {e}")))?
            .with_intra_threads(options.intra_threads)
            .map_err(|e| Error::Internal(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(&resolved.model_path)
            .map_err(|e| Error::Internal(format!("Failed to load ONNX model: {e}")))?;

        let tokenizer = Tokenizer::from_file(&resolved.tokenizer_path)
            .map_err(|e| Error::Internal(format!("Failed to load tokenizer: {e}")))?;

        let with_token_type_ids = Self::session_wants_token_type_ids(&session);
        let dimensions = Self::probe_dimensions(&mut session, &tokenizer, with_token_type_ids)?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            dimensions,
            name: resolved.name,
            batch_size: options.batch_size,
            with_token_type_ids,
        })
    }

    /// Loads an embedding model from local ONNX model and tokenizer files.
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable model name (e.g., "all-MiniLM-L6-v2")
    /// * `model_path` - Path to the `.onnx` model file
    /// * `tokenizer_path` - Path to the `tokenizer.json` file
    ///
    /// # Errors
    ///
    /// Returns an error if the files cannot be loaded or the model is invalid.
    pub fn from_files(
        name: impl Into<String>,
        model_path: impl AsRef<Path>,
        tokenizer_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let mut session = Session::builder(ort_environnement()?)
            .map_err(|e| Error::Internal(format!("Failed to create ONNX session builder: {e}")))?
            .with_inter_threads(1)
            .map_err(|e| Error::Internal(format!("Failed to set inter threads: {e}")))?
            .with_intra_threads(1)
            .map_err(|e| Error::Internal(format!("Failed to set intra threads: {e}")))?
            .commit_from_file(model_path.as_ref())
            .map_err(|e| Error::Internal(format!("Failed to load ONNX model: {e}")))?;

        let tokenizer = Tokenizer::from_file(tokenizer_path.as_ref())
            .map_err(|e| Error::Internal(format!("Failed to load tokenizer: {e}")))?;

        // Probe dimensions by running a dummy input
        let with_token_type_ids = Self::session_wants_token_type_ids(&session);
        let dimensions = Self::probe_dimensions(&mut session, &tokenizer, with_token_type_ids)?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            dimensions,
            name: name.into(),
            batch_size: 32,
            with_token_type_ids,
        })
    }

    /// Sets the maximum batch size for embedding generation.
    #[must_use]
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Creates an i64 tensor from shape and flat data.
    fn make_i64_tensor(
        shape: Vec<usize>,
        data: Vec<i64>,
    ) -> Result<ort::value::Value<ort::value::TensorValueType<i64>>> {
        ort::value::Tensor::from_array((shape, data.into_boxed_slice()))
            .map_err(|e| Error::Internal(format!("Failed to create tensor: {e}")))
    }

    /// Whether the session declares a `token_type_ids` input.
    fn session_wants_token_type_ids(session: &Session) -> bool {
        declares_token_type_ids(session.inputs().iter().map(|input| input.name()))
    }

    /// Runs the model. `token_type_ids` is only passed when the model declares it:
    /// two-input models (e.g. DistilBERT) reject unknown inputs.
    fn run_model(
        session: &mut Session,
        input_ids: ort::value::Value<ort::value::TensorValueType<i64>>,
        attention_mask: ort::value::Value<ort::value::TensorValueType<i64>>,
        token_type_ids: Option<ort::value::Value<ort::value::TensorValueType<i64>>>,
    ) -> Result<ort::session::SessionOutputs<'_>> {
        let outputs = match token_type_ids {
            Some(token_type_ids) => {
                session.run(ort::inputs![input_ids, attention_mask, token_type_ids])
            }
            None => session.run(ort::inputs![input_ids, attention_mask]),
        };
        outputs.map_err(|e| Error::Internal(format!("Model inference failed: {e}")))
    }

    /// Probes model output dimensions with a dummy input.
    fn probe_dimensions(
        session: &mut Session,
        tokenizer: &Tokenizer,
        with_token_type_ids: bool,
    ) -> Result<usize> {
        let encoding = tokenizer
            .encode("hello", false)
            .map_err(|e| Error::Internal(format!("Tokenizer encode error: {e}")))?;

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| m as i64)
            .collect();
        let len = ids.len();

        let input_ids = Self::make_i64_tensor(vec![1, len], ids)?;
        let attention_mask = Self::make_i64_tensor(vec![1, len], mask)?;
        let token_type_ids = if with_token_type_ids {
            Some(Self::make_i64_tensor(vec![1, len], vec![0i64; len])?)
        } else {
            None
        };

        let outputs = Self::run_model(session, input_ids, attention_mask, token_type_ids)?;

        let output = &outputs[0];
        let (shape, _data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Internal(format!("Failed to extract output tensor: {e}")))?;

        // Shape is typically [batch, dims] or [batch, seq_len, dims]
        let dims = *shape
            .last()
            .ok_or_else(|| Error::Internal("Model output has no dimensions".to_string()))?;

        Ok(dims as usize)
    }

    /// Embeds a single batch of texts.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let text_strings: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(text_strings, false)
            .map_err(|e| Error::Internal(format!("Tokenizer batch encode error: {e}")))?;

        // Pad to uniform length
        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0);

        let batch = texts.len();
        let mut all_ids = vec![0i64; batch * max_len];
        let mut all_mask = vec![0i64; batch * max_len];

        for (i, enc) in encodings.iter().enumerate() {
            for (j, &id) in enc.get_ids().iter().enumerate() {
                all_ids[i * max_len + j] = id as i64;
            }
            for (j, &m) in enc.get_attention_mask().iter().enumerate() {
                all_mask[i * max_len + j] = m as i64;
            }
        }

        let input_ids = Self::make_i64_tensor(vec![batch, max_len], all_ids)?;
        let attention_mask = Self::make_i64_tensor(vec![batch, max_len], all_mask)?;
        // BERT-style models (e.g. all-MiniLM-L6-v2) declare a third input.
        // Single-sentence inputs use segment 0 everywhere.
        let token_type_ids = if self.with_token_type_ids {
            Some(Self::make_i64_tensor(
                vec![batch, max_len],
                vec![0i64; batch * max_len],
            )?)
        } else {
            None
        };

        let mut session = self.session.lock();
        let outputs = Self::run_model(&mut session, input_ids, attention_mask, token_type_ids)?;

        let output = &outputs[0];
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Internal(format!("Failed to extract output tensor: {e}")))?;

        // Handle different output shapes
        let shape_usize: Vec<usize> = shape.iter().map(|&s| s as usize).collect();

        let result = if shape_usize.len() == 3 {
            // [batch, seq_len, dims] → take [CLS] token embedding (index 0)
            let seq_len = shape_usize[1];
            let dims = shape_usize[2];
            (0..batch)
                .map(|b| {
                    let offset = b * seq_len * dims; // CLS = seq index 0
                    data[offset..offset + dims].to_vec()
                })
                .collect()
        } else if shape_usize.len() == 2 {
            // [batch, dims] → already pooled
            let dims = shape_usize[1];
            (0..batch)
                .map(|b| {
                    let offset = b * dims;
                    data[offset..offset + dims].to_vec()
                })
                .collect()
        } else {
            return Err(Error::Internal(format!(
                "Unexpected output shape: {shape_usize:?}"
            )));
        };

        Ok(result)
    }
}

/// Returns `true` when the declared model input names include `token_type_ids`.
///
/// BERT-style exports (all-MiniLM, bge-small) declare it and fail without it;
/// other models (e.g. DistilBERT) only take `input_ids` and `attention_mask`
/// and would reject an extra input.
fn declares_token_type_ids<'a>(input_names: impl IntoIterator<Item = &'a str>) -> bool {
    input_names.into_iter().any(|name| name == "token_type_ids")
}

impl EmbeddingModel for OnnxEmbeddingModel {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch_size) {
            results.extend(self.embed_batch(chunk)?);
        }
        Ok(results)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(all(test, feature = "embed"))]
mod tests {
    use super::*;
    use crate::embedding::EmbeddingModelConfig;

    #[test]
    fn token_type_ids_not_requested_for_two_input_models() {
        assert!(!declares_token_type_ids(["input_ids", "attention_mask"]));
    }

    #[test]
    fn token_type_ids_requested_for_three_input_models() {
        assert!(declares_token_type_ids([
            "input_ids",
            "attention_mask",
            "token_type_ids"
        ]));
    }

    #[test]
    fn token_type_ids_not_requested_without_inputs() {
        assert!(!declares_token_type_ids(std::iter::empty::<&str>()));
    }

    #[test]
    #[ignore = "requires ONNX Runtime (ORT_DYLIB_PATH) and the all-MiniLM-L6-v2 model (~90MB download on first run)"]
    fn minilm_l6_embeds_fixed_sentence() {
        let model = OnnxEmbeddingModel::from_config(EmbeddingModelConfig::MiniLmL6v2)
            .expect("all-MiniLM-L6-v2 must load");
        let vectors = model
            .embed(&["Obrain stores graph nodes in a single file."])
            .expect("embedding must succeed");
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), 384);
        assert!(vectors[0].iter().any(|x| *x != 0.0));
    }
}

/// Environnement ONNX (ort rc.15 : `Session::builder` prend désormais un `&Environment`).
/// Créé une seule fois par processus, comme l'exige la bibliothèque.
fn ort_environnement() -> Result<&'static ort::environment::Environment> {
    static ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let env = ort::environment::init()
        .build()
        .map_err(|e| Error::Internal(format!("Failed to create ONNX environment: {e}")))?;
    Ok(ENV.get_or_init(|| env))
}
