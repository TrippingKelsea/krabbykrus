//! GGUF model loading and inference via candle.
//!
//! Supports quantized models (Q4_K_M, Q5_K_M, etc.) for low-resource local inference.
//! The engine handles tokenization, KV-cache management, and autoregressive generation.

use candle_core::{Device, IndexOp, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_llama as qllama;
use candle_transformers::models::quantized_qwen2 as qqwen2;
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// Errors specific to model inference.
#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("Model file not found: {0}")]
    ModelNotFound(PathBuf),
    #[error("Tokenizer error: {0}")]
    Tokenizer(String),
    #[error("Candle error: {0}")]
    Candle(#[from] candle_core::Error),
    #[error("Failed to download model: {0}")]
    Download(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Runtime statistics from inference.
#[derive(Debug, Clone, Default)]
pub struct InferenceStats {
    /// Total tokens generated in this call.
    pub tokens_generated: usize,
    /// Wall-clock time for generation in milliseconds.
    pub generation_time_ms: u64,
    /// Tokens per second.
    pub tokens_per_second: f64,
    /// Prompt tokens consumed.
    pub prompt_tokens: usize,
}

/// Configuration for the inference engine.
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    /// Path to the GGUF model file.
    pub model_path: PathBuf,
    /// Path to the tokenizer.json file.
    pub tokenizer_path: PathBuf,
    /// Maximum tokens to generate per call.
    pub max_tokens: usize,
    /// Sampling temperature (0.0 = greedy).
    pub temperature: f64,
    /// Top-p nucleus sampling threshold.
    pub top_p: f64,
    /// Repeat penalty for reducing repetition.
    pub repeat_penalty: f32,
    /// Number of recent tokens to consider for repeat penalty.
    pub repeat_last_n: usize,
    /// Random seed for reproducibility (0 = random).
    pub seed: u64,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            tokenizer_path: PathBuf::new(),
            max_tokens: 256,
            temperature: 0.1,
            top_p: 0.9,
            repeat_penalty: 1.1,
            repeat_last_n: 64,
            seed: 42,
        }
    }
}

/// Supported GGUF model architectures, auto-detected from metadata.
enum ModelArch {
    Llama(qllama::ModelWeights),
    Qwen2(qqwen2::ModelWeights),
}

impl ModelArch {
    fn forward(&mut self, x: &Tensor, index_pos: usize) -> candle_core::Result<Tensor> {
        match self {
            Self::Llama(m) => m.forward(x, index_pos),
            Self::Qwen2(m) => m.forward(x, index_pos),
        }
    }
}

/// Local GGUF model inference engine.
///
/// Loads a quantized model via candle and provides synchronous
/// (blocking) text generation. Designed to run on CPU with modest
/// memory requirements (~1-2 GB for Q4 1.5B-parameter models).
pub struct InferenceEngine {
    model: ModelArch,
    tokenizer: Tokenizer,
    device: Device,
    config: InferenceConfig,
    /// Cumulative stats across all calls since startup.
    cumulative_stats: std::sync::Mutex<CumulativeStats>,
}

#[derive(Debug, Clone, Default)]
struct CumulativeStats {
    total_calls: u64,
    total_tokens_generated: u64,
    total_prompt_tokens: u64,
    total_generation_time_ms: u64,
}

impl InferenceEngine {
    /// Load a GGUF model from disk.
    ///
    /// Auto-detects the model architecture from GGUF metadata
    /// (`general.architecture`). Supports: llama, qwen2.
    ///
    /// This is a blocking operation — call from a `spawn_blocking` context.
    pub fn load(config: InferenceConfig) -> Result<Self, InferenceError> {
        if !config.model_path.exists() {
            return Err(InferenceError::ModelNotFound(config.model_path.clone()));
        }

        info!("Loading GGUF model from {}", config.model_path.display());
        let device = Device::Cpu;

        let mut file = std::fs::File::open(&config.model_path)?;
        let content = candle_core::quantized::gguf_file::Content::read(&mut file)
            .map_err(InferenceError::Candle)?;

        // Read architecture from GGUF metadata
        let arch = content
            .metadata
            .get("general.architecture")
            .and_then(|v| v.to_string().ok())
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_default();

        info!(
            "GGUF architecture: {}",
            if arch.is_empty() { "(not set)" } else { &arch }
        );

        let model = match arch.as_str() {
            "qwen2" => ModelArch::Qwen2(qqwen2::ModelWeights::from_gguf(
                content, &mut file, &device,
            )?),
            // Default to llama — covers llama, mistral, and many llama-compatible models
            _ => ModelArch::Llama(qllama::ModelWeights::from_gguf(
                content, &mut file, &device,
            )?),
        };

        info!("Loading tokenizer from {}", config.tokenizer_path.display());
        let tokenizer = Tokenizer::from_file(&config.tokenizer_path)
            .map_err(|e| InferenceError::Tokenizer(e.to_string()))?;

        info!("Overseer inference engine loaded successfully");
        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            cumulative_stats: std::sync::Mutex::new(CumulativeStats::default()),
        })
    }

    /// Known quantization suffixes on HuggingFace repo names.
    /// GGUF/GGML/AWQ/GPTQ repos typically don't ship `tokenizer.json`;
    /// the tokenizer lives in the base model repo.
    const QUANT_SUFFIXES: &'static [&'static str] = &[
        "-GGUF", "-gguf", "-GGML", "-ggml", "-AWQ", "-awq", "-GPTQ", "-gptq", "-EXL2", "-exl2",
    ];

    /// Derive the base (non-quantized) repo ID by stripping known suffixes.
    fn base_repo_id(repo_id: &str) -> Option<&str> {
        Self::QUANT_SUFFIXES
            .iter()
            .find_map(|suffix| repo_id.strip_suffix(suffix))
    }

    /// Download a model from HuggingFace Hub if not already cached.
    ///
    /// Returns `(model_path, tokenizer_path)`.
    ///
    /// GGUF repos (e.g. `Qwen/Qwen2.5-1.5B-Instruct-GGUF`) typically don't
    /// include `tokenizer.json`. The tokenizer is resolved in order:
    ///
    /// 1. Explicit `tokenizer_repo` if provided
    /// 2. The model repo itself (works for non-quantized repos)
    /// 3. Base repo derived by stripping quantization suffixes (`-GGUF`, `-AWQ`, etc.)
    pub async fn download_model(
        repo_id: &str,
        model_filename: &str,
        tokenizer_repo: Option<&str>,
        _cache_dir: &Path,
    ) -> Result<(PathBuf, PathBuf), InferenceError> {
        let api =
            hf_hub::api::tokio::Api::new().map_err(|e| InferenceError::Download(e.to_string()))?;

        let repo = api.model(repo_id.to_string());

        info!("Downloading model {repo_id}/{model_filename}...");
        let model_path = repo
            .get(model_filename)
            .await
            .map_err(|e| InferenceError::Download(format!("model: {e}")))?;

        // Resolve tokenizer: explicit repo > model repo > base repo
        let tokenizer_path = if let Some(tok_repo) = tokenizer_repo {
            info!("Fetching tokenizer from configured repo {tok_repo}...");
            api.model(tok_repo.to_string())
                .get("tokenizer.json")
                .await
                .map_err(|e| InferenceError::Download(format!("tokenizer from {tok_repo}: {e}")))?
        } else {
            match repo.get("tokenizer.json").await {
                Ok(path) => path,
                Err(_) => {
                    let base = Self::base_repo_id(repo_id).ok_or_else(|| {
                        InferenceError::Download(format!(
                            "tokenizer not found in {repo_id} and could not derive base repo. \
                             Set `tokenizer_repo` in [overseer] config to specify it explicitly."
                        ))
                    })?;
                    info!("Tokenizer not in {repo_id}, trying base repo {base}...");
                    api.model(base.to_string())
                        .get("tokenizer.json")
                        .await
                        .map_err(|e| {
                            InferenceError::Download(format!("tokenizer from {base}: {e}"))
                        })?
                }
            }
        };

        Ok((model_path, tokenizer_path))
    }

    /// Generate text from a prompt.
    ///
    /// This is CPU-bound — call via `tokio::task::spawn_blocking`.
    pub fn generate(&mut self, prompt: &str) -> Result<(String, InferenceStats), InferenceError> {
        let start = std::time::Instant::now();

        let tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| InferenceError::Tokenizer(e.to_string()))?;

        let prompt_tokens = tokens.get_ids().len();
        let input_ids = Tensor::new(tokens.get_ids(), &self.device)?;

        let mut logits_processor = LogitsProcessor::new(
            self.config.seed,
            Some(self.config.temperature),
            Some(self.config.top_p),
        );

        let mut all_tokens: Vec<u32> = Vec::with_capacity(self.config.max_tokens);
        let mut input = input_ids.unsqueeze(0)?;

        // Autoregressive generation loop
        for i in 0..self.config.max_tokens {
            let logits = self.model.forward(&input, i)?;

            // Get logits for the last token position
            let logits = logits.squeeze(0)?;
            let logits = if i == 0 && prompt_tokens > 1 {
                // For the first forward pass with full prompt, take the last position
                logits.i(logits.dim(0)? - 1..)?
            } else {
                logits
            };
            let logits = logits.squeeze(0)?;

            let next_token = logits_processor.sample(&logits)?;

            // Check for EOS
            if self.is_eos(next_token) {
                break;
            }

            all_tokens.push(next_token);
            input = Tensor::new(&[next_token], &self.device)?.unsqueeze(0)?;
        }

        let generated_text = self
            .tokenizer
            .decode(&all_tokens, true)
            .map_err(|e| InferenceError::Tokenizer(e.to_string()))?;

        let elapsed = start.elapsed();
        let tokens_generated = all_tokens.len();
        let generation_time_ms = elapsed.as_millis() as u64;
        let tokens_per_second = if generation_time_ms > 0 {
            tokens_generated as f64 / (generation_time_ms as f64 / 1000.0)
        } else {
            0.0
        };

        let stats = InferenceStats {
            tokens_generated,
            generation_time_ms,
            tokens_per_second,
            prompt_tokens,
        };

        // Update cumulative stats
        if let Ok(mut cumulative) = self.cumulative_stats.lock() {
            cumulative.total_calls += 1;
            cumulative.total_tokens_generated += tokens_generated as u64;
            cumulative.total_prompt_tokens += prompt_tokens as u64;
            cumulative.total_generation_time_ms += generation_time_ms;
        }

        debug!(
            "Generated {} tokens in {}ms ({:.1} tok/s)",
            tokens_generated, generation_time_ms, tokens_per_second
        );

        Ok((generated_text, stats))
    }

    /// Check if a token ID is an end-of-sequence marker.
    fn is_eos(&self, token_id: u32) -> bool {
        // Common EOS tokens across models
        if let Some(eos_id) = self.tokenizer.token_to_id("</s>") {
            if token_id == eos_id {
                return true;
            }
        }
        if let Some(eos_id) = self.tokenizer.token_to_id("<|endoftext|>") {
            if token_id == eos_id {
                return true;
            }
        }
        if let Some(eos_id) = self.tokenizer.token_to_id("<|im_end|>") {
            if token_id == eos_id {
                return true;
            }
        }
        false
    }

    /// Get cumulative inference statistics since engine startup.
    pub fn cumulative_stats(&self) -> (u64, u64, u64, u64) {
        if let Ok(s) = self.cumulative_stats.lock() {
            (
                s.total_calls,
                s.total_tokens_generated,
                s.total_prompt_tokens,
                s.total_generation_time_ms,
            )
        } else {
            (0, 0, 0, 0)
        }
    }

    /// Get the model file path.
    pub fn model_path(&self) -> &Path {
        &self.config.model_path
    }

    /// Get the max tokens setting.
    pub fn max_tokens(&self) -> usize {
        self.config.max_tokens
    }

    /// Get the temperature setting.
    pub fn temperature(&self) -> f64 {
        self.config.temperature
    }
}
