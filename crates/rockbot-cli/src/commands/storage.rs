use anyhow::Result;
use rockbot_storage_runtime::{StorageRuntime, StoreKind};
use std::process::Command;
use std::path::PathBuf;

use crate::{load_config, StorageCommands, StorageProbeStore};

pub async fn run(command: &StorageCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        StorageCommands::Plan { config } => {
            let config_path = config.as_ref().unwrap_or(config_path);
            let cfg = load_config(config_path).await?;
            let runtime = StorageRuntime::new(config_path, &cfg).await?;
            let plan = runtime.plan()?;

            println!("Storage root: {}", plan.storage_root.display());
            println!("Virtual disk: {}", plan.disk_path.display());
            println!();
            println!("Store plan:");
            for store in plan.stores {
                println!("- {}: {:?} ({})", store.label, store.resolution, store.descriptor);
            }

            Ok(())
        }
        StorageCommands::Repair { config } => {
            let config_path = config.as_ref().unwrap_or(config_path);
            let cfg = load_config(config_path).await?;
            let runtime = StorageRuntime::new(config_path, &cfg).await?;

            for kind in [
                StoreKind::Vault,
                StoreKind::Agents,
                StoreKind::Sessions,
                StoreKind::Cron,
                StoreKind::Routing,
                StoreKind::Topology,
            ] {
                let probe_ok = probe_store(config_path, kind).unwrap_or(false);
                let outcome = if probe_ok {
                    format!("{}: healthy, left unchanged", kind.label())
                } else {
                    let repaired = runtime.repair_store(kind, &cfg.credentials.vault_path)?;
                    format!("{}: {}", repaired.kind.label(), repaired.action)
                };
                println!("- {outcome}");
            }

            println!("Storage repair/import pass completed.");
            println!("Use `rockbot storage plan` to review the resolved store sources.");
            Ok(())
        }
        StorageCommands::Probe { config, store } => {
            let config_path = config.as_ref().unwrap_or(config_path);
            let cfg = load_config(config_path).await?;
            let runtime = StorageRuntime::new(config_path, &cfg).await?;
            match map_probe_store(store) {
                StoreKind::Vault => {
                    let _ = runtime.open_vault_volume_sync(&cfg.credentials.vault_path)?;
                }
                StoreKind::Agents => {
                    let _ = runtime.open_agents_store(&cfg.credentials.vault_path).await?;
                }
                StoreKind::Sessions => {
                    let _ = runtime.open_sessions_store().await?;
                }
                StoreKind::Cron => {
                    let _ = runtime.open_cron_store().await?;
                }
                StoreKind::Routing => {
                    let _ = runtime.open_routing_store().await?;
                }
                StoreKind::Topology => {
                    let _ = runtime.open_topology_store().await?;
                }
            }
            Ok(())
        }
    }
}

fn map_probe_store(store: &StorageProbeStore) -> StoreKind {
    match store {
        StorageProbeStore::Vault => StoreKind::Vault,
        StorageProbeStore::Agents => StoreKind::Agents,
        StorageProbeStore::Sessions => StoreKind::Sessions,
        StorageProbeStore::Cron => StoreKind::Cron,
        StorageProbeStore::Routing => StoreKind::Routing,
        StorageProbeStore::Topology => StoreKind::Topology,
    }
}

fn probe_store(config_path: &PathBuf, kind: StoreKind) -> Result<bool> {
    let exe = std::env::current_exe()?;
    let store = match kind {
        StoreKind::Vault => "vault",
        StoreKind::Agents => "agents",
        StoreKind::Sessions => "sessions",
        StoreKind::Cron => "cron",
        StoreKind::Routing => "routing",
        StoreKind::Topology => "topology",
    };
    let status = Command::new(exe)
        .arg("storage")
        .arg("probe")
        .arg("--config")
        .arg(config_path)
        .arg(store)
        .status()?;
    if let Some(code) = status.code() {
        return Ok(code == 0);
    }
    if status.success() {
        Ok(true)
    } else {
        Ok(false)
    }
}
