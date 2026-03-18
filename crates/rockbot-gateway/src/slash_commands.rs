//! Gateway slash command handlers.
//!
//! Intercepted by the gateway WS and HTTP handlers before agent processing.

use crate::gateway::Gateway;

impl Gateway {
    /// Dispatch a message to all slash-command handlers.
    pub async fn handle_slash_commands(&self, message: &str) -> Option<String> {
        let trimmed = message.trim();
        if trimmed == "/help" || trimmed.starts_with("/help ") {
            return Some(Self::handle_help_command(trimmed));
        }
        if let Some(out) = self.handle_gateway_command(trimmed).await {
            return Some(out);
        }
        if let Some(out) = self.handle_credentials_command(trimmed) {
            return Some(out);
        }
        if let Some(out) = self.handle_vault_command(trimmed) {
            return Some(out);
        }
        #[cfg(feature = "remote-exec")]
        if let Some(out) = self.handle_noise_command(trimmed).await {
            return Some(out);
        }
        #[cfg(not(feature = "remote-exec"))]
        if trimmed == "/noise" || trimmed.starts_with("/noise ") {
            return Some("Remote execution feature not enabled.".to_string());
        }
        #[cfg(feature = "overseer")]
        if trimmed == "/overseer" || trimmed.starts_with("/overseer ") {
            if let Some(out) = self.handle_overseer_command(trimmed).await {
                return Some(out);
            }
        }
        None
    }

    fn handle_help_command(trimmed: &str) -> String {
        let sub = trimmed.strip_prefix("/help").unwrap_or("").trim();
        if !sub.is_empty() {
            return format!("Unknown help topic: `{sub}`. Try `/help`.");
        }
        #[allow(unused_mut)]
        let mut out = String::from(
            "## RockBot Commands\n\n\
             | Command | Description |\n\
             |---------|-------------|\n\
             | `/help` | This help |\n\
             | `/gateway [status\\|agents\\|reload\\|help]` | Gateway management |\n\
             | `/credentials [list\\|help]` | Credential management |\n\
             | `/vault [status\\|help]` | Vault management |\n",
        );
        #[cfg(feature = "remote-exec")]
        out.push_str("| `/noise [status\\|help]` | Remote execution sessions |\n");
        #[cfg(feature = "overseer")]
        out.push_str("| `/overseer [status\\|init\\|help]` | Overseer management |\n");
        out
    }

    async fn handle_gateway_command(&self, trimmed: &str) -> Option<String> {
        if trimmed != "/gateway" && !trimmed.starts_with("/gateway ") {
            return None;
        }
        let sub = trimmed.strip_prefix("/gateway").unwrap_or("").trim();
        Some(match sub {
            "" | "status" => {
                let health = self.get_health_status().await;
                format!(
                    "## Gateway Status\n\n| Field | Value |\n|-------|-------|\n\
                     | Version | `{}` |\n| Uptime | {}s |\n\
                     | Connections | {} |\n| Sessions | {} |\n\
                     | Agents | {} |\n| Pending | {} |\n",
                    health.version, health.uptime_seconds, health.active_connections,
                    health.active_sessions, health.agents.len(), health.pending_agents,
                )
            }
            "reload" => {
                match self.reload_agents().await {
                    Ok((created, pending)) => {
                        format!(
                            "Config reloaded. {created} agent(s) created, {pending} still pending."
                        )
                    }
                    Err(e) => format!("Reload failed: {e}"),
                }
            }
            "agents" => {
                let agents = self.agents.read().await;
                if agents.is_empty() {
                    "No agents registered.".to_string()
                } else {
                    let mut out = String::from(
                        "## Agents\n\n| ID | Model | Msgs | Healthy |\n|----|-------|---------:|--------:|\n",
                    );
                    for (id, agent) in agents.iter() {
                        let (healthy, msgs) = match agent.health_check().await {
                            Ok(h) => (h.llm_healthy, h.stats.messages_processed),
                            Err(_) => (false, 0),
                        };
                        let model = agent.config.model.as_deref().unwrap_or("(default)");
                        out.push_str(&format!(
                            "| `{id}` | `{model}` | {msgs} | {} |\n",
                            if healthy { "yes" } else { "no" },
                        ));
                    }
                    out
                }
            }
            "help" => "## /gateway\n\n| Command | Description |\n|---------|-------------|\n\
                        | `status` | Gateway status |\n| `agents` | List agents |\n\
                        | `reload` | Reload config & retry pending agents |\n| `help` | This help |\n".to_string(),
            other => format!("Unknown: `{other}`. Try `/gateway help`."),
        })
    }

    fn handle_credentials_command(&self, trimmed: &str) -> Option<String> {
        if trimmed != "/credentials" && !trimmed.starts_with("/credentials ") {
            return None;
        }
        let sub = trimmed.strip_prefix("/credentials").unwrap_or("").trim();
        Some(match sub {
            "" | "list" => {
                if self.credential_manager.is_some() {
                    "Credential manager active. Use TUI or API to manage.".to_string()
                } else {
                    "Credential manager not initialized.".to_string()
                }
            }
            "help" => "## /credentials\n\n| Command | Description |\n|---------|-------------|\n\
                        | `list` | Show status |\n| `help` | This help |\n"
                .to_string(),
            other => format!("Unknown: `{other}`. Try `/credentials help`."),
        })
    }

    fn handle_vault_command(&self, trimmed: &str) -> Option<String> {
        if trimmed != "/vault" && !trimmed.starts_with("/vault ") {
            return None;
        }
        let sub = trimmed.strip_prefix("/vault").unwrap_or("").trim();
        Some(match sub {
            "" | "status" => {
                let path = self.credentials_config.vault_path.display();
                let enabled = self.credentials_config.enabled;
                let exists = rockbot_credentials::CredentialVault::exists(
                    &self.credentials_config.vault_path,
                );
                let state = if self.credential_manager.is_some() {
                    "unlocked"
                } else {
                    "not initialized"
                };
                format!(
                    "## Vault\n\n| Field | Value |\n|-------|-------|\n\
                     | Enabled | `{enabled}` |\n| Initialized | `{exists}` |\n\
                     | Path | `{path}` |\n| State | `{state}` |\n"
                )
            }
            "help" => "## /vault\n\n| Command | Description |\n|---------|-------------|\n\
                        | `status` | Vault status |\n| `help` | This help |\n"
                .to_string(),
            other => format!("Unknown: `{other}`. Try `/vault help`."),
        })
    }

    #[cfg(feature = "remote-exec")]
    async fn handle_noise_command(&self, trimmed: &str) -> Option<String> {
        if trimmed != "/noise" && !trimmed.starts_with("/noise ") {
            return None;
        }
        let sub = trimmed.strip_prefix("/noise").unwrap_or("").trim();
        Some(match sub {
            "" | "status" => {
                let executors = self.remote_exec_registry.list_executors().await;
                if executors.is_empty() {
                    "## Noise\n\nNo active sessions.".to_string()
                } else {
                    let mut out = String::from(
                        "## Noise Sessions\n\n| Conn | Type | Capabilities | Dir |\n|------|------|--------------|-----|\n",
                    );
                    for (_id, identity, caps) in &executors {
                        let cl: Vec<&str> = caps
                            .capabilities
                            .iter()
                            .map(|c| {
                                use rockbot_client::remote_exec::ToolCapability;
                                match c {
                                    ToolCapability::Filesystem => "fs",
                                    ToolCapability::Shell => "sh",
                                    ToolCapability::Network => "net",
                                    ToolCapability::Browser => "browser",
                                    ToolCapability::Agent => "agent",
                                    ToolCapability::Memory => "mem",
                                    ToolCapability::Full => "full",
                                }
                            })
                            .collect();
                        let display_id = identity
                            .client_uuid
                            .as_deref()
                            .unwrap_or(identity.conn_id.as_str());
                        let short = if display_id.len() > 8 {
                            &display_id[..8]
                        } else {
                            display_id
                        };
                        let wd = caps.working_dir.as_deref().unwrap_or("-");
                        out.push_str(&format!(
                            "| `{short}…` | `{}` | {} | `{wd}` |\n",
                            caps.client_type,
                            cl.join(", ")
                        ));
                    }
                    out
                }
            }
            "help" => "## /noise\n\n| Command | Description |\n|---------|-------------|\n\
                        | `status` | Active sessions |\n| `help` | This help |\n"
                .to_string(),
            other => format!("Unknown: `{other}`. Try `/noise help`."),
        })
    }

    #[cfg(feature = "overseer")]
    async fn handle_overseer_command(&self, trimmed: &str) -> Option<String> {
        let sub = trimmed.strip_prefix("/overseer").unwrap_or("").trim();
        match sub {
            "init" if self.overseer().is_none() => Some(
                "## Overseer Setup\n\n\
                     Add the following to your `rockbot.toml`:\n\n\
                     ```toml\n\
                     [overseer]\n\
                     enabled = true\n\
                     model_path = \"/path/to/model.gguf\"\n\
                     ```\n\n\
                     Then restart the gateway."
                    .to_string(),
            ),
            _ => None, // Let the overseer module handle its own commands
        }
    }
}
