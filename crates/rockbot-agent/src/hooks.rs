//! Hook/middleware system for agent lifecycle events.
//!
//! Hooks fire at key points in the agent processing pipeline and can
//! observe, modify, or abort operations.

use rockbot_config::Message;
use serde_json::Value;
use std::sync::Arc;

/// Events that hooks can respond to.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// Before an incoming user message is processed.
    PreMessage {
        agent_id: String,
        session_id: String,
        message: Message,
    },
    /// After the agent produces a response message.
    PostMessage {
        agent_id: String,
        session_id: String,
        response: Message,
    },
    /// Before a tool call is executed.
    PreToolCall {
        agent_id: String,
        session_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// After a tool call completes.
    PostToolCall {
        agent_id: String,
        session_id: String,
        tool_name: String,
        result: Value,
        success: bool,
    },
    /// When an error occurs during processing.
    OnError {
        agent_id: String,
        session_id: String,
        error: String,
    },
}

/// Result of hook execution, controlling the pipeline.
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue processing normally.
    Continue,
    /// Continue but with modified event data (e.g. rewritten message).
    Modify(Value),
    /// Abort processing with the given reason.
    Abort { reason: String },
}

/// Trait for implementing hooks.
///
/// Hooks are called in registration order. If any hook returns `Abort`,
/// subsequent hooks are skipped and the operation is cancelled.
#[async_trait::async_trait]
pub trait Hook: Send + Sync {
    /// Human-readable name for this hook (used in logging).
    fn name(&self) -> &str;

    /// Called for each lifecycle event. Return `Continue` to proceed,
    /// `Modify` to alter data, or `Abort` to cancel.
    async fn on_event(&self, event: &HookEvent) -> HookResult;
}

/// Registry that manages hooks and fires them in order.
pub struct HookRegistry {
    hooks: Vec<Arc<dyn Hook>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook. Hooks fire in registration order.
    pub fn register(&mut self, hook: Arc<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Remove all hooks.
    pub fn clear(&mut self) {
        self.hooks.clear();
    }

    /// Number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether the registry has no hooks.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Fire an event through all registered hooks.
    ///
    /// Returns `Continue` if all hooks pass, `Modify` with the last modification
    /// if any hook modifies data, or `Abort` if any hook aborts.
    pub async fn fire(&self, event: &HookEvent) -> HookResult {
        let mut last_modify: Option<Value> = None;

        for hook in &self.hooks {
            match hook.on_event(event).await {
                HookResult::Continue => {}
                HookResult::Modify(data) => {
                    last_modify = Some(data);
                }
                HookResult::Abort { reason } => {
                    tracing::info!("Hook '{}' aborted event: {reason}", hook.name());
                    return HookResult::Abort { reason };
                }
            }
        }

        match last_modify {
            Some(data) => HookResult::Modify(data),
            None => HookResult::Continue,
        }
    }
}

/// A logging hook that logs all events at debug level.
pub struct LoggingHook;

#[async_trait::async_trait]
impl Hook for LoggingHook {
    fn name(&self) -> &str {
        "logging"
    }

    async fn on_event(&self, event: &HookEvent) -> HookResult {
        match event {
            HookEvent::PreMessage {
                agent_id,
                session_id,
                ..
            } => {
                tracing::debug!("[hook:logging] PreMessage agent={agent_id} session={session_id}");
            }
            HookEvent::PostMessage {
                agent_id,
                session_id,
                ..
            } => {
                tracing::debug!("[hook:logging] PostMessage agent={agent_id} session={session_id}");
            }
            HookEvent::PreToolCall {
                agent_id,
                tool_name,
                ..
            } => {
                tracing::debug!("[hook:logging] PreToolCall agent={agent_id} tool={tool_name}");
            }
            HookEvent::PostToolCall {
                agent_id,
                tool_name,
                success,
                ..
            } => {
                tracing::debug!("[hook:logging] PostToolCall agent={agent_id} tool={tool_name} success={success}");
            }
            HookEvent::OnError {
                agent_id, error, ..
            } => {
                tracing::warn!("[hook:logging] OnError agent={agent_id}: {error}");
            }
        }
        HookResult::Continue
    }
}

/// A metrics hook that records hook-related counters.
pub struct MetricsHook;

#[async_trait::async_trait]
impl Hook for MetricsHook {
    fn name(&self) -> &str {
        "metrics"
    }

    async fn on_event(&self, event: &HookEvent) -> HookResult {
        match event {
            HookEvent::PreMessage { agent_id, .. } => {
                crate::metrics::record_agent_message(agent_id);
            }
            HookEvent::PostToolCall {
                tool_name, success, ..
            } => {
                crate::metrics::record_tool_call(
                    tool_name,
                    *success,
                    std::time::Duration::ZERO, // Hook doesn't have timing info
                );
            }
            _ => {}
        }
        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingHook {
        count: AtomicUsize,
        result: HookResult,
    }

    impl CountingHook {
        fn new(result: HookResult) -> Self {
            Self {
                count: AtomicUsize::new(0),
                result,
            }
        }

        fn call_count(&self) -> usize {
            self.count.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl Hook for CountingHook {
        fn name(&self) -> &str {
            "counting"
        }

        async fn on_event(&self, _event: &HookEvent) -> HookResult {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.result.clone()
        }
    }

    fn test_event() -> HookEvent {
        HookEvent::PreMessage {
            agent_id: "test-agent".to_string(),
            session_id: "test-session".to_string(),
            message: Message::text("hello"),
        }
    }

    #[tokio::test]
    async fn test_hook_firing_order() {
        let mut registry = HookRegistry::new();

        let h1 = Arc::new(CountingHook::new(HookResult::Continue));
        let h2 = Arc::new(CountingHook::new(HookResult::Continue));

        registry.register(h1.clone());
        registry.register(h2.clone());

        let result = registry.fire(&test_event()).await;
        assert!(matches!(result, HookResult::Continue));
        assert_eq!(h1.call_count(), 1);
        assert_eq!(h2.call_count(), 1);
    }

    #[tokio::test]
    async fn test_hook_abort_stops_chain() {
        let mut registry = HookRegistry::new();

        let h1 = Arc::new(CountingHook::new(HookResult::Abort {
            reason: "blocked".to_string(),
        }));
        let h2 = Arc::new(CountingHook::new(HookResult::Continue));

        registry.register(h1.clone());
        registry.register(h2.clone());

        let result = registry.fire(&test_event()).await;
        assert!(matches!(result, HookResult::Abort { .. }));
        assert_eq!(h1.call_count(), 1);
        assert_eq!(h2.call_count(), 0); // Should not have been called
    }

    #[tokio::test]
    async fn test_hook_modify() {
        let mut registry = HookRegistry::new();
        let h = Arc::new(CountingHook::new(HookResult::Modify(
            serde_json::json!({"modified": true}),
        )));
        registry.register(h);

        let result = registry.fire(&test_event()).await;
        assert!(matches!(result, HookResult::Modify(_)));
    }

    #[tokio::test]
    async fn test_empty_registry() {
        let registry = HookRegistry::new();
        assert!(registry.is_empty());
        let result = registry.fire(&test_event()).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn test_logging_hook() {
        let hook = LoggingHook;
        let result = hook.on_event(&test_event()).await;
        assert!(matches!(result, HookResult::Continue));
    }
}
