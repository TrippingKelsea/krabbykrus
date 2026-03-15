//! Re-export from rockbot-client.
pub use rockbot_client::acp::*;

/// Run the ACP stdio server loop (re-exported with backward-compatible return type).
pub async fn run_acp_server(agent_ids: Vec<String>) -> crate::error::Result<()> {
    rockbot_client::acp::run_acp_server(agent_ids).await?;
    Ok(())
}
