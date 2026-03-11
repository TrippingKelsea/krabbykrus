// Quick vault test - v2
fn main() {
    use std::path::PathBuf;
    
    let vault_path = PathBuf::from("/home/kelsea/.config/rockbot/vault");
    let keyfile_path = PathBuf::from("/home/kelsea/.config/rockbot/vault.key");
    
    println!("=== Vault Test ===");
    println!("Testing vault at: {:?}", vault_path);
    println!("Keyfile at: {:?}", keyfile_path);
    println!("Vault exists: {}", vault_path.join("meta.json").exists());
    println!("Keyfile exists: {}", keyfile_path.exists());
    
    // Try to open and unlock
    match rockbot_credentials::CredentialVault::open(&vault_path) {
        Ok(mut vault) => {
            println!("Vault opened successfully");
            println!("Unlock method: {:?}", vault.unlock_method());
            
            match vault.unlock_with_keyfile(&keyfile_path) {
                Ok(()) => {
                    println!("Vault unlocked successfully!");
                    let endpoints = vault.list_endpoints();
                    println!("Endpoints: {:?}", endpoints);
                }
                Err(e) => {
                    println!("Failed to unlock: {:?}", e);
                }
            }
        }
        Err(e) => {
            println!("Failed to open vault: {:?}", e);
        }
    }
    
    // Test Claude Code session key
    println!("\n=== Claude Code Session Key Test ===");
    if let Some(home) = dirs::home_dir() {
        let credentials_path = home.join(".claude").join(".credentials.json");
        println!("Credentials path: {:?}", credentials_path);
        
        if credentials_path.exists() {
            println!("✓ Credentials file exists");
            if let Ok(content) = std::fs::read_to_string(&credentials_path) {
                #[derive(serde::Deserialize)]
                struct ClaudeCredentials {
                    #[serde(rename = "claudeAiOauth")]
                    claude_ai_oauth: Option<OAuthCredentials>,
                }
                #[derive(serde::Deserialize)]
                struct OAuthCredentials {
                    #[serde(rename = "accessToken")]
                    access_token: String,
                    #[serde(rename = "expiresAt")]
                    expires_at: u64,
                }
                
                match serde_json::from_str::<ClaudeCredentials>(&content) {
                    Ok(creds) => {
                        if let Some(oauth) = creds.claude_ai_oauth {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            
                            println!("Access token prefix: {}...", &oauth.access_token[..30.min(oauth.access_token.len())]);
                            println!("Expires at: {} ms", oauth.expires_at);
                            println!("Current time: {} ms", now);
                            println!("Token valid for: {} hours", (oauth.expires_at - now) / 3600000);
                            
                            if oauth.expires_at > now + 300_000 {
                                println!("✓ Token is VALID");
                            } else {
                                println!("✗ Token is EXPIRED or expiring soon");
                            }
                        } else {
                            println!("✗ No OAuth credentials found in file");
                        }
                    }
                    Err(e) => {
                        println!("✗ Failed to parse credentials: {}", e);
                    }
                }
            } else {
                println!("✗ Failed to read credentials file");
            }
        } else {
            println!("✗ Credentials file NOT found");
        }
    }
}
