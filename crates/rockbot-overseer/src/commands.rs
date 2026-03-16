//! Overseer slash command handlers.
//!
//! These commands are intercepted by the gateway before agent processing.
//! They return formatted text directly to the user without LLM involvement.

use crate::Overseer;

/// Result of a slash command — either rendered output or "not my command".
pub enum CommandResult {
    /// Command was handled; return this text to the user.
    Handled(String),
    /// Not an overseer command — pass through to normal processing.
    NotHandled,
}

impl Overseer {
    /// Dispatch an `/overseer <subcommand>` message.
    ///
    /// Returns `CommandResult::Handled(output)` if the message is an overseer
    /// command, or `CommandResult::NotHandled` if it should be passed through.
    pub fn dispatch_command(&self, message: &str) -> CommandResult {
        let trimmed = message.trim();
        if !trimmed.starts_with("/overseer") {
            return CommandResult::NotHandled;
        }

        let rest = trimmed.trim_start_matches("/overseer").trim();
        let (subcmd, args) = match rest.find(char::is_whitespace) {
            Some(pos) => (&rest[..pos], rest[pos..].trim()),
            None => (rest, ""),
        };

        match subcmd {
            "status" => CommandResult::Handled(self.cmd_status()),
            "explain" => CommandResult::Handled(self.cmd_explain()),
            "history" => CommandResult::Handled(self.cmd_history(args)),
            "trust" => CommandResult::Handled(self.cmd_trust(args)),
            "config" => CommandResult::Handled(self.cmd_config()),
            "help" | "" => CommandResult::Handled(self.cmd_help()),
            other => CommandResult::Handled(format!(
                "Unknown overseer command: `{other}`\n\n{}",
                self.cmd_help()
            )),
        }
    }

    /// `/overseer status` — operational status, model info, license.
    fn cmd_status(&self) -> String {
        let uptime = self.uptime();
        let uptime_str = format_duration(uptime);

        let (total_calls, total_tokens, total_prompt_tokens, total_time_ms) =
            self.inference_cumulative_stats();

        let avg_tok_s = if total_time_ms > 0 {
            total_tokens as f64 / (total_time_ms as f64 / 1000.0)
        } else {
            0.0
        };

        let (allow, note, caution, block) = self.decision_log.verdict_counts();
        let total_decisions = allow + note + caution + block;

        let mode_str = if self.config.enforce {
            "enforcing"
        } else {
            "advisory"
        };

        format!(
            "## Overseer Status\n\n\
             | Field | Value |\n\
             |-------|-------|\n\
             | Status | **active** ({mode_str} mode) |\n\
             | Uptime | {uptime_str} |\n\
             | Model | `{}` |\n\
             | Model file | `{}` |\n\
             | Max tokens | {} |\n\
             | Temperature | {} |\n\
             | License | {} |\n\n\
             ### Inference Stats\n\n\
             | Metric | Value |\n\
             |--------|-------|\n\
             | Total judgments | {total_calls} |\n\
             | Tokens generated | {total_tokens} |\n\
             | Prompt tokens consumed | {total_prompt_tokens} |\n\
             | Avg throughput | {avg_tok_s:.1} tok/s |\n\n\
             ### Decision Summary\n\n\
             | Verdict | Count |\n\
             |---------|-------|\n\
             | Allow | {allow} |\n\
             | Allow (with note) | {note} |\n\
             | Caution | {caution} |\n\
             | Block | {block} |\n\
             | **Total** | **{total_decisions}** |",
            self.config.model_id,
            self.config.model_path.display(),
            self.config.max_tokens,
            self.config.temperature,
            self.config.license.as_deref().unwrap_or("See model card"),
        )
    }

    /// `/overseer explain` — chain of thought, last decisions, reasoning.
    fn cmd_explain(&self) -> String {
        let recent = self.decision_log.recent(10);

        if recent.is_empty() {
            return "## Overseer Explain\n\n\
                    No decisions have been made yet. The overseer evaluates agent actions \
                    (tool calls, output completeness, loop detection) using an embedded local \
                    model and produces advisory verdicts.\n\n\
                    ### Decision Framework\n\n\
                    1. **Tool calls**: Before execution, the overseer evaluates the tool name \
                       and parameters for safety (file paths, commands, network access)\n\
                    2. **Completeness**: After the agent produces output, the overseer checks \
                       whether the response addresses the user's request\n\
                    3. **Loop detection**: During multi-iteration tool loops, the overseer \
                       monitors for repetitive patterns\n\
                    4. **Content safety**: Input and output are scanned for PII, injection \
                       attempts, and harmful content"
                .to_string();
        }

        let mut output = String::from("## Overseer Explain\n\n");
        output.push_str("### Decision Framework\n\n");
        output.push_str(
            "The overseer evaluates agent behavior using an embedded local model. \
             Each judgment follows a structured prompt that asks the model to classify \
             the action as ALLOW, CAUTION, or BLOCK with reasoning.\n\n",
        );

        // Consolidated stats for recent decisions
        let total_time: u64 = recent.iter().map(|d| d.generation_time_ms).sum();
        let total_tok: usize = recent.iter().map(|d| d.tokens_generated).sum();
        let avg_tok_s = if total_time > 0 {
            total_tok as f64 / (total_time as f64 / 1000.0)
        } else {
            0.0
        };

        output.push_str("### Performance (last 10 decisions)\n\n");
        output.push_str(&format!(
            "| Metric | Value |\n\
             |--------|-------|\n\
             | Avg inference time | {:.0}ms |\n\
             | Avg throughput | {avg_tok_s:.1} tok/s |\n\
             | Total tokens | {total_tok} |\n\n",
            if recent.is_empty() {
                0.0
            } else {
                total_time as f64 / recent.len() as f64
            },
        ));

        output.push_str("### Recent Decisions\n\n");

        for entry in &recent {
            let ts = entry.timestamp.format("%H:%M:%S");
            let verdict_icon = match &entry.verdict {
                crate::judgments::OverseerVerdict::Allow => "[ok]",
                crate::judgments::OverseerVerdict::AllowWithNote(_) => "[note]",
                crate::judgments::OverseerVerdict::Caution(_) => "[!]",
                crate::judgments::OverseerVerdict::Block(_) => "[X]",
            };

            output.push_str(&format!(
                "**{ts}** {verdict_icon} `{}` ({}) agent=`{}`",
                entry.verdict.label(),
                entry.judgment_type.label(),
                entry.agent_id,
            ));

            if let Some(msg) = entry.verdict.message() {
                output.push_str(&format!("\n  > {msg}"));
            }
            if let Some(reasoning) = &entry.reasoning {
                let preview = if reasoning.len() > 120 {
                    &reasoning[..120]
                } else {
                    reasoning.as_str()
                };
                output.push_str(&format!("\n  Reasoning: _{preview}_"));
            }
            output.push_str(&format!(
                "\n  ({} tok, {}ms, {:.1} tok/s)\n\n",
                entry.tokens_generated, entry.generation_time_ms, entry.tokens_per_second,
            ));
        }

        output
    }

    /// `/overseer history [agent_id]` — full decision history, optionally filtered.
    fn cmd_history(&self, args: &str) -> String {
        let entries = if args.is_empty() {
            self.decision_log.recent(50)
        } else {
            self.decision_log.for_agent(args)
        };

        if entries.is_empty() {
            return if args.is_empty() {
                "No overseer decisions recorded yet.".to_string()
            } else {
                format!("No overseer decisions for agent `{args}`.")
            };
        }

        let mut output = if args.is_empty() {
            format!("## Overseer History (last {} decisions)\n\n", entries.len())
        } else {
            format!(
                "## Overseer History for `{args}` ({} decisions)\n\n",
                entries.len()
            )
        };

        output.push_str("| Time | Agent | Type | Verdict | Note |\n");
        output.push_str("|------|-------|------|---------|------|\n");

        for entry in &entries {
            let ts = entry.timestamp.format("%H:%M:%S");
            let note = entry
                .verdict
                .message()
                .map(|m| truncate_str(m, 40))
                .unwrap_or_default();
            output.push_str(&format!(
                "| {ts} | {} | {} | {} | {note} |\n",
                entry.agent_id,
                entry.judgment_type.label(),
                entry.verdict.label(),
            ));
        }

        output
    }

    /// `/overseer trust [agent_id]` — trust score / behavioral summary for an agent.
    fn cmd_trust(&self, args: &str) -> String {
        if args.is_empty() {
            return "Usage: `/overseer trust <agent_id>`\n\n\
                    Shows the behavioral trust profile for a specific agent."
                .to_string();
        }

        let entries = self.decision_log.for_agent(args);
        if entries.is_empty() {
            return format!("No overseer data for agent `{args}` yet.");
        }

        let total = entries.len();
        let mut allow = 0;
        let mut note = 0;
        let mut caution = 0;
        let mut block = 0;

        for e in &entries {
            match &e.verdict {
                crate::judgments::OverseerVerdict::Allow => allow += 1,
                crate::judgments::OverseerVerdict::AllowWithNote(_) => note += 1,
                crate::judgments::OverseerVerdict::Caution(_) => caution += 1,
                crate::judgments::OverseerVerdict::Block(_) => block += 1,
            }
        }

        let trust_score = if total > 0 {
            let weighted =
                allow as f64 * 1.0 + note as f64 * 0.9 + caution as f64 * 0.4 + block as f64 * 0.0;
            (weighted / total as f64 * 100.0).round() as u32
        } else {
            100
        };

        let trust_label = match trust_score {
            90..=100 => "High",
            70..=89 => "Moderate",
            50..=69 => "Low",
            _ => "Very Low",
        };

        format!(
            "## Trust Profile: `{args}`\n\n\
             | Metric | Value |\n\
             |--------|-------|\n\
             | Trust score | **{trust_score}%** ({trust_label}) |\n\
             | Total evaluations | {total} |\n\
             | Allow | {allow} |\n\
             | Allow (with note) | {note} |\n\
             | Caution | {caution} |\n\
             | Block | {block} |"
        )
    }

    /// `/overseer config` — current overseer configuration.
    fn cmd_config(&self) -> String {
        let mode_str = if self.config.enforce {
            "enforcing (verdicts block agent actions)"
        } else {
            "advisory (verdicts logged but not enforced)"
        };

        format!(
            "## Overseer Configuration\n\n\
             | Setting | Value |\n\
             |---------|-------|\n\
             | Mode | {mode_str} |\n\
             | Model ID | `{}` |\n\
             | Model path | `{}` |\n\
             | Max tokens | {} |\n\
             | Temperature | {} |\n\
             | Top-p | {} |\n\
             | Repeat penalty | {} |\n\
             | Seed | {} |\n\
             | License | {} |",
            self.config.model_id,
            self.config.model_path.display(),
            self.config.max_tokens,
            self.config.temperature,
            self.config.top_p,
            self.config.repeat_penalty,
            self.config.seed,
            self.config.license.as_deref().unwrap_or("See model card"),
        )
    }

    /// `/overseer help` — list available commands.
    fn cmd_help(&self) -> String {
        "## Overseer Commands\n\n\
         | Command | Description |\n\
         |---------|-------------|\n\
         | `/overseer status` | Operational status, model version, inference stats |\n\
         | `/overseer explain` | Decision framework, reasoning behind recent verdicts |\n\
         | `/overseer history [agent]` | Decision log, optionally filtered by agent |\n\
         | `/overseer trust <agent>` | Behavioral trust profile for an agent |\n\
         | `/overseer config` | Current overseer configuration |\n\
         | `/overseer help` | This help message |"
            .to_string()
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
