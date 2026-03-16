// Quick vault test
fn main() {
    use std::path::PathBuf;

    let vault_path = PathBuf::from("/home/kelsea/.config/rockbot/vault");
    let keyfile_path = PathBuf::from("/home/kelsea/.config/rockbot/vault.key");

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
}
