use anyhow::{anyhow, Result};
use rockbot_config::{
    config::{StorageEncryptionMode, StorageKeySource},
    Config,
};
use rockbot_pki::PkiManager;
use rockbot_storage::Store;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Vault,
    Agents,
    Sessions,
    Cron,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    Persistent,
    Recovery,
}

#[derive(Clone)]
pub struct OpenedStore {
    pub store: Arc<Store>,
    pub descriptor: String,
    pub mode: StoreMode,
}

#[derive(Clone)]
pub struct StorageRuntime {
    config: Config,
    storage_root: PathBuf,
    disk_path: PathBuf,
    pki_manager: Option<Arc<PkiManager>>,
}

impl StorageRuntime {
    pub async fn new(config_path: &Path, config: &Config) -> Result<Self> {
        let storage_root = storage_root_dir(config_path, config);
        Self::new_with_root(config, storage_root).await
    }

    pub async fn new_with_root(config: &Config, storage_root: PathBuf) -> Result<Self> {
        Self::new_with_root_sync(config, storage_root)
    }

    pub fn new_with_root_sync(config: &Config, storage_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&storage_root)?;
        let disk_path = Store::default_disk_path(&storage_root);
        let pki_manager = open_pki_for_storage(config)?.map(Arc::new);
        Ok(Self {
            config: config.clone(),
            storage_root,
            disk_path,
            pki_manager,
        })
    }

    pub fn storage_root(&self) -> &Path {
        &self.storage_root
    }

    pub fn disk_path(&self) -> &Path {
        &self.disk_path
    }

    pub fn pki_manager(&self) -> Option<&PkiManager> {
        self.pki_manager.as_deref()
    }

    pub fn key_for_label(&self, label: &str) -> Result<Option<[u8; 32]>> {
        storage_key_for_label(&self.config, self.pki_manager(), label)
    }

    pub async fn open_sessions_store(&self) -> Result<OpenedStore> {
        let key = self.key_for_label("sessions")?;
        let legacy_path = self.storage_root.join("data").join("sessions.redb");
        if legacy_path.exists() {
            match Store::open(&legacy_path) {
                Ok(store) => {
                    return Ok(OpenedStore {
                        store: Arc::new(store),
                        descriptor: format!("legacy store {}", legacy_path.display()),
                        mode: StoreMode::Persistent,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        "Could not open legacy session store {}: {err}. Falling back to recovery session store instead of touching the suspect virtual-disk volume.",
                        legacy_path.display()
                    );
                    return self.open_recovery_store("sessions.recovery.redb", key).await;
                }
            }
        }

        match Store::open_volume(&self.disk_path, "sessions", 512 * 1024 * 1024, key) {
            Ok(store) => Ok(OpenedStore {
                store: Arc::new(store),
                descriptor: encryption_mode_log(
                    key.is_some(),
                    &format!("virtual disk {} volume 'sessions'", self.disk_path.display()),
                ),
                mode: StoreMode::Persistent,
            }),
            Err(err) => {
                tracing::warn!(
                    "Could not open virtual-disk sessions volume: {err}. Falling back to recovery session store."
                );
                self.open_recovery_store("sessions.recovery.redb", key).await
            }
        }
    }

    pub async fn open_agents_store(&self, vault_path: &Path) -> Result<OpenedStore> {
        let key = self.key_for_label("agents")?;
        let legacy_path = vault_path.join("agents.redb");
        if legacy_path.exists() {
            match Store::open(&legacy_path) {
                Ok(store) => {
                    return Ok(OpenedStore {
                        store: Arc::new(store),
                        descriptor: format!("legacy store {}", legacy_path.display()),
                        mode: StoreMode::Persistent,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        "Could not open legacy agent store {}: {err}. Attempting virtual-disk recovery.",
                        legacy_path.display()
                    );
                }
            }
        }

        let store = Store::open_volume(&self.disk_path, "agents", 128 * 1024 * 1024, key)?;
        Ok(OpenedStore {
            store: Arc::new(store),
            descriptor: encryption_mode_log(
                key.is_some(),
                &format!("virtual disk {} volume 'agents'", self.disk_path.display()),
            ),
            mode: StoreMode::Persistent,
        })
    }

    pub async fn open_cron_store(&self) -> Result<OpenedStore> {
        let key = self.key_for_label("cron")?;
        let legacy_path = self.storage_root.join("data").join("cron.redb");
        if legacy_path.exists() {
            match Store::open(&legacy_path) {
                Ok(store) => {
                    return Ok(OpenedStore {
                        store: Arc::new(store),
                        descriptor: format!("legacy store {}", legacy_path.display()),
                        mode: StoreMode::Persistent,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        "Could not open legacy cron store {}: {err}. Falling back to virtual-disk cron volume.",
                        legacy_path.display()
                    );
                }
            }
        }

        let store = Store::open_volume(&self.disk_path, "cron", 128 * 1024 * 1024, key)?;
        Ok(OpenedStore {
            store: Arc::new(store),
            descriptor: encryption_mode_log(
                key.is_some(),
                &format!("virtual disk {} volume 'cron'", self.disk_path.display()),
            ),
            mode: StoreMode::Persistent,
        })
    }

    pub async fn open_vault_volume(&self, data_dir: &Path) -> Result<OpenedStore> {
        self.open_vault_volume_sync(data_dir)
    }

    pub fn open_vault_volume_sync(&self, data_dir: &Path) -> Result<OpenedStore> {
        let store = self.open_vault_store_sync(data_dir)?;
        Ok(OpenedStore {
            store: Arc::new(store),
            descriptor: format!(
                "virtual disk {} volume 'vault' (plaintext)",
                self.disk_path.display()
            ),
            mode: StoreMode::Persistent,
        })
    }

    pub fn open_vault_store_sync(&self, data_dir: &Path) -> Result<Store> {
        let legacy_path = data_dir.join("vault.db");
        if legacy_path.exists() {
            let should_import = match rockbot_vdisk::volume_info(&self.disk_path, "vault")? {
                Some(info) => {
                    if info.len != std::fs::metadata(&legacy_path)?.len() {
                        true
                    } else {
                        let prefix = rockbot_vdisk::read_volume_prefix(&self.disk_path, "vault", 4)?;
                        prefix.as_deref() != Some(b"redb".as_slice())
                    }
                }
                None => true,
            };

            if should_import {
                tracing::info!(
                    "Importing legacy {} into vault volume",
                    legacy_path.display()
                );
                rockbot_vdisk::replace_file(&self.disk_path, "vault", &legacy_path, None)?;
            }
        }

        Store::open_volume(&self.disk_path, "vault", 256 * 1024 * 1024, None)
    }

    pub async fn open_agents_watch_store(&self, vault_path: &Path) -> Result<OpenedStore> {
        self.open_agents_store(vault_path).await
    }

    async fn open_recovery_store(
        &self,
        file_name: &str,
        key: Option<[u8; 32]>,
    ) -> Result<OpenedStore> {
        let recovery_path = self.storage_root.join("runtime").join(file_name);
        if let Some(parent) = recovery_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let store = Store::open_with_optional_key(&recovery_path, key)?;
        Ok(OpenedStore {
            store: Arc::new(store),
            descriptor: format!("recovery store {}", recovery_path.display()),
            mode: StoreMode::Recovery,
        })
    }
}

pub fn default_pki_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
        .join("pki")
}

pub fn storage_root_dir(config_path: &Path, config: &Config) -> PathBuf {
    if let Some(parent) = config_path.parent() {
        return parent.to_path_buf();
    }
    if let Some(parent) = config.credentials.vault_path.parent() {
        return parent.to_path_buf();
    }
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
}

pub fn open_pki_for_storage(config: &Config) -> Result<Option<PkiManager>> {
    if !config.security.storage.enabled {
        return Ok(None);
    }
    if matches!(config.security.storage.mode, StorageEncryptionMode::Disabled) {
        return Ok(None);
    }
    if !matches!(config.security.storage.key_source, StorageKeySource::PkiLocal) {
        return Ok(None);
    }

    let pki_dir = config.effective_pki().pki_dir.unwrap_or_else(default_pki_dir);
    let manager = PkiManager::new(pki_dir).map_err(|e| {
        anyhow!(
            "Encrypted storage is enabled, but the PKI manager could not be opened for storage keys: {e}"
        )
    })?;
    Ok(Some(manager))
}

pub fn storage_key_for_label(
    config: &Config,
    pki_manager: Option<&PkiManager>,
    label: &str,
) -> Result<Option<[u8; 32]>> {
    if !config.security.storage.enabled {
        return Ok(None);
    }
    if matches!(config.security.storage.mode, StorageEncryptionMode::Disabled) {
        return Ok(None);
    }

    match config.security.storage.key_source {
        StorageKeySource::PkiLocal => match pki_manager {
            Some(mgr) => Ok(Some(mgr.ensure_local_storage_key(label).map_err(|e| {
                anyhow!(
                    "Encrypted storage is enabled, but the storage key for '{label}' could not be created or loaded: {e}"
                )
            })?)),
            None => Err(anyhow!(
                "Encrypted storage is enabled, but no PKI manager is available for storage key '{label}'"
            )),
        },
        StorageKeySource::DataLocal | StorageKeySource::External => Ok(None),
    }
}

pub fn encryption_mode_log(encrypted: bool, base: &str) -> String {
    if encrypted {
        format!("{base} (encrypted)")
    } else {
        format!("{base} (plaintext)")
    }
}
