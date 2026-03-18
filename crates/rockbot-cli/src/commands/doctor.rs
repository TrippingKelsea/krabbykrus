//! Health and diagnostics commands
//!
//! When the `doctor-ai` feature is enabled, provides AI-powered config diagnosis,
//! auto-repair, and migration detection using a local GGUF model.

use crate::load_config;
use anyhow::Result;
use std::path::PathBuf;

use crate::DoctorCommands;

/// Run health diagnostics
pub async fn run(command: &Option<DoctorCommands>, config_path: &PathBuf) -> Result<()> {
    match command {
        None | Some(DoctorCommands::Check) => run_health_check(config_path).await,
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Diagnose { config }) => {
            let path = config.as_ref().unwrap_or(config_path);
            run_diagnose(path).await
        }
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Migrate { config }) => {
            let path = config.as_ref().unwrap_or(config_path);
            run_migrate(path).await
        }
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Storage { config }) => {
            let path = config.as_ref().unwrap_or(config_path);
            run_storage(path).await
        }
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Download) => run_download().await,
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Status) => run_status().await,
        #[cfg(feature = "doctor-ai")]
        Some(DoctorCommands::Tui) => rockbot_tui::doctor_tui::run_doctor_tui(config_path).await,
    }
}

/// Basic health check (always available, no AI needed)
async fn run_health_check(config_path: &PathBuf) -> Result<()> {
    println!("RockBot Health Check");
    println!("========================");

    // Check configuration
    print!("Configuration: ");
    let config = match load_config(config_path).await {
        Ok(c) => {
            println!("Valid");
            c
        }
        Err(e) => {
            println!("Invalid - {e}");
            #[cfg(feature = "doctor-ai")]
            {
                println!("\nDoctor AI can help diagnose this issue.");
                println!("Run: rockbot doctor diagnose");
            }
            return Ok(());
        }
    };

    // Check workspace directories
    print!("Workspace access: ");
    let workspace_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot");

    if tokio::fs::metadata(&workspace_dir).await.is_ok() {
        println!("Accessible");
    } else {
        println!("Directory doesn't exist, will be created");
    }

    // Check database path
    print!("Database directory: ");
    let db_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
        .join("data");

    if tokio::fs::metadata(&db_dir).await.is_ok() {
        println!("Accessible");
    } else {
        println!("Directory doesn't exist, will be created");
    }

    // Check gateway auth configuration
    print!("Gateway Auth: ");
    if config.gateway.is_localhost() {
        println!("Localhost (no auth required)");
    } else if config.gateway.requires_api_key() {
        println!("API key required (non-localhost bind)");
        println!("     Create one with: rockbot credentials api-key create");
    } else {
        println!("Warning: Non-localhost bind without auth");
    }

    // Doctor AI status
    #[cfg(feature = "doctor-ai")]
    {
        print!("Doctor AI: ");
        if config.doctor.is_some() {
            println!("Configured");
        } else {
            println!("Not configured (using defaults)");
        }
    }

    println!("\nSystem Status: Ready to start");
    println!("   Run 'rockbot gateway' to start the server");

    Ok(())
}

// --- AI-powered subcommands (only when doctor-ai is enabled) ---

#[cfg(feature = "doctor-ai")]
async fn run_diagnose(config_path: &PathBuf) -> Result<()> {
    use rockbot_doctor::DoctorAi;

    println!("Doctor AI: Diagnosing config...\n");

    let raw_toml = tokio::fs::read_to_string(config_path).await?;

    // Try to parse the config normally
    let parse_error = match raw_toml.parse::<toml::Value>() {
        Ok(_) => {
            // TOML syntax is fine, try full deserialization
            match rockbot_config::Config::from_file(config_path).await {
                Ok(_) => {
                    println!("Config is valid! No issues detected.");
                    return Ok(());
                }
                Err(e) => format!("{e}"),
            }
        }
        Err(e) => format!("{e}"),
    };

    // Initialize doctor AI
    let doctor_config = try_parse_doctor_config_from_raw(&raw_toml);
    let mut doctor = DoctorAi::init(doctor_config).await?;

    // Diagnose the error
    let diagnosis = doctor.diagnose_parse_error(&raw_toml, &parse_error).await;

    // Display diagnosis
    println!("  Error Type: {}", diagnosis.kind.label());
    if let Some(ref field) = diagnosis.field_path {
        println!("  Field:      {field}");
    }
    if let Some(line) = diagnosis.line {
        println!("  Line:       {line}");
    }
    println!("  Raw Error:  {}", diagnosis.raw_error);
    if !diagnosis.explanation.is_empty() {
        println!("\n  Explanation:");
        for line in diagnosis.explanation.lines() {
            println!("    {line}");
        }
    }

    // Suggest a fix
    if let Some(fix) = doctor.suggest_fix(&raw_toml, &diagnosis).await {
        println!("\n  Suggested Fix: {}", fix.describe());

        if doctor.auto_fix_enabled() {
            println!("\n  Auto-fix is enabled. Applying...");
            let patched = rockbot_doctor::repair::apply_fix(&raw_toml, &fix)?;
            match patched.parse::<toml::Value>() {
                Ok(_) => {
                    tokio::fs::write(config_path, &patched).await?;
                    println!("  Config updated and verified at {}", config_path.display());
                    doctor.record_successful_fix(&diagnosis, &fix);
                }
                Err(verify_err) => {
                    println!("  Fix verification failed: {verify_err}");
                    println!("  Original config preserved.");
                }
            }
        } else {
            println!("  Run with [doctor] auto_fix = true to apply automatically.");
        }
    }

    Ok(())
}

#[cfg(feature = "doctor-ai")]
async fn run_migrate(config_path: &PathBuf) -> Result<()> {
    use rockbot_doctor::{DoctorAi, MigrationSource};

    println!("Doctor AI: Checking for config migrations...\n");

    let raw_toml = tokio::fs::read_to_string(config_path).await?;
    let doctor_config = try_parse_doctor_config_from_raw(&raw_toml);
    let doctor = DoctorAi::init(doctor_config).await?;

    let notes = doctor.check_migration(&raw_toml).await;

    if notes.is_empty() {
        println!("  No deprecated or renamed fields detected.");
        return Ok(());
    }

    println!("  Found {} migration note(s):\n", notes.len());
    for note in &notes {
        let confidence = match note.source {
            MigrationSource::StaticTable => "[known]",
            MigrationSource::AiDetected => "[detected]",
            MigrationSource::Learned => "[learned]",
        };
        let arrow = match &note.new_path {
            Some(new) => format!("-> {new}"),
            None => "(removed)".to_string(),
        };
        let version = note
            .since_version
            .as_deref()
            .map(|v| format!(" (since {v})"))
            .unwrap_or_default();

        println!("  {confidence} {}{arrow}{version}", note.old_path);
    }

    Ok(())
}

#[cfg(feature = "doctor-ai")]
async fn run_storage(config_path: &PathBuf) -> Result<()> {
    use rockbot_doctor::{inspect_storage, recommended_actions, summarize_report, DoctorAi};
    use std::time::Duration;

    println!("Doctor AI: Inspecting storage state...\n");
    let report = inspect_storage(config_path);
    let summary = summarize_report(&report);
    println!("{summary}");
    println!("Recommended next steps:");
    for action in recommended_actions(&report) {
        println!("- {action}");
    }
    println!();

    let raw_toml = tokio::fs::read_to_string(config_path).await.unwrap_or_default();
    let doctor_config = try_parse_doctor_config_from_raw(&raw_toml);
    let doctor = DoctorAi::init(doctor_config).await?;
    let analysis = tokio::time::timeout(
        Duration::from_secs(10),
        doctor.diagnose_storage_report(&report),
    )
    .await;

    match analysis {
        Ok(text) if !text.trim().is_empty() => {
            println!("Doctor AI Assessment:\n");
            println!("{text}");
        }
        Ok(_) => {}
        Err(_) => {
            println!("Doctor AI Assessment:\n");
            println!("The deterministic storage report is complete, but the AI explanation timed out after 10 seconds.");
        }
    }

    Ok(())
}

#[cfg(feature = "doctor-ai")]
async fn run_download() -> Result<()> {
    use rockbot_doctor::DoctorConfig;

    println!("Doctor AI: Downloading model...\n");

    let config = DoctorConfig::default();
    println!("  Model: {}/{}", config.model_id, config.model_filename);

    // Just initializing the doctor triggers download
    let _doctor = rockbot_doctor::DoctorAi::init(config).await?;
    println!("  Model downloaded and ready.");

    Ok(())
}

#[cfg(feature = "doctor-ai")]
async fn run_status() -> Result<()> {
    use rockbot_doctor::DoctorConfig;

    println!("Doctor AI Status\n");

    let config = DoctorConfig::default();
    println!(
        "  Model:       {}/{}",
        config.model_id, config.model_filename
    );
    println!("  Max tokens:  {}", config.max_tokens);
    println!("  Temperature: {}", config.temperature);
    println!("  Top-p:       {}", config.top_p);
    println!("  Seed:        {}", config.seed);
    println!("  Auto-fix:    {}", config.auto_fix);

    // Check if model is cached
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rockbot")
        .join("doctor");
    let model_path = cache_dir.join(&config.model_filename);
    if model_path.exists() {
        println!("  Cached:      yes ({})", model_path.display());
    } else {
        println!("  Cached:      no (run `rockbot doctor download` to fetch)");
    }

    Ok(())
}

/// Try to extract DoctorConfig from raw TOML even if the full config doesn't parse.
///
/// Public so the startup interceptor in `lib.rs` can reuse it.
#[cfg(feature = "doctor-ai")]
pub fn try_parse_doctor_config_from_raw(raw_toml: &str) -> rockbot_doctor::DoctorConfig {
    // Parse permissively, extract just the [doctor] section
    if let Ok(value) = raw_toml.parse::<toml::Value>() {
        if let Some(doctor_section) = value.get("doctor") {
            if let Ok(config) =
                serde_json::from_str::<rockbot_doctor::DoctorConfig>(&doctor_section.to_string())
            {
                return config;
            }
            // toml::Value serializes differently, try via serde_json roundtrip
            if let Ok(json) = serde_json::to_string(doctor_section) {
                if let Ok(config) = serde_json::from_str::<rockbot_doctor::DoctorConfig>(&json) {
                    return config;
                }
            }
        }
    }
    rockbot_doctor::DoctorConfig::default()
}
