use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct AgentExtractionManifest {
    pub agent_id: String,
    pub source_vdisk: String,
    pub extracted_at: String,
    pub files: Vec<ExtractedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractedFile {
    pub name: String,
    pub size_bytes: u64,
    pub well_known: bool,
}

pub async fn extract_agent_vdisk(
    runtime: &rockbot_storage_runtime::StorageRuntime,
    agent_id: &str,
    out_dir: &Path,
) -> Result<AgentExtractionManifest> {
    let export_root = out_dir.join(agent_id);
    tokio::fs::create_dir_all(&export_root).await?;

    let files = runtime.list_agent_context_files(agent_id).await?;
    let mut manifest_files = Vec::new();

    for file in files.into_iter().filter(|file| file.exists) {
        let content = runtime.read_agent_context_file(agent_id, &file.name).await?;
        tokio::fs::write(export_root.join(&file.name), content).await?;
        manifest_files.push(ExtractedFile {
            name: file.name,
            size_bytes: file.size_bytes,
            well_known: file.well_known,
        });
    }

    let manifest = AgentExtractionManifest {
        agent_id: agent_id.to_string(),
        source_vdisk: runtime.agent_vdisk_path(agent_id)?.display().to_string(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        files: manifest_files,
    };

    tokio::fs::write(
        export_root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;

    Ok(manifest)
}

pub fn default_agent_extract_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("exports")
        .join("agents")
}
