use anyhow::{anyhow, Result};
use rockbot_config::{
    config::{StorageEncryptionMode, StorageKeySource},
    Config,
};
use rockbot_pki::PkiManager;
use rockbot_storage::Store;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const WELL_KNOWN_AGENT_CONTEXT_FILES: &[&str] =
    &["SOUL.md", "SYSTEM-PROMPT.md", "AGENTS.md", "MEMORY.md"];

#[derive(Debug, Clone)]
pub struct AgentContextFileInfo {
    pub name: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub well_known: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Vault,
    Agents,
    Sessions,
    Cron,
    Routing,
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

#[derive(Debug, Clone)]
pub struct LegacyFileState {
    pub path: PathBuf,
    pub exists: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct VolumeState {
    pub name: String,
    pub exists: bool,
    pub len_bytes: Option<u64>,
    pub capacity_bytes: Option<u64>,
    pub header_kind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    Legacy,
    VirtualDisk,
    Recovery,
    Missing,
}

#[derive(Debug, Clone)]
pub struct StorePlan {
    pub kind: StoreKind,
    pub label: &'static str,
    pub legacy: LegacyFileState,
    pub volume: VolumeState,
    pub resolution: ResolutionSource,
    pub descriptor: String,
}

#[derive(Debug, Clone)]
pub struct StoragePlanReport {
    pub storage_root: PathBuf,
    pub disk_path: PathBuf,
    pub disk_exists: bool,
    pub stores: Vec<StorePlan>,
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

    pub fn plan(&self) -> Result<StoragePlanReport> {
        let vault_path = self.config.credentials.vault_path.clone();
        Ok(StoragePlanReport {
            storage_root: self.storage_root.clone(),
            disk_path: self.disk_path.clone(),
            disk_exists: self.disk_path.exists(),
            stores: vec![
                self.plan_store(StoreKind::Vault, &vault_path)?,
                self.plan_store(StoreKind::Agents, &vault_path)?,
                self.plan_store(StoreKind::Sessions, &vault_path)?,
                self.plan_store(StoreKind::Cron, &vault_path)?,
                self.plan_store(StoreKind::Routing, &vault_path)?,
            ],
        })
    }

    pub fn plan_store(&self, kind: StoreKind, vault_path: &Path) -> Result<StorePlan> {
        let label = kind.label();
        let legacy_path = kind.legacy_path(&self.storage_root, vault_path);
        let legacy = LegacyFileState {
            exists: legacy_path.exists(),
            size_bytes: std::fs::metadata(&legacy_path).ok().map(|m| m.len()),
            path: legacy_path.clone(),
        };

        let volume = if let Some(volume_name) = kind.volume_name() {
            let info = rockbot_vdisk::volume_info(&self.disk_path, volume_name)?;
            let header_kind = rockbot_vdisk::read_volume_prefix(&self.disk_path, volume_name, 4)?
                .map(|prefix| {
                    if prefix.as_slice() == b"redb" {
                        "plaintext_redb".to_string()
                    } else if prefix.is_empty() {
                        "empty".to_string()
                    } else {
                        "opaque_or_encrypted".to_string()
                    }
                });
            VolumeState {
                name: volume_name.to_string(),
                exists: info.is_some(),
                len_bytes: info.as_ref().map(|i| i.len),
                capacity_bytes: info.as_ref().map(|i| i.capacity),
                header_kind,
            }
        } else {
            VolumeState {
                name: label.to_string(),
                exists: false,
                len_bytes: None,
                capacity_bytes: None,
                header_kind: None,
            }
        };

        let (resolution, descriptor) = match kind {
            StoreKind::Vault => {
                if legacy.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        format!("virtual disk {} volume 'vault' (plaintext)", self.disk_path.display()),
                    )
                } else if volume.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        format!("virtual disk {} volume 'vault' (plaintext)", self.disk_path.display()),
                    )
                } else {
                    (ResolutionSource::Missing, "unavailable".to_string())
                }
            }
            StoreKind::Agents => {
                if legacy.exists {
                    (ResolutionSource::Legacy, format!("legacy store {}", legacy.path.display()))
                } else if volume.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        encryption_mode_log(
                            self.key_for_label("agents")?.is_some(),
                            &format!("virtual disk {} volume 'agents'", self.disk_path.display()),
                        ),
                    )
                } else {
                    (ResolutionSource::Missing, "unavailable".to_string())
                }
            }
            StoreKind::Sessions => {
                if legacy.exists {
                    (ResolutionSource::Legacy, format!("legacy store {}", legacy.path.display()))
                } else if volume.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        encryption_mode_log(
                            self.key_for_label("sessions")?.is_some(),
                            &format!("virtual disk {} volume 'sessions'", self.disk_path.display()),
                        ),
                    )
                } else {
                    (
                        ResolutionSource::Recovery,
                        format!(
                            "recovery store {}",
                            self.storage_root.join("runtime").join("sessions.recovery.redb").display()
                        ),
                    )
                }
            }
            StoreKind::Cron => {
                if legacy.exists {
                    (ResolutionSource::Legacy, format!("legacy store {}", legacy.path.display()))
                } else if volume.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        encryption_mode_log(
                            self.key_for_label("cron")?.is_some(),
                            &format!("virtual disk {} volume 'cron'", self.disk_path.display()),
                        ),
                    )
                } else {
                    (ResolutionSource::Missing, "unavailable".to_string())
                }
            }
            StoreKind::Routing => {
                if legacy.exists {
                    (ResolutionSource::Legacy, format!("legacy store {}", legacy.path.display()))
                } else if volume.exists {
                    (
                        ResolutionSource::VirtualDisk,
                        encryption_mode_log(
                            self.key_for_label("routing")?.is_some(),
                            &format!("virtual disk {} volume 'routing'", self.disk_path.display()),
                        ),
                    )
                } else {
                    (ResolutionSource::Missing, "unavailable".to_string())
                }
            }
        };

        Ok(StorePlan {
            kind,
            label,
            legacy,
            volume,
            resolution,
            descriptor,
        })
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

    pub async fn open_routing_store(&self) -> Result<OpenedStore> {
        let key = self.key_for_label("routing")?;
        let legacy_path = self.storage_root.join("data").join("routing.redb");
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
                        "Could not open legacy routing store {}: {err}. Falling back to virtual-disk routing volume.",
                        legacy_path.display()
                    );
                }
            }
        }

        let store = Store::open_volume(&self.disk_path, "routing", 64 * 1024 * 1024, key)?;
        Ok(OpenedStore {
            store: Arc::new(store),
            descriptor: encryption_mode_log(
                key.is_some(),
                &format!("virtual disk {} volume 'routing'", self.disk_path.display()),
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

    pub fn agent_context_root(&self) -> PathBuf {
        self.storage_root.join("agents")
    }

    pub async fn initialize_agent_context(
        &self,
        agent_id: &str,
        system_prompt: Option<&str>,
    ) -> Result<PathBuf> {
        let agent_dir = agent_context_dir(&self.storage_root, agent_id)?;
        tokio::fs::create_dir_all(&agent_dir).await?;

        ensure_agent_context_file(
            &agent_dir.join("SOUL.md"),
            "# Agent Identity\n\n\
             You are a capable autonomous agent. You accomplish tasks by taking direct action \
             using your tools — never by describing what you would do.\n\n\
             ## Principles\n\n\
             - Act decisively. Start working immediately when given a task.\n\
             - Be thorough. Complete every step before reporting results.\n\
             - Be resilient. When something fails, analyze the error and try a different approach.\n\
             - Be self-sufficient. Never ask the user to do something you can do with your tools.\n",
        )
        .await?;

        ensure_agent_context_file(
            &agent_dir.join("SYSTEM-PROMPT.md"),
            system_prompt
                .unwrap_or("# System Prompt\n\nCustomize this agent's system prompt here.\n"),
        )
        .await?;

        ensure_agent_context_file(
            &agent_dir.join("AGENTS.md"),
            "# Operational Guidelines\n\n\
             Define behavioral rules, constraints, and standard operating procedures here.\n",
        )
        .await?;

        ensure_agent_context_file(
            &agent_dir.join("MEMORY.md"),
            "# Memory Guidelines\n\n\
             Describe how this agent should use its memory tools, what to remember,\n\
             and how to organize stored knowledge.\n",
        )
        .await?;

        Ok(agent_dir)
    }

    pub async fn list_agent_context_files(&self, agent_id: &str) -> Result<Vec<AgentContextFileInfo>> {
        list_agent_context_files(&self.storage_root, agent_id).await
    }

    pub async fn read_agent_context_file(&self, agent_id: &str, filename: &str) -> Result<String> {
        read_agent_context_file(&self.storage_root, agent_id, filename).await
    }

    pub async fn write_agent_context_file(
        &self,
        agent_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<()> {
        write_agent_context_file(&self.storage_root, agent_id, filename, content).await
    }

    pub async fn delete_agent_context_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        delete_agent_context_file(&self.storage_root, agent_id, filename).await
    }
}

impl StoreKind {
    pub fn label(self) -> &'static str {
        match self {
            StoreKind::Vault => "vault",
            StoreKind::Agents => "agents",
            StoreKind::Sessions => "sessions",
            StoreKind::Cron => "cron",
            StoreKind::Routing => "routing",
        }
    }

    pub fn volume_name(self) -> Option<&'static str> {
        Some(self.label())
    }

    pub fn legacy_path(self, storage_root: &Path, vault_path: &Path) -> PathBuf {
        match self {
            StoreKind::Vault => vault_path.join("vault.db"),
            StoreKind::Agents => vault_path.join("agents.redb"),
            StoreKind::Sessions => storage_root.join("data").join("sessions.redb"),
            StoreKind::Cron => storage_root.join("data").join("cron.redb"),
            StoreKind::Routing => storage_root.join("data").join("routing.redb"),
        }
    }
}

pub fn default_pki_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
        .join("pki")
}

pub fn default_storage_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
}

pub fn is_valid_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= 64
        && !agent_id.contains('/')
        && !agent_id.contains('\\')
        && !agent_id.contains("..")
        && agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn is_valid_agent_context_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.ends_with(".md")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn agent_context_dir(storage_root: &Path, agent_id: &str) -> Result<PathBuf> {
    if !is_valid_agent_id(agent_id) {
        return Err(anyhow!("invalid agent id"));
    }
    Ok(storage_root.join("agents").join(agent_id))
}

async fn ensure_agent_context_file(path: &Path, content: &str) -> Result<()> {
    if tokio::fs::metadata(path).await.is_err() {
        tokio::fs::write(path, content).await?;
    }
    Ok(())
}

pub async fn list_agent_context_files(
    storage_root: &Path,
    agent_id: &str,
) -> Result<Vec<AgentContextFileInfo>> {
    let agent_dir = agent_context_dir(storage_root, agent_id)?;
    let mut files: Vec<AgentContextFileInfo> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for &name in WELL_KNOWN_AGENT_CONTEXT_FILES {
        let file_path = agent_dir.join(name);
        let (exists, size) = match tokio::fs::metadata(&file_path).await {
            Ok(meta) => (true, meta.len()),
            Err(_) => (false, 0),
        };
        seen.insert(name.to_string());
        files.push(AgentContextFileInfo {
            name: name.to_string(),
            exists,
            size_bytes: size,
            well_known: true,
        });
    }

    if let Ok(mut entries) = tokio::fs::read_dir(&agent_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !seen.contains(&name) {
                let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                files.push(AgentContextFileInfo {
                    name,
                    exists: true,
                    size_bytes: size,
                    well_known: false,
                });
            }
        }
    }

    Ok(files)
}

pub async fn read_agent_context_file(
    storage_root: &Path,
    agent_id: &str,
    filename: &str,
) -> Result<String> {
    if !is_valid_agent_context_filename(filename) {
        return Err(anyhow!("invalid filename"));
    }
    let path = agent_context_dir(storage_root, agent_id)?.join(filename);
    Ok(tokio::fs::read_to_string(path).await?)
}

pub async fn write_agent_context_file(
    storage_root: &Path,
    agent_id: &str,
    filename: &str,
    content: &str,
) -> Result<()> {
    if !is_valid_agent_context_filename(filename) {
        return Err(anyhow!("invalid filename"));
    }
    let agent_dir = agent_context_dir(storage_root, agent_id)?;
    tokio::fs::create_dir_all(&agent_dir).await?;
    tokio::fs::write(agent_dir.join(filename), content).await?;
    Ok(())
}

pub async fn delete_agent_context_file(
    storage_root: &Path,
    agent_id: &str,
    filename: &str,
) -> Result<()> {
    if filename == "SOUL.md" {
        return Err(anyhow!("Cannot delete SOUL.md"));
    }
    if !is_valid_agent_context_filename(filename) {
        return Err(anyhow!("invalid filename"));
    }
    let path = agent_context_dir(storage_root, agent_id)?.join(filename);
    tokio::fs::remove_file(path).await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cfg_with_root(root: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.credentials.vault_path = root.join("vault");
        cfg.security.storage.enabled = false;
        cfg
    }

    #[tokio::test]
    async fn sessions_plan_prefers_legacy_when_present() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        let legacy = root.join("data").join("sessions.redb");
        std::fs::write(&legacy, b"redb").unwrap();

        let runtime = StorageRuntime::new_with_root_sync(&cfg_with_root(root), root.to_path_buf()).unwrap();
        let plan = runtime.plan_store(StoreKind::Sessions, &root.join("vault")).unwrap();

        assert_eq!(plan.resolution, ResolutionSource::Legacy);
        assert!(plan.descriptor.contains("legacy store"));
    }

    #[tokio::test]
    async fn sessions_open_uses_recovery_when_legacy_is_invalid() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data").join("sessions.redb"), b"not-redb").unwrap();

        let runtime = StorageRuntime::new_with_root_sync(&cfg_with_root(root), root.to_path_buf()).unwrap();
        let opened = runtime.open_sessions_store().await.unwrap();

        assert_eq!(opened.mode, StoreMode::Recovery);
        assert!(opened.descriptor.contains("sessions.recovery.redb"));
    }

    #[tokio::test]
    async fn agents_plan_prefers_vdisk_when_legacy_missing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("vault")).unwrap();
        let cfg = cfg_with_root(root);
        let runtime = StorageRuntime::new_with_root_sync(&cfg, root.to_path_buf()).unwrap();
        let _ = Store::open_volume(runtime.disk_path(), "agents", 128 * 1024 * 1024, None).unwrap();

        let plan = runtime.plan_store(StoreKind::Agents, &root.join("vault")).unwrap();
        assert_eq!(plan.resolution, ResolutionSource::VirtualDisk);
        assert!(plan.descriptor.contains("volume 'agents'"));
    }

    #[tokio::test]
    async fn agent_context_files_round_trip_through_runtime_interface() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let cfg = cfg_with_root(root);
        let runtime = StorageRuntime::new_with_root_sync(&cfg, root.to_path_buf()).unwrap();

        runtime
            .initialize_agent_context("hex", Some("# prompt"))
            .await
            .unwrap();
        runtime
            .write_agent_context_file("hex", "MEMORY.md", "# mem")
            .await
            .unwrap();

        let files = runtime.list_agent_context_files("hex").await.unwrap();
        assert!(files.iter().any(|f| f.name == "SOUL.md" && f.exists));
        assert!(files.iter().any(|f| f.name == "MEMORY.md" && f.exists));

        let memory = runtime.read_agent_context_file("hex", "MEMORY.md").await.unwrap();
        assert_eq!(memory, "# mem");
    }
}
