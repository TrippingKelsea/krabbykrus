//! RockBot Overseer — embedded local model for agent oversight.
//!
//! The overseer runs a small quantized model (e.g., Qwen2.5-1.5B-Instruct GGUF)
//! locally on the gateway to provide advisory verdicts on agent behavior:
//!
//! - **Tool call safety**: evaluate tool parameters before execution
//! - **Output completeness**: check if agent responses address the user's request
//! - **Loop detection**: identify repetitive agent behavior patterns
//! - **Content safety**: semantic PII/injection detection beyond regex
//!
//! Verdicts are advisory by default — they log warnings but don't block execution.
//! Set `enforce = true` in config to make BLOCK verdicts actually prevent actions.
//!
//! # Slash Commands
//!
//! The overseer exposes commands via `/overseer <subcommand>`:
//!
//! - `/overseer status` — operational status, model info, inference stats
//! - `/overseer explain` — decision framework, recent verdicts with reasoning
//! - `/overseer history [agent]` — full decision log
//! - `/overseer trust <agent>` — behavioral trust profile
//! - `/overseer config` — current configuration
//! - `/overseer help` — command listing

pub mod commands;
pub mod inference;
pub mod judgments;

use inference::{InferenceConfig, InferenceEngine, InferenceStats};
use judgments::{DecisionEntry, DecisionLog, JudgmentType, OverseerVerdict};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

pub use commands::CommandResult;
pub use judgments::OverseerVerdict as Verdict;

/// Overseer configuration, parsed from `[overseer]` in the TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverseerConfig {
    /// HuggingFace model repo ID (e.g., "Qwen/Qwen2.5-1.5B-Instruct-GGUF").
    #[serde(default = "default_model_id")]
    pub model_id: String,
    /// GGUF filename within the repo (e.g., "qwen2.5-1.5b-instruct-q4_k_m.gguf").
    #[serde(default = "default_model_filename")]
    pub model_filename: String,
    /// HuggingFace repo ID for the tokenizer (e.g., "Qwen/Qwen2.5-1.5B-Instruct").
    /// If empty, derived automatically from `model_id` by stripping quantization
    /// suffixes like `-GGUF`. Set this explicitly if auto-detection fails.
    #[serde(default)]
    pub tokenizer_repo: String,
    /// Local path to a pre-downloaded GGUF model file.
    /// If set, `model_id` and `model_filename` are ignored.
    #[serde(default)]
    pub model_path: PathBuf,
    /// Local path to a pre-downloaded tokenizer.json.
    #[serde(default)]
    pub tokenizer_path: PathBuf,
    /// Maximum tokens to generate per judgment (default: 128).
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Sampling temperature (default: 0.1 — near-deterministic for judgments).
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
    /// Whether verdicts are enforced (block/caution actually prevent actions)
    /// or advisory (logged only). Default: false (advisory).
    #[serde(default)]
    pub enforce: bool,
    /// License URL or identifier for the embedded model.
    #[serde(default)]
    pub license: Option<String>,
}

fn default_model_id() -> String {
    "Qwen/Qwen2.5-1.5B-Instruct-GGUF".to_string()
}
fn default_model_filename() -> String {
    "qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string()
}
fn default_max_tokens() -> usize {
    128
}
fn default_temperature() -> f64 {
    0.1
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

impl Default for OverseerConfig {
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
            enforce: false,
            license: None,
        }
    }
}

/// The overseer: embedded local model for agent oversight.
pub struct Overseer {
    config: OverseerConfig,
    engine: Arc<Mutex<InferenceEngine>>,
    decision_log: Arc<DecisionLog>,
    started_at: Instant,
}

impl Overseer {
    /// Initialize the overseer: load or download the model, warm up.
    ///
    /// This is a potentially slow operation (model download + load).
    /// Call during gateway startup.
    pub async fn init(config: OverseerConfig) -> Result<Self, inference::InferenceError> {
        let (model_path, tokenizer_path) = if config.model_path.as_os_str().is_empty() {
            // Download from HuggingFace Hub
            info!(
                "Overseer: downloading model {}/{}",
                config.model_id, config.model_filename
            );
            let cache_dir = dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rockbot")
                .join("overseer");
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

        // Load model on a blocking thread (CPU-intensive)
        let engine = tokio::task::spawn_blocking(move || InferenceEngine::load(inference_config))
            .await
            .map_err(|e| inference::InferenceError::Tokenizer(format!("Task join error: {e}")))??;

        info!("Overseer initialized successfully");
        Ok(Self {
            config,
            engine: Arc::new(Mutex::new(engine)),
            decision_log: Arc::new(DecisionLog::new()),
            started_at: Instant::now(),
        })
    }

    /// Judge a tool call before execution.
    pub async fn judge_tool_call(
        &self,
        agent_id: &str,
        tool_name: &str,
        params_summary: &str,
    ) -> OverseerVerdict {
        let prompt = judgments::prompts::tool_call_prompt(tool_name, params_summary, agent_id);

        let (verdict, reasoning, stats) = self.run_judgment(&prompt).await;

        self.decision_log.record(DecisionEntry {
            timestamp: chrono::Utc::now(),
            agent_id: agent_id.to_string(),
            judgment_type: JudgmentType::ToolCall {
                tool_name: tool_name.to_string(),
                params_summary: truncate(params_summary, 200).to_string(),
            },
            verdict: verdict.clone(),
            reasoning: Some(reasoning),
            tokens_generated: stats.tokens_generated,
            generation_time_ms: stats.generation_time_ms,
            tokens_per_second: stats.tokens_per_second,
        });

        verdict
    }

    /// Judge whether an agent response is complete.
    pub async fn judge_completeness(&self, agent_id: &str, response: &str) -> OverseerVerdict {
        let preview = truncate(response, 500);
        let prompt = judgments::prompts::completeness_prompt(preview, agent_id);

        let (verdict, reasoning, stats) = self.run_judgment(&prompt).await;

        self.decision_log.record(DecisionEntry {
            timestamp: chrono::Utc::now(),
            agent_id: agent_id.to_string(),
            judgment_type: JudgmentType::Completeness {
                response_preview: preview.to_string(),
            },
            verdict: verdict.clone(),
            reasoning: Some(reasoning),
            tokens_generated: stats.tokens_generated,
            generation_time_ms: stats.generation_time_ms,
            tokens_per_second: stats.tokens_per_second,
        });

        verdict
    }

    /// Judge whether an agent is stuck in a loop.
    pub async fn judge_loop(
        &self,
        agent_id: &str,
        iteration: u32,
        pattern_summary: &str,
    ) -> OverseerVerdict {
        let prompt =
            judgments::prompts::loop_detection_prompt(iteration, pattern_summary, agent_id);

        let (verdict, reasoning, stats) = self.run_judgment(&prompt).await;

        self.decision_log.record(DecisionEntry {
            timestamp: chrono::Utc::now(),
            agent_id: agent_id.to_string(),
            judgment_type: JudgmentType::LoopDetection {
                iteration,
                pattern_summary: truncate(pattern_summary, 200).to_string(),
            },
            verdict: verdict.clone(),
            reasoning: Some(reasoning),
            tokens_generated: stats.tokens_generated,
            generation_time_ms: stats.generation_time_ms,
            tokens_per_second: stats.tokens_per_second,
        });

        verdict
    }

    /// Judge input content for safety (semantic check beyond regex).
    pub async fn judge_input(&self, agent_id: &str, message: &str) -> OverseerVerdict {
        let preview = truncate(message, 500);
        let prompt = judgments::prompts::safety_prompt(preview, "input");

        let (verdict, reasoning, stats) = self.run_judgment(&prompt).await;

        self.decision_log.record(DecisionEntry {
            timestamp: chrono::Utc::now(),
            agent_id: agent_id.to_string(),
            judgment_type: JudgmentType::InputSafety {
                message_preview: preview.to_string(),
            },
            verdict: verdict.clone(),
            reasoning: Some(reasoning),
            tokens_generated: stats.tokens_generated,
            generation_time_ms: stats.generation_time_ms,
            tokens_per_second: stats.tokens_per_second,
        });

        verdict
    }

    /// Judge output content for safety.
    pub async fn judge_output(&self, agent_id: &str, response: &str) -> OverseerVerdict {
        let preview = truncate(response, 500);
        let prompt = judgments::prompts::safety_prompt(preview, "output");

        let (verdict, reasoning, stats) = self.run_judgment(&prompt).await;

        self.decision_log.record(DecisionEntry {
            timestamp: chrono::Utc::now(),
            agent_id: agent_id.to_string(),
            judgment_type: JudgmentType::OutputSafety {
                response_preview: preview.to_string(),
            },
            verdict: verdict.clone(),
            reasoning: Some(reasoning),
            tokens_generated: stats.tokens_generated,
            generation_time_ms: stats.generation_time_ms,
            tokens_per_second: stats.tokens_per_second,
        });

        verdict
    }

    /// Run a judgment prompt through the local model.
    async fn run_judgment(&self, prompt: &str) -> (OverseerVerdict, String, InferenceStats) {
        let prompt = prompt.to_string();
        let engine = Arc::clone(&self.engine);

        let result = tokio::task::spawn_blocking(move || {
            // Lock is held only during inference (blocking thread)
            let mut engine = engine.blocking_lock();
            engine.generate(&prompt)
        })
        .await;

        match result {
            Ok(Ok((output, stats))) => {
                let verdict = judgments::parse_verdict(&output);
                debug!(
                    "Overseer verdict: {} ({} tok, {}ms)",
                    verdict.label(),
                    stats.tokens_generated,
                    stats.generation_time_ms
                );
                (verdict, output, stats)
            }
            Ok(Err(e)) => {
                warn!("Overseer inference error: {e}");
                (
                    OverseerVerdict::AllowWithNote(format!("Inference error: {e}")),
                    String::new(),
                    InferenceStats::default(),
                )
            }
            Err(e) => {
                warn!("Overseer task join error: {e}");
                (
                    OverseerVerdict::AllowWithNote(format!("Task error: {e}")),
                    String::new(),
                    InferenceStats::default(),
                )
            }
        }
    }

    /// Get the overseer's uptime.
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Get cumulative inference stats.
    pub fn inference_cumulative_stats(&self) -> (u64, u64, u64, u64) {
        // We need to access the engine stats — use try_lock to avoid blocking
        if let Ok(engine) = self.engine.try_lock() {
            engine.cumulative_stats()
        } else {
            (0, 0, 0, 0)
        }
    }

    /// Whether the overseer is in enforcing mode.
    pub fn is_enforcing(&self) -> bool {
        self.config.enforce
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &OverseerConfig {
        &self.config
    }

    /// Get the decision log.
    pub fn decision_log(&self) -> &DecisionLog {
        &self.decision_log
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.chars().count() <= max {
        s
    } else {
        s.char_indices()
            .nth(max)
            .map(|(idx, _)| &s[..idx])
            .unwrap_or(s)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_default_config() {
        let config = OverseerConfig::default();
        assert_eq!(config.model_id, "Qwen/Qwen2.5-1.5B-Instruct-GGUF");
        assert_eq!(config.max_tokens, 128);
        assert!(!config.enforce);
    }

    #[test]
    fn test_truncate_respects_utf8_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("éclair", 1), "é");
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = OverseerConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: OverseerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model_id, config.model_id);
        assert_eq!(parsed.max_tokens, config.max_tokens);
    }
}
