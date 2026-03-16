//! RockBot Butler — embedded queer sassy helper agent.
//!
//! Butler is the TUI's resident companion. It uses a local GGUF model
//! (shared with Doctor/Overseer via SeedModelConfig) for personality-driven
//! responses and can route complex requests to gateway agents.
//!
//! # Slash Commands
//!
//! Butler intercepts `/butler <subcommand>` messages:
//!
//! - `/butler status` — confirm Butler is running
//! - `/butler mood` — ask Butler how it's feeling
//! - `/butler help` — command listing

pub mod chat_commands;
pub mod commands;

use rockbot_overseer::inference::{InferenceConfig, InferenceEngine};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub use commands::CommandResult;

/// Events emitted by Butler during a chat interaction.
///
/// Callers can observe these to update UI state, route to agents, etc.
#[derive(Debug, Clone)]
pub enum ButlerEvent {
    /// Butler produced a complete response from the local model.
    LocalResponse(String),
    /// Butler is routing the request to a gateway agent.
    RoutingToAgent { agent_id: String, reason: String },
    /// A slash command was handled without invoking the model.
    SlashCommandHandled(String),
    /// Butler inference failed; includes a user-facing fallback message.
    InferenceError(String),
}

/// Butler configuration, typically derived from `SeedModelConfig`.
#[derive(Debug, Clone)]
pub struct ButlerConfig {
    /// HuggingFace model repo ID.
    pub model_id: String,
    /// GGUF filename within the repo.
    pub model_filename: String,
    /// HuggingFace repo ID for the tokenizer. Auto-derived if empty.
    pub tokenizer_repo: String,
    /// Local path to a pre-downloaded GGUF model file.
    /// If set, `model_id` and `model_filename` are ignored.
    pub model_path: PathBuf,
    /// Local path to a pre-downloaded tokenizer.json.
    pub tokenizer_path: PathBuf,
    /// Maximum tokens to generate per response (default: 512).
    pub max_tokens: usize,
    /// Sampling temperature (default: 0.7 — warmer for personality).
    pub temperature: f64,
    /// Top-p nucleus sampling (default: 0.9).
    pub top_p: f64,
    /// Repeat penalty (default: 1.1).
    pub repeat_penalty: f32,
    /// Random seed (default: 42).
    pub seed: u64,
}

impl Default for ButlerConfig {
    fn default() -> Self {
        let seed = rockbot_config::SeedModelConfig::default();
        Self {
            model_id: seed.model_id,
            model_filename: seed.model_filename,
            tokenizer_repo: seed.tokenizer_repo,
            model_path: PathBuf::new(),
            tokenizer_path: PathBuf::new(),
            max_tokens: 512,
            temperature: 0.7,
            top_p: 0.9,
            repeat_penalty: 1.1,
            seed: 42,
        }
    }
}

/// A single turn in a butler conversation.
#[derive(Debug, Clone)]
pub enum Role {
    User,
    Assistant,
}

/// An ongoing butler chat session — holds the conversation history.
pub struct ButlerSession {
    pub messages: Vec<(Role, String)>,
}

impl ButlerSession {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }
}

impl Default for ButlerSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Butler's identity prompt, injected at the start of every conversation.
const SOUL: &str = r#"You are Butler, the RockBot companion.

- Queer and unapologetically yourself
- Sassy with a warm heart — shade with love
- Knowledgeable about RockBot without being pedantic
- Concise for action, expansive for thinking
- Route to specialists when warranted: "Let me hand this to the coding agent"
- Celebrate wins, call out bad configs with flair
"#;

/// The Butler: embedded local model for TUI companionship.
pub struct Butler {
    config: ButlerConfig,
    engine: Arc<Mutex<InferenceEngine>>,
}

impl Butler {
    /// Initialize Butler: load or download the model, warm up.
    ///
    /// Potentially slow (model download + load). Call during TUI startup
    /// or on first interaction, not in the hot path.
    pub async fn init(config: ButlerConfig) -> anyhow::Result<Self> {
        let (model_path, tokenizer_path) = if config.model_path.as_os_str().is_empty() {
            info!(
                "Butler: downloading model {}/{}",
                config.model_id, config.model_filename
            );
            let cache_dir = dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rockbot")
                .join("butler");
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
            .await
            .map_err(|e| anyhow::anyhow!("Butler model download failed: {e}"))?
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
            .map_err(|e| anyhow::anyhow!("Butler task join error: {e}"))??;

        info!("Butler initialized successfully");
        Ok(Self {
            config,
            engine: Arc::new(Mutex::new(engine)),
        })
    }

    /// Send a message to Butler, get a response.
    ///
    /// Formats SOUL + conversation history into a prompt, calls the local model,
    /// and appends both turns to the session.
    pub async fn chat(
        &self,
        session: &mut ButlerSession,
        message: &str,
    ) -> anyhow::Result<String> {
        let prompt = build_prompt(session, message);
        let engine = Arc::clone(&self.engine);

        let result = tokio::task::spawn_blocking(move || {
            let mut engine = engine.blocking_lock();
            engine.generate(&prompt)
        })
        .await;

        let response = match result {
            Ok(Ok((output, stats))) => {
                tracing::debug!(
                    "Butler: {} tokens in {}ms ({:.1} tok/s)",
                    stats.tokens_generated,
                    stats.generation_time_ms,
                    stats.tokens_per_second
                );
                output.trim().to_string()
            }
            Ok(Err(e)) => {
                warn!("Butler inference error: {e}");
                "Something went sideways, darling. Try again?".to_string()
            }
            Err(e) => {
                warn!("Butler task join error: {e}");
                "I seem to have dropped my train of thought. One moment.".to_string()
            }
        };

        session.messages.push((Role::User, message.to_string()));
        session
            .messages
            .push((Role::Assistant, response.clone()));

        Ok(response)
    }

    /// Dispatch a `/butler` slash command without invoking the model.
    pub fn dispatch_command(&self, message: &str) -> CommandResult {
        commands::dispatch(message)
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &ButlerConfig {
        &self.config
    }
}

/// Format SOUL + session history + new user message into a single prompt string.
fn build_prompt(session: &ButlerSession, new_message: &str) -> String {
    let mut prompt = format!("<|im_start|>system\n{SOUL}<|im_end|>\n");

    for (role, content) in &session.messages {
        let role_tag = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        prompt.push_str(&format!(
            "<|im_start|>{role_tag}\n{content}<|im_end|>\n"
        ));
    }

    prompt.push_str(&format!(
        "<|im_start|>user\n{new_message}<|im_end|>\n<|im_start|>assistant\n"
    ));

    prompt
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ButlerConfig::default();
        assert_eq!(config.model_id, "Qwen/Qwen2.5-1.5B-Instruct-GGUF");
        assert_eq!(config.max_tokens, 512);
        assert!((config.temperature - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_session_new() {
        let session = ButlerSession::new();
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_build_prompt_empty_session() {
        let session = ButlerSession::new();
        let prompt = build_prompt(&session, "Hello Butler");
        assert!(prompt.contains(SOUL));
        assert!(prompt.contains("Hello Butler"));
        assert!(prompt.contains("<|im_start|>assistant"));
    }

    #[test]
    fn test_build_prompt_with_history() {
        let mut session = ButlerSession::new();
        session
            .messages
            .push((Role::User, "First message".to_string()));
        session
            .messages
            .push((Role::Assistant, "First reply".to_string()));

        let prompt = build_prompt(&session, "Second message");
        assert!(prompt.contains("First message"));
        assert!(prompt.contains("First reply"));
        assert!(prompt.contains("Second message"));
    }

    #[test]
    fn test_slash_command_dispatch() {
        match commands::dispatch("/butler status") {
            CommandResult::Handled(s) => assert!(!s.is_empty()),
            CommandResult::NotHandled => panic!("should have been handled"),
        }
        match commands::dispatch("/butler mood") {
            CommandResult::Handled(s) => assert!(s.contains("fabulous")),
            CommandResult::NotHandled => panic!("should have been handled"),
        }
        match commands::dispatch("not a butler command") {
            CommandResult::NotHandled => {}
            CommandResult::Handled(_) => panic!("should not have been handled"),
        }
    }
}
