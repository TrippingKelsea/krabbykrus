//! Test Anthropic API provider
//!
//! Run with: cargo run --example test_oauth --package rockbot-llm --features anthropic
//!
//! Requires the `anthropic` feature flag and either:
//! - ANTHROPIC_API_KEY environment variable, or
//! - Claude Code credentials at ~/.claude/.credentials.json

#[cfg(feature = "anthropic")]
use rockbot_llm::{
    anthropic::AnthropicProvider, ChatCompletionRequest, LlmProvider, Message, MessageRole,
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
    {
        _main().await
    }
}

#[cfg(feature = "anthropic")]
async fn _main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Checking Anthropic credentials...");

    if !AnthropicProvider::has_credentials() {
        eprintln!(
            "No credentials found. Set ANTHROPIC_API_KEY or install Claude Code credentials."
        );
        return Ok(());
    }

    if let Some(path) = AnthropicProvider::credentials_path() {
        println!("Credentials found at: {}", path.display());
    }

    println!(
        "Credentials valid: {}",
        AnthropicProvider::credentials_valid()
    );

    let provider = AnthropicProvider::new()?;

    println!("Provider ID: {}", provider.id());
    println!("Capabilities: {:?}", provider.capabilities());

    // Make a simple test request
    let request = ChatCompletionRequest {
        model: "claude-sonnet-4-20250514".to_string(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "Say 'Hello from rockbot!' and nothing else.".to_string(),
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
        }],
        tools: None,
        temperature: Some(0.0),
        max_tokens: Some(50),
        stream: false,
        response_format: None,
    };

    println!("\nSending request to model: {}", request.model);

    match provider.chat_completion(request).await {
        Ok(response) => {
            println!("\nSuccess!");
            println!("Response: {}", response.choices[0].message.content);
            println!(
                "Usage: {} input, {} output tokens",
                response.usage.prompt_tokens, response.usage.completion_tokens
            );
        }
        Err(e) => {
            eprintln!("\nAPI call failed: {e:?}");
            return Err(e.into());
        }
    }

    Ok(())
}
