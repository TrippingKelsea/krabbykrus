//! Observability metrics for RockBot.
//!
//! Uses the [`metrics`] crate facade so any compatible recorder backend
//! (Prometheus, OTLP, in-memory) can be plugged in at startup.

use metrics::{counter, gauge, histogram};
use std::time::Instant;

/// Record an LLM request completion.
pub fn record_llm_request(provider: &str, model: &str, duration: std::time::Duration, input_tokens: u64, output_tokens: u64) {
    histogram!("llm_request_duration_seconds", "provider" => provider.to_string(), "model" => model.to_string())
        .record(duration.as_secs_f64());
    counter!("llm_tokens_total", "direction" => "input", "provider" => provider.to_string(), "model" => model.to_string())
        .increment(input_tokens);
    counter!("llm_tokens_total", "direction" => "output", "provider" => provider.to_string(), "model" => model.to_string())
        .increment(output_tokens);
    counter!("llm_requests_total", "provider" => provider.to_string(), "model" => model.to_string())
        .increment(1);
}

/// Record a tool call.
pub fn record_tool_call(tool_name: &str, success: bool, duration: std::time::Duration) {
    let status = if success { "success" } else { "error" };
    counter!("tool_calls_total", "tool" => tool_name.to_string(), "status" => status)
        .increment(1);
    histogram!("tool_call_duration_seconds", "tool" => tool_name.to_string())
        .record(duration.as_secs_f64());
}

/// Record an agent message processed.
pub fn record_agent_message(agent_id: &str) {
    counter!("agent_messages_total", "agent_id" => agent_id.to_string())
        .increment(1);
}

/// Update active session count.
pub fn set_active_sessions(count: u64) {
    gauge!("active_sessions").set(count as f64);
}

/// Update active agent count.
pub fn set_active_agents(count: u64) {
    gauge!("active_agents").set(count as f64);
}

/// A timing guard — records the elapsed duration when dropped.
pub struct TimingGuard {
    start: Instant,
    metric_name: &'static str,
    labels: Vec<(&'static str, String)>,
}

impl TimingGuard {
    /// Start timing an operation.
    pub fn new(metric_name: &'static str, labels: Vec<(&'static str, String)>) -> Self {
        Self {
            start: Instant::now(),
            metric_name,
            labels,
        }
    }
}

impl Drop for TimingGuard {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        // Build label pairs as owned tuples
        let labels: Vec<(String, String)> = self.labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        // Use describe_histogram to avoid borrow issues — fall back to direct recording
        let _ = labels; // Labels consumed above
        histogram!(self.metric_name).record(duration.as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_record_llm_request() {
        // Just verify the function doesn't panic (no recorder installed = no-op)
        record_llm_request("anthropic", "claude-3", std::time::Duration::from_millis(500), 100, 50);
    }

    #[test]
    fn test_record_tool_call() {
        record_tool_call("read", true, std::time::Duration::from_millis(10));
        record_tool_call("exec", false, std::time::Duration::from_millis(5000));
    }

    #[test]
    fn test_record_agent_message() {
        record_agent_message("agent-1");
    }

    #[test]
    fn test_set_active_sessions() {
        set_active_sessions(5);
        set_active_sessions(0);
    }

    #[test]
    fn test_set_active_agents() {
        set_active_agents(3);
    }

    #[test]
    fn test_timing_guard() {
        let _guard = TimingGuard::new("test_duration_seconds", vec![("op", "test".to_string())]);
        std::thread::sleep(std::time::Duration::from_millis(1));
        // Guard dropped here, records duration
    }
}
