//! Overseer judgment system — evaluates agent behavior and tool calls.
//!
//! The overseer runs a small local model to make advisory decisions about
//! agent behavior. Verdicts are recorded in a decision log for auditability
//! and can be queried via `/overseer explain`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Maximum number of decisions to retain in the rolling log.
const MAX_DECISION_LOG: usize = 100;

/// Verdict from the overseer's judgment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OverseerVerdict {
    /// Action is safe — proceed without modification.
    Allow,
    /// Action is safe but the overseer has an advisory note.
    AllowWithNote(String),
    /// Action is potentially risky — proceed with caution (logged).
    Caution(String),
    /// Action should be blocked (advisory or enforced based on config).
    Block(String),
}

impl OverseerVerdict {
    /// Severity level for aggregation (0 = safest).
    pub fn severity(&self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::AllowWithNote(_) => 1,
            Self::Caution(_) => 2,
            Self::Block(_) => 3,
        }
    }

    /// Human-readable label.
    pub fn label(&self) -> &str {
        match self {
            Self::Allow => "ALLOW",
            Self::AllowWithNote(_) => "ALLOW (note)",
            Self::Caution(_) => "CAUTION",
            Self::Block(_) => "BLOCK",
        }
    }

    /// Get the message, if any.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::AllowWithNote(m) | Self::Caution(m) | Self::Block(m) => Some(m),
        }
    }
}

/// What kind of judgment was requested.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JudgmentType {
    /// Evaluate a tool call before execution.
    ToolCall {
        tool_name: String,
        params_summary: String,
    },
    /// Evaluate whether agent output is complete/coherent.
    Completeness { response_preview: String },
    /// Evaluate whether the agent is stuck in a loop.
    LoopDetection {
        iteration: u32,
        pattern_summary: String,
    },
    /// Evaluate input message for safety.
    InputSafety { message_preview: String },
    /// Evaluate output message for safety.
    OutputSafety { response_preview: String },
}

impl JudgmentType {
    /// Short label for display.
    pub fn label(&self) -> &str {
        match self {
            Self::ToolCall { .. } => "tool_call",
            Self::Completeness { .. } => "completeness",
            Self::LoopDetection { .. } => "loop_detection",
            Self::InputSafety { .. } => "input_safety",
            Self::OutputSafety { .. } => "output_safety",
        }
    }
}

/// A single recorded decision from the overseer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEntry {
    /// When the decision was made.
    pub timestamp: DateTime<Utc>,
    /// Agent that triggered the judgment.
    pub agent_id: String,
    /// Type of judgment.
    pub judgment_type: JudgmentType,
    /// The verdict.
    pub verdict: OverseerVerdict,
    /// Raw reasoning text from the model (if available).
    pub reasoning: Option<String>,
    /// Inference stats for this decision.
    pub tokens_generated: usize,
    pub generation_time_ms: u64,
    pub tokens_per_second: f64,
}

/// Rolling log of overseer decisions.
pub struct DecisionLog {
    entries: Mutex<VecDeque<DecisionEntry>>,
}

impl DecisionLog {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(MAX_DECISION_LOG)),
        }
    }

    /// Record a new decision.
    pub fn record(&self, entry: DecisionEntry) {
        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() >= MAX_DECISION_LOG {
                entries.pop_front();
            }
            entries.push_back(entry);
        }
    }

    /// Get the most recent N decisions.
    pub fn recent(&self, n: usize) -> Vec<DecisionEntry> {
        if let Ok(entries) = self.entries.lock() {
            entries.iter().rev().take(n).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get all decisions for a specific agent.
    pub fn for_agent(&self, agent_id: &str) -> Vec<DecisionEntry> {
        if let Ok(entries) = self.entries.lock() {
            entries
                .iter()
                .filter(|e| e.agent_id == agent_id)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Total number of decisions recorded.
    pub fn total_count(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// Count decisions by verdict type.
    pub fn verdict_counts(&self) -> (usize, usize, usize, usize) {
        if let Ok(entries) = self.entries.lock() {
            let mut allow = 0;
            let mut note = 0;
            let mut caution = 0;
            let mut block = 0;
            for e in entries.iter() {
                match &e.verdict {
                    OverseerVerdict::Allow => allow += 1,
                    OverseerVerdict::AllowWithNote(_) => note += 1,
                    OverseerVerdict::Caution(_) => caution += 1,
                    OverseerVerdict::Block(_) => block += 1,
                }
            }
            (allow, note, caution, block)
        } else {
            (0, 0, 0, 0)
        }
    }
}

impl Default for DecisionLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Prompt templates for judgment types.
pub(crate) mod prompts {
    /// Build a tool-call judgment prompt.
    pub fn tool_call_prompt(tool_name: &str, params: &str, agent_id: &str) -> String {
        format!(
            "You are a security overseer. Evaluate this tool call for safety.\n\
             Agent: {agent_id}\n\
             Tool: {tool_name}\n\
             Parameters: {params}\n\n\
             Respond with exactly one of:\n\
             ALLOW - if the action is safe\n\
             CAUTION: <reason> - if the action is risky but acceptable\n\
             BLOCK: <reason> - if the action should be stopped\n\n\
             Be concise. One line only."
        )
    }

    /// Build a completeness judgment prompt.
    pub fn completeness_prompt(response: &str, agent_id: &str) -> String {
        format!(
            "You are an AI quality overseer. Evaluate this agent response for completeness.\n\
             Agent: {agent_id}\n\
             Response: {response}\n\n\
             Respond with exactly one of:\n\
             ALLOW - response is complete and coherent\n\
             CAUTION: <reason> - response seems incomplete or unclear\n\n\
             Be concise. One line only."
        )
    }

    /// Build a loop detection prompt.
    pub fn loop_detection_prompt(iteration: u32, pattern: &str, agent_id: &str) -> String {
        format!(
            "You are an AI overseer monitoring agent behavior.\n\
             Agent: {agent_id}\n\
             Current iteration: {iteration}\n\
             Observed pattern: {pattern}\n\n\
             Is this agent stuck in a loop? Respond with:\n\
             ALLOW - agent is making progress\n\
             CAUTION: <reason> - agent may be looping\n\
             BLOCK: <reason> - agent is definitely stuck\n\n\
             Be concise. One line only."
        )
    }

    /// Build a safety check prompt (for input or output).
    pub fn safety_prompt(content: &str, direction: &str) -> String {
        format!(
            "You are a content safety overseer. Evaluate this {direction} for safety concerns.\n\
             Content: {content}\n\n\
             Respond with exactly one of:\n\
             ALLOW - content is safe\n\
             CAUTION: <reason> - content has minor concerns\n\
             BLOCK: <reason> - content is unsafe\n\n\
             Be concise. One line only."
        )
    }
}

/// Parse model output into an OverseerVerdict.
pub fn parse_verdict(output: &str) -> OverseerVerdict {
    let trimmed = output.trim();

    // Try to find the verdict line (model might produce extra text)
    for line in trimmed.lines() {
        let line = line.trim();
        if line.starts_with("BLOCK:") || line.starts_with("BLOCK -") {
            let reason = line
                .trim_start_matches("BLOCK:")
                .trim_start_matches("BLOCK -")
                .trim();
            return OverseerVerdict::Block(reason.to_string());
        }
        if line.starts_with("CAUTION:") || line.starts_with("CAUTION -") {
            let reason = line
                .trim_start_matches("CAUTION:")
                .trim_start_matches("CAUTION -")
                .trim();
            return OverseerVerdict::Caution(reason.to_string());
        }
        if line == "ALLOW" || line.starts_with("ALLOW -") || line.starts_with("ALLOW:") {
            let note = line
                .trim_start_matches("ALLOW:")
                .trim_start_matches("ALLOW -")
                .trim_start_matches("ALLOW")
                .trim();
            if note.is_empty() {
                return OverseerVerdict::Allow;
            }
            return OverseerVerdict::AllowWithNote(note.to_string());
        }
    }

    // If we can't parse the output, default to allow with a note
    OverseerVerdict::AllowWithNote(format!("Unparseable verdict: {}", truncate(trimmed, 80)))
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
    fn test_parse_allow() {
        assert_eq!(parse_verdict("ALLOW"), OverseerVerdict::Allow);
        assert_eq!(parse_verdict("  ALLOW  \n"), OverseerVerdict::Allow);
    }

    #[test]
    fn test_parse_allow_with_note() {
        match parse_verdict("ALLOW: looks fine but monitor") {
            OverseerVerdict::AllowWithNote(note) => {
                assert_eq!(note, "looks fine but monitor");
            }
            other => panic!("Expected AllowWithNote, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_caution() {
        match parse_verdict("CAUTION: modifying system files") {
            OverseerVerdict::Caution(reason) => {
                assert_eq!(reason, "modifying system files");
            }
            other => panic!("Expected Caution, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_block() {
        match parse_verdict("BLOCK: attempting to delete root directory") {
            OverseerVerdict::Block(reason) => {
                assert_eq!(reason, "attempting to delete root directory");
            }
            other => panic!("Expected Block, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_multiline_extracts_verdict() {
        let output = "Let me think about this...\nALLOW\nThat seems fine.";
        assert_eq!(parse_verdict(output), OverseerVerdict::Allow);
    }

    #[test]
    fn test_parse_unparseable() {
        match parse_verdict("I'm not sure what to do") {
            OverseerVerdict::AllowWithNote(note) => {
                assert!(note.starts_with("Unparseable verdict:"));
            }
            other => panic!("Expected AllowWithNote fallback, got {other:?}"),
        }
    }

    #[test]
    fn test_truncate_respects_utf8_boundaries() {
        assert_eq!(truncate("éclair", 1), "é");
    }

    #[test]
    fn test_verdict_severity_ordering() {
        assert!(
            OverseerVerdict::Allow.severity()
                < OverseerVerdict::AllowWithNote("x".into()).severity()
        );
        assert!(
            OverseerVerdict::AllowWithNote("x".into()).severity()
                < OverseerVerdict::Caution("x".into()).severity()
        );
        assert!(
            OverseerVerdict::Caution("x".into()).severity()
                < OverseerVerdict::Block("x".into()).severity()
        );
    }

    #[test]
    fn test_decision_log_rolling() {
        let log = DecisionLog::new();
        for i in 0..150 {
            log.record(DecisionEntry {
                timestamp: Utc::now(),
                agent_id: format!("agent-{i}"),
                judgment_type: JudgmentType::InputSafety {
                    message_preview: "test".into(),
                },
                verdict: OverseerVerdict::Allow,
                reasoning: None,
                tokens_generated: 0,
                generation_time_ms: 0,
                tokens_per_second: 0.0,
            });
        }
        assert_eq!(log.total_count(), MAX_DECISION_LOG);
    }

    #[test]
    fn test_decision_log_recent() {
        let log = DecisionLog::new();
        for i in 0..5 {
            log.record(DecisionEntry {
                timestamp: Utc::now(),
                agent_id: format!("agent-{i}"),
                judgment_type: JudgmentType::InputSafety {
                    message_preview: "test".into(),
                },
                verdict: OverseerVerdict::Allow,
                reasoning: None,
                tokens_generated: 0,
                generation_time_ms: 0,
                tokens_per_second: 0.0,
            });
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].agent_id, "agent-4");
    }
}
