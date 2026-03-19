#![allow(clippy::unwrap_used, clippy::expect_used)]

use rockbot_storage_runtime::StorageRuntime;

#[tokio::test]
async fn extracts_agent_vdisk_documents_to_flat_files() {
    let base = std::env::current_dir()
        .unwrap_or_default()
        .join("target")
        .join("tmp-tests");
    std::fs::create_dir_all(&base).unwrap();
    let temp = tempfile::tempdir_in(base).unwrap();
    let mut config = rockbot_config::Config::default();
    config.credentials.vault_path = temp.path().join("vault");
    config.security.storage.enabled = false;
    let runtime = StorageRuntime::new_with_root_sync(&config, temp.path().to_path_buf()).unwrap();

    runtime
        .initialize_agent_context("hex", Some("# prompt"))
        .await
        .unwrap();
    runtime
        .write_agent_context_file("hex", "MEMORY.md", "# memory")
        .await
        .unwrap();

    let out_root = temp.path().join("export");
    let manifest = rockbot_storage_utility::extract_agent_vdisk(&runtime, "hex", &out_root)
        .await
        .unwrap();

    assert_eq!(manifest.agent_id, "hex");
    assert!(out_root.join("hex").join("SOUL.md").exists());
    assert!(out_root.join("hex").join("MEMORY.md").exists());
    assert!(out_root.join("hex").join("manifest.json").exists());
}
