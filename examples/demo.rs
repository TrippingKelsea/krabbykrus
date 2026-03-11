//! Demo: Minimal RockBot usage example
//!
//! This example shows how to create an agent and process a message.
//!
//! Run with:
//!   ANTHROPIC_API_KEY=your_key cargo run --example demo

use anyhow::Result;
use rockbot_core::config::AgentInstance;
use rockbot_core::message::{Message, MessageRole};
use rockbot_core::session::SessionManager;
use rockbot_core::Agent;
use rockbot_llm::LlmProviderRegistry;
use rockbot_memory::MemoryManager;
use rockbot_security::{SecurityConfig, SecurityManager, SandboxConfig, CapabilityConfig};
use rockbot_tools::{ToolConfig, ToolRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("rockbot=debug,info")
        .init();

    println!("🦀 RockBot Demo\n");

    // Create temporary directories for this demo
    let temp_dir = TempDir::new()?;
    let workspace_path = temp_dir.path().join("workspace");
    let db_path = temp_dir.path().join("sessions.db");

    // Initialize components
    println!("📦 Initializing components...");

    // Session manager
    let session_manager = Arc::new(SessionManager::new(&db_path, 100).await?);
    println!("  ✓ Session manager");

    // Tool registry (minimal profile for demo)
    let tool_config = ToolConfig {
        profile: "minimal".to_string(),
        deny: vec![],
        configs: HashMap::new(),
    };
    let tool_registry = Arc::new(ToolRegistry::new(tool_config).await?);
    println!("  ✓ Tool registry");

    // Security manager
    let security_config = SecurityConfig {
        sandbox: SandboxConfig {
            mode: "disabled".to_string(),
            scope: "session".to_string(),
            image: None,
        },
        capabilities: CapabilityConfig::default(),
    };
    let security_manager = Arc::new(SecurityManager::new(security_config).await?);
    println!("  ✓ Security manager");

    // Memory manager
    let memory_manager = Arc::new(MemoryManager::new(workspace_path.clone()).await?);
    println!("  ✓ Memory manager");

    // LLM provider registry
    let llm_registry = LlmProviderRegistry::new().await?;
    println!("  ✓ LLM provider registry");

    // Check for Anthropic API key
    let model = if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  ✓ Anthropic provider (API key found)");
        "anthropic/claude-sonnet-4-20250514"
    } else {
        println!("  ⚠ Using mock provider (set ANTHROPIC_API_KEY for real responses)");
        "mock-model"
    };

    // Get LLM provider
    let llm_provider = llm_registry.get_provider_for_model(model).await?;

    // Create agent configuration
    let agent_config = AgentInstance {
        id: "demo-agent".to_string(),
        workspace: Some(workspace_path),
        model: Some(model.to_string()),
        max_tool_calls: None,
        parent_id: None,
        system_prompt: None,
        enabled: true,
        config: HashMap::new(),
    };

    // Create agent
    let agent = Agent::new(
        agent_config,
        llm_provider,
        tool_registry,
        memory_manager,
        security_manager,
        session_manager,
        None, // No credential accessor for demo
    )
    .await?;
    println!("  ✓ Agent created\n");

    // Create a test message
    let user_message = Message::text("Hello! What's 2 + 2? Please answer briefly.")
        .with_role(MessageRole::User);

    println!("💬 Sending message: \"{}\"", user_message.extract_text().unwrap_or_default());
    println!("   (This may take a moment...)\n");

    // Process the message
    let session_id = "demo-session".to_string();
    match agent.process_message(session_id, user_message).await {
        Ok(response) => {
            println!("✅ Response received!");
            println!("   Message: {}", response.message.extract_text().unwrap_or_default());
            println!("   Tokens used: {} (prompt: {}, completion: {})",
                response.tokens_used.total_tokens,
                response.tokens_used.prompt_tokens,
                response.tokens_used.completion_tokens
            );
            println!("   Processing time: {}ms", response.processing_time_ms);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }

    // Get agent stats
    let stats = agent.get_stats().await;
    println!("\n📊 Agent Stats:");
    println!("   Messages processed: {}", stats.messages_processed);
    println!("   Total tokens: {}", stats.total_tokens);

    println!("\n✨ Demo complete!");
    Ok(())
}
