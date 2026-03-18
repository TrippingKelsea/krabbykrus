//! Deterministic storage diagnostics for mixed legacy/vdisk deployments.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageReport {
    pub storage_root: PathBuf,
    pub disk_path: PathBuf,
    pub disk_exists: bool,
    pub legacy_files: Vec<LegacyStoreFile>,
    pub volumes: Vec<VolumeState>,
    pub findings: Vec<StorageFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyStoreFile {
    pub label: String,
    pub path: PathBuf,
    pub exists: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeState {
    pub name: String,
    pub exists: bool,
    pub len_bytes: Option<u64>,
    pub capacity_bytes: Option<u64>,
    pub header_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageFinding {
    pub severity: FindingSeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

pub fn inspect_storage(config_path: &Path) -> StorageReport {
    let storage_root = config_path
        .parent()
        .map(Path::to_path_buf)
        .or_else(|| dirs::config_dir().map(|p| p.join("rockbot")))
        .unwrap_or_else(|| PathBuf::from("."));
    let disk_path = storage_root.join("rockbot.data");

    let legacy_files = [
        ("vault", storage_root.join("vault").join("vault.db")),
        ("agents", storage_root.join("vault").join("agents.redb")),
        ("sessions", storage_root.join("data").join("sessions.redb")),
        ("cron", storage_root.join("data").join("cron.redb")),
    ]
    .into_iter()
    .map(|(label, path)| LegacyStoreFile {
        label: label.to_string(),
        size_bytes: std::fs::metadata(&path).ok().map(|m| m.len()),
        exists: path.exists(),
        path,
    })
    .collect::<Vec<_>>();

    let volumes = ["vault", "agents", "sessions", "cron"]
        .into_iter()
        .map(|name| {
            let info = rockbot_vdisk::volume_info(&disk_path, name).ok().flatten();
            let header_kind = rockbot_vdisk::read_volume_prefix(&disk_path, name, 4)
                .ok()
                .flatten()
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
                name: name.to_string(),
                exists: info.is_some(),
                len_bytes: info.map(|i| i.len),
                capacity_bytes: info.map(|i| i.capacity),
                header_kind,
            }
        })
        .collect::<Vec<_>>();

    let mut findings = Vec::new();
    if !disk_path.exists() {
        findings.push(StorageFinding {
            severity: FindingSeverity::Warning,
            code: "missing_vdisk".to_string(),
            message: format!("Virtual disk {} does not exist yet.", disk_path.display()),
        });
    }

    for legacy in &legacy_files {
        let volume = volumes.iter().find(|v| v.name == legacy.label);
        if legacy.exists && volume.is_some_and(|v| v.exists) {
            findings.push(StorageFinding {
                severity: FindingSeverity::Info,
                code: format!("legacy_{}_coexists", legacy.label),
                message: format!(
                    "Legacy store {} still exists alongside the '{}' virtual-disk volume.",
                    legacy.path.display(),
                    legacy.label
                ),
            });
        } else if legacy.exists {
            findings.push(StorageFinding {
                severity: FindingSeverity::Info,
                code: format!("legacy_{}_present", legacy.label),
                message: format!(
                    "Legacy store {} is present and may need migration.",
                    legacy.path.display()
                ),
            });
        }
    }

    for volume in &volumes {
        if volume.exists && volume.len_bytes == Some(0) {
            findings.push(StorageFinding {
                severity: FindingSeverity::Warning,
                code: format!("{}_empty_volume", volume.name),
                message: format!("Virtual-disk volume '{}' exists but is empty.", volume.name),
            });
        }
        if volume.exists
            && volume.header_kind.as_deref() == Some("plaintext_redb")
            && volume.name != "vault"
        {
            findings.push(StorageFinding {
                severity: FindingSeverity::Warning,
                code: format!("{}_plaintext_volume", volume.name),
                message: format!(
                    "Virtual-disk volume '{}' appears to contain plaintext redb bytes.",
                    volume.name
                ),
            });
        }
    }

    StorageReport {
        storage_root,
        disk_exists: disk_path.exists(),
        disk_path,
        legacy_files,
        volumes,
        findings,
    }
}

pub fn summarize_report(report: &StorageReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Storage root: {}\nVirtual disk: {} ({})\n",
        report.storage_root.display(),
        report.disk_path.display(),
        if report.disk_exists { "present" } else { "missing" }
    ));

    out.push_str("Legacy stores:\n");
    for legacy in &report.legacy_files {
        out.push_str(&format!(
            "- {}: {} [{}]\n",
            legacy.label,
            legacy.path.display(),
            if legacy.exists {
                format!("present, {} bytes", legacy.size_bytes.unwrap_or(0))
            } else {
                "missing".to_string()
            }
        ));
    }

    out.push_str("Virtual-disk volumes:\n");
    for volume in &report.volumes {
        out.push_str(&format!(
            "- {}: {}",
            volume.name,
            if volume.exists { "present" } else { "missing" }
        ));
        if let Some(len) = volume.len_bytes {
            out.push_str(&format!(", len={len}"));
        }
        if let Some(capacity) = volume.capacity_bytes {
            out.push_str(&format!(", capacity={capacity}"));
        }
        if let Some(kind) = &volume.header_kind {
            out.push_str(&format!(", header={kind}"));
        }
        out.push('\n');
    }

    if report.findings.is_empty() {
        out.push_str("Findings:\n- none\n");
    } else {
        out.push_str("Findings:\n");
        for finding in &report.findings {
            out.push_str(&format!(
                "- {:?}: {} ({})\n",
                finding.severity, finding.message, finding.code
            ));
        }
    }
    out
}

pub fn recommended_actions(report: &StorageReport) -> Vec<String> {
    let mut actions = Vec::new();

    let legacy_sessions = report
        .legacy_files
        .iter()
        .find(|f| f.label == "sessions" && f.exists);
    let legacy_cron = report
        .legacy_files
        .iter()
        .find(|f| f.label == "cron" && f.exists);
    let legacy_agents = report
        .legacy_files
        .iter()
        .find(|f| f.label == "agents" && f.exists);
    let legacy_vault = report
        .legacy_files
        .iter()
        .find(|f| f.label == "vault" && f.exists);

    if legacy_sessions.is_some() || legacy_cron.is_some() || legacy_agents.is_some() || legacy_vault.is_some() {
        actions.push(
            "Legacy standalone stores still coexist with rockbot.data. Treat this node as mid-migration; prefer explicit migration/repair over assuming the vdisk volumes are authoritative.".to_string(),
        );
    }

    if let Some(volume) = report.volumes.iter().find(|v| v.name == "vault" && v.exists) {
        if volume.header_kind.as_deref() == Some("plaintext_redb") {
            actions.push(
                "The embedded 'vault' volume looks like plaintext redb bytes. Re-importing from the legacy vault file or marking the volume as migrated should be part of the next storage migration pass.".to_string(),
            );
        }
    }

    if report
        .volumes
        .iter()
        .any(|v| v.exists && v.header_kind.as_deref() == Some("opaque_or_encrypted"))
        && report.legacy_files.iter().any(|f| f.exists)
    {
        actions.push(
            "Opaque/encrypted vdisk volumes coexist with legacy files. Verify each volume explicitly before opening it in production; do not assume presence implies health.".to_string(),
        );
    }

    if actions.is_empty() {
        actions.push("No immediate storage migration actions detected.".to_string());
    }

    actions
}
