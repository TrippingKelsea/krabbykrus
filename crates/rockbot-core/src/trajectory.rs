//! Structured trajectory recording for agent execution.
//!
//! Records every step of an agent's processing loop as a structured log
//! for debugging, replay, and evaluation.

use crate::agent::TokenUsage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single entry in an agent execution trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEntry {
    /// When this event occurred.
    pub timestamp: DateTime<Utc>,
    /// The event that occurred.
    pub event: TrajectoryEvent,
    /// Current tool-loop iteration (0 = before loop).
    pub iteration: usize,
    /// Cumulative token usage at this point.
    pub cumulative_tokens: u64,
}

/// Events recorded in the trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TrajectoryEvent {
    /// User message received.
    #[serde(rename = "user_message")]
    UserMessage {
        content_preview: String,
    },
    /// System prompt assembled.
    #[serde(rename = "system_prompt")]
    SystemPrompt {
        length_chars: usize,
    },
    /// LLM request sent.
    #[serde(rename = "llm_request")]
    LlmRequest {
        model: String,
        message_count: usize,
        tools_available: usize,
    },
    /// LLM response received.
    #[serde(rename = "llm_response")]
    LlmResponse {
        content_preview: String,
        tool_call_names: Vec<String>,
        tokens: TokenUsage,
    },
    /// Tool execution started.
    #[serde(rename = "tool_start")]
    ToolStart {
        tool_name: String,
        params_preview: String,
    },
    /// Tool execution completed.
    #[serde(rename = "tool_done")]
    ToolDone {
        tool_name: String,
        success: bool,
        duration_ms: u64,
        result_preview: String,
    },
    /// Continuation nudge sent.
    #[serde(rename = "nudge")]
    Nudge {
        nudge_type: String,
        consecutive_count: u32,
    },
    /// Context compaction performed.
    #[serde(rename = "compaction")]
    Compaction {
        messages_before: usize,
        messages_after: usize,
        tokens_after: usize,
    },
    /// Guardrail check result.
    #[serde(rename = "guardrail")]
    Guardrail {
        name: String,
        direction: String,
        result: String,
    },
    /// Loop detector verdict.
    #[serde(rename = "loop_verdict")]
    LoopVerdict {
        verdict: String,
    },
    /// Reflection pass.
    #[serde(rename = "reflection")]
    Reflection {
        action: String,
    },
    /// Error during processing.
    #[serde(rename = "error")]
    Error {
        message: String,
    },
    /// Agent processing complete.
    #[serde(rename = "complete")]
    Complete {
        total_iterations: usize,
        total_tool_calls: usize,
        final_tokens: TokenUsage,
        duration_ms: u64,
    },
}

/// A complete trajectory for one agent interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trajectory {
    /// Session this trajectory belongs to.
    pub session_id: String,
    /// Agent that produced this trajectory.
    pub agent_id: String,
    /// When the interaction started.
    pub started_at: DateTime<Utc>,
    /// All recorded events, in order.
    pub entries: Vec<TrajectoryEntry>,
}

impl Trajectory {
    /// Create a new trajectory for the given session and agent.
    pub fn new(session_id: &str, agent_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            started_at: Utc::now(),
            entries: Vec::new(),
        }
    }

    /// Record an event at the current time.
    pub fn record(&mut self, event: TrajectoryEvent, iteration: usize, cumulative_tokens: u64) {
        self.entries.push(TrajectoryEntry {
            timestamp: Utc::now(),
            event,
            iteration,
            cumulative_tokens,
        });
    }

    /// Total duration from first to last entry.
    pub fn duration_ms(&self) -> u64 {
        if self.entries.len() < 2 {
            return 0;
        }
        let first = self.entries.first().map_or_else(Utc::now, |e| e.timestamp);
        let last = self.entries.last().map_or_else(Utc::now, |e| e.timestamp);
        (last - first).num_milliseconds().max(0) as u64
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the trajectory has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serialize to JSON lines (one JSON object per line).
    pub fn to_jsonl(&self) -> String {
        self.entries.iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Truncate a string to `max_len` chars for preview purposes.
pub fn preview(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_trajectory_new() {
        let t = Trajectory::new("sess-1", "agent-1");
        assert_eq!(t.session_id, "sess-1");
        assert_eq!(t.agent_id, "agent-1");
        assert!(t.is_empty());
    }

    #[test]
    fn test_trajectory_record() {
        let mut t = Trajectory::new("s", "a");
        t.record(TrajectoryEvent::UserMessage {
            content_preview: "hello".to_string(),
        }, 0, 0);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn test_trajectory_to_jsonl() {
        let mut t = Trajectory::new("s", "a");
        t.record(TrajectoryEvent::UserMessage {
            content_preview: "hello".to_string(),
        }, 0, 0);
        t.record(TrajectoryEvent::Error {
            message: "oops".to_string(),
        }, 1, 100);
        let jsonl = t.to_jsonl();
        assert_eq!(jsonl.lines().count(), 2);
    }

    #[test]
    fn test_preview() {
        assert_eq!(preview("short", 10), "short");
        assert_eq!(preview("a long string here", 6), "a long...");
    }

    #[test]
    fn test_trajectory_event_serialization() {
        let event = TrajectoryEvent::ToolDone {
            tool_name: "read".to_string(),
            success: true,
            duration_ms: 42,
            result_preview: "file contents".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_done\""));
        assert!(json.contains("\"success\":true"));
    }
}
