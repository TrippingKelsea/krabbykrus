//! Test OAuth authentication with Anthropic API
//!
//! Run with: cargo run --example test_session_key --package rockbot-llm --features anthropic
//!
//! This example demonstrates both OAuth mode (using Claude Code credentials)
//! and runtime mode switching between API key and OAuth modes.
//!
//! Requires the `anthropic` feature flag.

#[cfg(feature = "anthropic")]
use rockbot_llm::{
    anthropic::{AnthropicAuth, AnthropicAuthMode, AnthropicProvider},
    ChatCompletionRequest, LlmProvider, Message, MessageRole,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(feature = "anthropic"))]
    {
        eprintln!("This example requires the 'anthropic' feature. Run with:");
        eprintln!("  cargo run --example test_oauth --package rockbot-llm --features anthropic");
        return Ok(());
    }
    #[cfg(feature = "anthropic")]
    { _main().await }
}

#[cfg(feature = "anthropic")]
async fn _main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Loading Claude Code credentials...");
    
    // Try to load OAuth credentials from Claude Code
    let auth = match AnthropicAuth::from_claude_credentials() {
        Ok(auth) => {
            println!("✅ Loaded credentials from ~/.claude/.credentials.json");
            if auth.is_expired() {
                println!("⚠️  Warning: Token appears to be expired!");
            } else {
                println!("✅ Token is not expired");
            }
            auth
        }
        Err(e) => {
            eprintln!("❌ Failed to load credentials: {:?}", e);
            return Err(e.into());
        }
    };
    
    // Create provider with OAuth credentials
    let provider = AnthropicProvider::with_oauth(auth);
    
    // Show current mode
    println!("Current mode: {:?}", provider.current_mode().await);
    
    println!("\nTesting API call...");
    println!("Provider ID: {}", provider.id());
    println!("Capabilities: {:?}", provider.capabilities());
    
    // Make a simple test request
    let request = ChatCompletionRequest {
        model: "claude-sonnet-4-20250514".to_string(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "Say 'Hello from rockbot!' and nothing else.".to_string(),
            tool_calls: None,
        }],
        tools: None,
        temperature: Some(0.0),
        max_tokens: Some(50),
        stream: false,
    };
    
    println!("\nSending request to model: {}", request.model);
    
    match provider.chat_completion(request).await {
        Ok(response) => {
            println!("\n✅ Success!");
            println!("Response: {}", response.choices[0].message.content);
            println!("Usage: {} input, {} output tokens", 
                response.usage.prompt_tokens, 
                response.usage.completion_tokens);
        }
        Err(e) => {
            eprintln!("\n❌ API call failed: {:?}", e);
            return Err(e.into());
        }
    }
    
    // Demonstrate mode switching (if API key is available)
    println!("\n--- Mode Switching Demo ---");
    if let Ok(_) = std::env::var("ANTHROPIC_API_KEY") {
        println!("ANTHROPIC_API_KEY found, demonstrating mode switch...");
        
        // Switch to API mode
        if let Err(e) = provider.set_mode(AnthropicAuthMode::Api).await {
            println!("❌ Failed to switch to API mode: {:?}", e);
        } else {
            println!("✅ Switched to API mode: {:?}", provider.current_mode().await);
        }
        
        // Switch back to OAuth
        if let Err(e) = provider.set_mode(AnthropicAuthMode::OAuth).await {
            println!("❌ Failed to switch back to OAuth mode: {:?}", e);
        } else {
            println!("✅ Switched back to OAuth mode: {:?}", provider.current_mode().await);
        }
    } else {
        println!("No ANTHROPIC_API_KEY set, skipping mode switch demo");
    }
    
    Ok(())
}
