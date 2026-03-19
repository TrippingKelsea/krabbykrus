//! `rockbot-doctor` — AI-powered configuration diagnostics.
//!
//! Embeds a local quantized model (same candle/GGUF infrastructure as the
//! overseer) specifically for diagnosing and auto-fixing configuration issues.
//!
//! ## Capabilities
//!
//! - **Parse error diagnosis**: Classify and explain config errors in plain English
//! - **Auto-repair**: Suggest or apply TOML fixes (field renames, type corrections, missing sections)
//! - **Config migration**: Detect outdated fields across version changes
//!
//! ## Feature Gate
//!
//! This crate is compiled in only when `doctor-ai` is enabled at the binary level.
//! The `[doctor]` config section is silently ignored when the feature is off.

pub mod chat_commands;
pub mod diagnosis;
pub mod learned;
pub mod migration;
pub mod prompts;
pub mod repair;
pub mod storage;

use rockbot_overseer::inference::{InferenceConfig, InferenceEngine};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub use diagnosis::{ConfigDiagnosis, DiagnosisKind};
pub use learned::{LearnedFix, LearnedStore};
pub use migration::{MigrationNote, MigrationSource};
pub use repair::DoctorFix;
pub use storage::{inspect_storage, recommended_actions, summarize_report, StorageReport};

/// Speaker role in a Doctor AI conversation.
#[derive(Debug, Clone)]
pub enum Role {
    User,
    Assistant,
}

/// Doctor AI configuration, parsed from `[doctor]` in the TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorConfig {
    /// HuggingFace model repo ID.
    #[serde(default = "default_model_id")]
    pub model_id: String,
    /// GGUF filename within the repo.
    #[serde(default = "default_model_filename")]
    pub model_filename: String,
    /// HuggingFace repo ID for the tokenizer. Auto-derived if empty.
    #[serde(default)]
    pub tokenizer_repo: String,
    /// Local path to a pre-downloaded GGUF model file.
    #[serde(default)]
    pub model_path: PathBuf,
    /// Local path to a pre-downloaded tokenizer.json.
    #[serde(default)]
    pub tokenizer_path: PathBuf,
    /// Maximum tokens to generate per diagnosis (default: 512).
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Sampling temperature (default: 0.05 — very deterministic for structured output).
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Top-p sampling (default: 0.9).
    #[serde(default = "default_top_p")]
    pub top_p: f64,
    /// Repeat penalty (default: 1.1).
    #[serde(default = "default_repeat_penalty")]
    pub repeat_penalty: f32,
    /// Random seed (default: 42).
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Automatically apply safe fixes without prompting (default: false).
    #[serde(default)]
    pub auto_fix: bool,
}

fn default_model_id() -> String {
    "Qwen/Qwen2.5-1.5B-Instruct-GGUF".to_string()
}
fn default_model_filename() -> String {
    "qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string()
}
fn default_max_tokens() -> usize {
    512
}
fn default_temperature() -> f64 {
    0.05
}
fn default_top_p() -> f64 {
    0.9
}
fn default_repeat_penalty() -> f32 {
    1.1
}
fn default_seed() -> u64 {
    42
}

impl Default for DoctorConfig {
    fn default() -> Self {
        Self {
            model_id: default_model_id(),
            model_filename: default_model_filename(),
            tokenizer_repo: String::new(),
            model_path: PathBuf::new(),
            tokenizer_path: PathBuf::new(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            top_p: default_top_p(),
            repeat_penalty: default_repeat_penalty(),
            seed: default_seed(),
            auto_fix: false,
        }
    }
}

/// The Doctor AI: embedded local model for configuration diagnostics.
pub struct DoctorAi {
    config: DoctorConfig,
    engine: Arc<Mutex<InferenceEngine>>,
    learned: Option<LearnedStore>,
}

impl DoctorAi {
    /// Initialize the doctor AI: load or download the model.
    ///
    /// Follows the same pattern as `Overseer::init()` — downloads from
    /// HuggingFace Hub if no local path is configured.
    pub async fn init(
        config: DoctorConfig,
    ) -> Result<Self, rockbot_overseer::inference::InferenceError> {
        let (model_path, tokenizer_path) = if config.model_path.as_os_str().is_empty() {
            info!(
                "Doctor AI: downloading model {}/{}",
                config.model_id, config.model_filename
            );
            let cache_dir = dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rockbot")
                .join("doctor");
            let tokenizer_repo = if config.tokenizer_repo.is_empty() {
                None
            } else {
                Some(config.tokenizer_repo.as_str())
            };
            InferenceEngine::download_model(
                &config.model_id,
                &config.model_filename,
                tokenizer_repo,
                &cache_dir,
            )
            .await?
        } else {
            (config.model_path.clone(), config.tokenizer_path.clone())
        };

        let inference_config = InferenceConfig {
            model_path,
            tokenizer_path,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            top_p: config.top_p,
            repeat_penalty: config.repeat_penalty,
            repeat_last_n: 64,
            seed: config.seed,
        };

        let engine = tokio::task::spawn_blocking(move || InferenceEngine::load(inference_config))
            .await
            .map_err(|e| {
                rockbot_overseer::inference::InferenceError::Tokenizer(format!(
                    "Task join error: {e}"
                ))
            })??;

        let learned = match LearnedStore::open() {
            Ok(store) => {
                info!("Doctor AI: loaded {} learned fixes", store.len());
                Some(store)
            }
            Err(e) => {
                warn!("Doctor AI: could not load learned store: {e}");
                None
            }
        };

        info!("Doctor AI initialized successfully");
        Ok(Self {
            config,
            engine: Arc::new(Mutex::new(engine)),
            learned,
        })
    }

    /// Diagnose a configuration parse error.
    ///
    /// Takes the raw TOML text and the error string, returns a structured
    /// diagnosis with a plain-English explanation.
    pub async fn diagnose_parse_error(&self, raw_toml: &str, error: &str) -> ConfigDiagnosis {
        // Step 1: Fast deterministic classification (no model needed)
        let mut diagnosis = diagnosis::classify_error(error);

        // Step 2: Extract context around the error location
        let excerpt = diagnosis::extract_toml_excerpt(raw_toml, &diagnosis);

        // Step 3: Use AI for the plain-English explanation
        let prompt = prompts::diagnose_prompt(
            &excerpt,
            error,
            diagnosis.field_path.as_deref().unwrap_or("unknown"),
        );
        let ai_output = self.generate(&prompt).await;

        if !ai_output.is_empty() {
            diagnosis.explanation = ai_output.trim().to_string();
        }

        diagnosis
    }

    /// Suggest a concrete fix for a diagnosed config problem.
    ///
    /// Checks the learned store first. If a cached fix exists for this
    /// error+field fingerprint, it is returned immediately without invoking
    /// the LLM. Otherwise falls through to the model, optionally injecting
    /// few-shot examples from recent successful fixes.
    pub async fn suggest_fix(
        &self,
        raw_toml: &str,
        diagnosis: &ConfigDiagnosis,
    ) -> Option<DoctorFix> {
        let field_path = diagnosis.field_path.as_deref().unwrap_or("unknown");

        // Fast path: return cached fix if we have one for this fingerprint.
        if let Some(ref store) = self.learned {
            let fp = LearnedStore::fingerprint(&diagnosis.raw_error, field_path);
            if let Some(cached) = store.lookup(&fp) {
                if let Ok(fix) = serde_json::from_str::<DoctorFix>(&cached.fix_serialized) {
                    tracing::debug!("Doctor AI: returning cached fix for fingerprint {fp}");
                    return Some(fix);
                }
            }
        }

        let current_value = diagnosis::extract_field_value(raw_toml, field_path);
        let kind_str = diagnosis.kind.as_str();

        // Build prompt — inject few-shot examples when available.
        let prompt = if let Some(ref store) = self.learned {
            let examples: Vec<(String, String, String)> = store
                .recent_examples(5)
                .into_iter()
                .map(|e| {
                    (
                        e.field_pattern.clone(),
                        e.diagnosis_kind.clone(),
                        e.fix_description.clone(),
                    )
                })
                .collect();
            if examples.is_empty() {
                prompts::fix_prompt(field_path, &current_value, &diagnosis.raw_error, kind_str)
            } else {
                prompts::fix_prompt_with_examples(
                    field_path,
                    &current_value,
                    &diagnosis.raw_error,
                    kind_str,
                    &examples,
                )
            }
        } else {
            prompts::fix_prompt(field_path, &current_value, &diagnosis.raw_error, kind_str)
        };

        let output = self.generate(&prompt).await;
        repair::parse_fix_suggestion(&output, field_path)
    }

    /// Record a verified successful fix in the learned store.
    ///
    /// Call this after a fix has been applied and validated. Persists to disk
    /// immediately so the fix survives across invocations.
    pub fn record_successful_fix(&mut self, diagnosis: &ConfigDiagnosis, fix: &DoctorFix) {
        if let Some(ref mut store) = self.learned {
            let field_path = diagnosis.field_path.as_deref().unwrap_or("unknown");
            let fingerprint = LearnedStore::fingerprint(&diagnosis.raw_error, field_path);
            let learned_fix = LearnedFix {
                fingerprint,
                diagnosis_kind: diagnosis.kind.as_str().to_string(),
                field_pattern: field_path.to_string(),
                fix_description: fix.describe(),
                fix_serialized: serde_json::to_string(fix).unwrap_or_default(),
                recorded_at: chrono::Utc::now(),
                apply_count: 1,
            };
            store.record(learned_fix);
            if let Err(e) = store.save() {
                tracing::warn!("Failed to save learned fix: {e}");
            }
        }
    }

    /// Explain storage state and recommend migration/recovery steps.
    pub async fn diagnose_storage_report(&self, report: &StorageReport) -> String {
        let summary = storage::summarize_report(report);
        let prompt = prompts::storage_prompt(&summary);
        self.generate(&prompt).await
    }

    /// Check for outdated config fields that need migration.
    pub async fn check_migration(&self, raw_toml: &str) -> Vec<MigrationNote> {
        // Step 1: Check static migration table (high confidence)
        let mut notes = migration::check_static_table(raw_toml);

        // Step 2: Use AI to detect anything the static table missed
        let known_renames = migration::format_known_renames();
        let prompt = prompts::migration_prompt(raw_toml, &known_renames);
        let output = self.generate(&prompt).await;

        let ai_notes = migration::parse_migration_output(&output);
        // Only add AI notes that don't duplicate static ones
        for note in ai_notes {
            if !notes.iter().any(|n| n.old_path == note.old_path) {
                notes.push(note);
            }
        }

        notes
    }

    /// Free-form conversation with the Doctor AI model.
    ///
    /// Formats a system prompt + conversation history + user message,
    /// then generates a response via the local model.
    pub async fn chat(
        &self,
        history: &[(Role, String)],
        user_message: &str,
    ) -> anyhow::Result<String> {
        let prompt = build_chat_prompt(history, user_message);
        let engine = Arc::clone(&self.engine);
        let result = tokio::task::spawn_blocking(move || {
            let mut engine = engine.blocking_lock();
            engine.generate(&prompt)
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join: {e}"))??;
        Ok(result.0.trim().to_string())
    }

    /// Whether auto-fix is enabled in the config.
    pub fn auto_fix_enabled(&self) -> bool {
        self.config.auto_fix
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &DoctorConfig {
        &self.config
    }

    /// Run a prompt through the local model.
    async fn generate(&self, prompt: &str) -> String {
        let prompt = prompt.to_string();
        let engine = Arc::clone(&self.engine);

        let result = tokio::task::spawn_blocking(move || {
            let mut engine = engine.blocking_lock();
            engine.generate(&prompt)
        })
        .await;

        match result {
            Ok(Ok((output, stats))) => {
                tracing::debug!(
                    "Doctor AI: {} tokens in {}ms ({:.1} tok/s)",
                    stats.tokens_generated,
                    stats.generation_time_ms,
                    stats.tokens_per_second
                );
                output
            }
            Ok(Err(e)) => {
                warn!("Doctor AI inference error: {e}");
                String::new()
            }
            Err(e) => {
                warn!("Doctor AI task join error: {e}");
                String::new()
            }
        }
    }
}

/// Build a ChatML-formatted prompt for free-form Doctor AI conversation.
fn build_chat_prompt(history: &[(Role, String)], user_message: &str) -> String {
    let system = "You are Doctor, RockBot's config diagnostician. Clinical, exact, mildly exasperated by bad configs.";
    let mut prompt = format!("<|im_start|>system\n{system}<|im_end|>\n");
    for (role, content) in history {
        let tag = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let content = content
            .replace("<|im_start|>", "[im_start]")
            .replace("<|im_end|>", "[im_end]");
        prompt.push_str(&format!("<|im_start|>{tag}\n{content}<|im_end|>\n"));
    }
    let user_message = user_message
        .replace("<|im_start|>", "[im_start]")
        .replace("<|im_end|>", "[im_end]");
    prompt.push_str(&format!(
        "<|im_start|>user\n{user_message}<|im_end|>\n<|im_start|>assistant\n"
    ));
    prompt
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DoctorConfig::default();
        assert_eq!(config.model_id, "Qwen/Qwen2.5-1.5B-Instruct-GGUF");
        assert_eq!(config.max_tokens, 512);
        assert!(!config.auto_fix);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = DoctorConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: DoctorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model_id, config.model_id);
        assert_eq!(parsed.max_tokens, config.max_tokens);
        assert_eq!(parsed.temperature, config.temperature);
    }
}
