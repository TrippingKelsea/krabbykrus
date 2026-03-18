use anyhow::Result;
use rockbot_storage_runtime::StorageRuntime;
use std::path::PathBuf;

use crate::{load_config, StorageCommands};

pub async fn run(command: &StorageCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        StorageCommands::Plan { config } => {
            let config_path = config.as_ref().unwrap_or(config_path);
            let cfg = load_config(config_path).await?;
            let runtime = StorageRuntime::new(config_path, &cfg).await?;

            println!("Storage root: {}", runtime.storage_root().display());
            println!("Virtual disk: {}", runtime.disk_path().display());

            match runtime.open_sessions_store().await {
                Ok(store) => println!("sessions: {}", store.descriptor),
                Err(err) => println!("sessions: unavailable ({err})"),
            }
            match runtime.open_cron_store().await {
                Ok(store) => println!("cron: {}", store.descriptor),
                Err(err) => println!("cron: unavailable ({err})"),
            }
            match runtime.open_vault_volume_sync(&cfg.credentials.vault_path) {
                Ok(store) => println!("vault: {}", store.descriptor),
                Err(err) => println!("vault: unavailable ({err})"),
            }
            match runtime.open_agents_store(&cfg.credentials.vault_path).await {
                Ok(store) => println!("agents: {}", store.descriptor),
                Err(err) => println!("agents: unavailable ({err})"),
            }

            Ok(())
        }
        StorageCommands::Repair { config } => {
            let config_path = config.as_ref().unwrap_or(config_path);
            let cfg = load_config(config_path).await?;
            let runtime = StorageRuntime::new(config_path, &cfg).await?;

            let _ = runtime.open_vault_volume_sync(&cfg.credentials.vault_path)?;
            let _ = runtime.open_sessions_store().await?;
            let _ = runtime.open_cron_store().await?;
            let _ = runtime.open_agents_store(&cfg.credentials.vault_path).await?;

            println!("Storage repair/import pass completed.");
            println!("Use `rockbot storage plan` to review the resolved store sources.");
            Ok(())
        }
    }
}
