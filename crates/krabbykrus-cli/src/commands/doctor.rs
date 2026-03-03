//! Health and diagnostics commands

use anyhow::Result;
use std::path::PathBuf;
use crate::load_config;

/// Run health diagnostics
pub async fn run(config_path: &PathBuf) -> Result<()> {
    println!("🏥 Krabbykrus Health Check");
    println!("========================");
    
    // Check configuration
    print!("Configuration: ");
    let config = match load_config(config_path).await {
        Ok(c) => {
            println!("✅ Valid");
            c
        }
        Err(e) => {
            println!("❌ Invalid - {}", e);
            return Ok(());
        }
    };
    
    // Check workspace directories
    print!("Workspace access: ");
    let workspace_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("krabbykrus");
    
    if tokio::fs::metadata(&workspace_dir).await.is_ok() {
        println!("✅ Accessible");
    } else {
        println!("⚠️  Directory doesn't exist, will be created");
    }
    
    // Check database path
    print!("Database directory: ");
    let db_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("krabbykrus")
        .join("data");
    
    if tokio::fs::metadata(&db_dir).await.is_ok() {
        println!("✅ Accessible");
    } else {
        println!("⚠️  Directory doesn't exist, will be created");
    }
    
    // Check gateway auth configuration
    print!("Gateway Auth: ");
    if config.gateway.is_localhost() {
        println!("✅ Localhost (no auth required)");
    } else if config.gateway.requires_api_key() {
        println!("🔐 API key required (non-localhost bind)");
        println!("     Create one with: krabbykrus credentials api-key create");
    } else {
        println!("⚠️  Warning: Non-localhost bind without auth");
    }
    
    println!("\n🎯 System Status: Ready to start");
    println!("   Run 'krabbykrus gateway' to start the server");
    
    Ok(())
}