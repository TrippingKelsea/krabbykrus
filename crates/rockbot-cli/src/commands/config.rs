//! Configuration management commands

use crate::{load_config, ConfigCommands, ConfigInitCommands};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Run configuration commands
pub async fn run(command: &ConfigCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        ConfigCommands::Show => show_config(config_path).await,
        ConfigCommands::Validate => validate_config(config_path).await,
        ConfigCommands::Init { command } => match command {
            ConfigInitCommands::Gateway {
                output,
                force,
                https_port,
                client_port,
                bind_host,
                listen_ips,
            } => {
                init_gateway_config(
                    output.as_ref().unwrap_or(config_path),
                    *force,
                    bind_host,
                    listen_ips,
                    *https_port,
                    *client_port,
                )
                .await
            }
            ConfigInitCommands::Client {
                output,
                force,
                gateway_ip,
                https_port,
                client_port,
            } => {
                init_client_config(
                    output.as_ref().unwrap_or(config_path),
                    *force,
                    gateway_ip,
                    *https_port,
                    *client_port,
                )
                .await
            }
        },
    }
}

/// Show current configuration
async fn show_config(config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path).await?;

    let toml_string = toml::to_string_pretty(&config)?;
    println!("{toml_string}");

    Ok(())
}

/// Validate configuration
async fn validate_config(config_path: &PathBuf) -> Result<()> {
    match load_config(config_path).await {
        Ok(config) => {
            println!("✅ Configuration is valid");
            println!(
                "   Gateway HTTPS: {}:{}",
                config.gateway.bind_host, config.gateway.port
            );
            println!(
                "   Gateway Client: {}:{}",
                config.client.gateway_host, config.client.client_port
            );
            println!("   Agents: {} configured", config.agents.list.len());
            println!("   Tools: {} profile", config.tools.profile);
            println!("   Security: {} sandbox", config.security.sandbox.mode);
        }
        Err(e) => {
            println!("❌ Configuration is invalid: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn init_gateway_config(
    output_path: &Path,
    force: bool,
    bind_host: &str,
    listen_ips: &[String],
    https_port: u16,
    client_port: u16,
) -> Result<()> {
    ensure_output_path(output_path, force).await?;

    let config_dir = output_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let cert_path = config_dir.join("gateway.crt");
    let key_path = config_dir.join("gateway.key");
    let pki_dir = config_dir.join("pki");

    if !cert_path.exists() || !key_path.exists() || force {
        super::cert::generate_self_signed_cert(&cert_path, &key_path, listen_ips, 365).await?;
        println!("   TLS cert: {}", cert_path.display());
        println!("   TLS key:  {}", key_path.display());
    }

    let resolved_bind_host = listen_ips
        .first()
        .cloned()
        .unwrap_or_else(|| bind_host.to_string());
    let listen_ips_toml = if listen_ips.is_empty() {
        String::new()
    } else {
        format!(
            "listen_ips = [{}]\n",
            listen_ips
                .iter()
                .map(|ip| format!("\"{ip}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let toml = format!(
        r#"# RockBot gateway bootstrap config
# Runtime entities such as agents should live in the replicated store, not here.

[gateway]
bind_host = "{bind_host}"
{listen_ips}\
port = {https_port}
client_port = {client_port}
tls_cert = "{tls_cert}"
tls_key = "{tls_key}"
pki_dir = "{pki_dir}"

[client]
gateway_host = "127.0.0.1"
https_port = {https_port}
client_port = {client_port}

[security.storage]
enabled = true
mode = "encrypted_by_default"
key_source = "pki_local"

[security.roles]
gateway = true
vault_provider = false
"#,
        bind_host = resolved_bind_host,
        listen_ips = listen_ips_toml,
        tls_cert = cert_path.display(),
        tls_key = key_path.display(),
        pki_dir = pki_dir.display()
    );

    tokio::fs::write(output_path, toml).await?;

    println!("Gateway bootstrap config created at {}", output_path.display());
    if listen_ips.is_empty() {
        println!("   HTTPS/Web UI listener: {resolved_bind_host}:{https_port}");
        println!("   Client/mTLS listener:  {resolved_bind_host}:{client_port}");
    } else {
        println!(
            "   HTTPS/Web UI listeners: {}",
            listen_ips
                .iter()
                .map(|ip| format!("{ip}:{https_port}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "   Client/mTLS listeners:  {}",
            listen_ips
                .iter()
                .map(|ip| format!("{ip}:{client_port}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(())
}

async fn init_client_config(
    output_path: &Path,
    force: bool,
    gateway_ip: &str,
    https_port: u16,
    client_port: u16,
) -> Result<()> {
    ensure_output_path(output_path, force).await?;

    let toml = format!(
        r#"# RockBot client bootstrap config
# This file only contains connection bootstrap and local security settings.

[gateway]
bind_host = "127.0.0.1"
port = {https_port}
client_port = {client_port}

[client]
gateway_host = "{gateway_ip}"
https_port = {https_port}
client_port = {client_port}

[security.storage]
enabled = true
mode = "encrypted_by_default"
key_source = "pki_local"

[security.roles]
gateway = false
vault_provider = false
"#
    );

    tokio::fs::write(output_path, toml).await?;

    println!("Client bootstrap config created at {}", output_path.display());
    println!("   Gateway HTTPS/Web UI: {gateway_ip}:{https_port}");
    println!("   Gateway client port:  {gateway_ip}:{client_port}");

    Ok(())
}

async fn ensure_output_path(output_path: &Path, force: bool) -> Result<()> {
    if output_path.exists() && !force {
        anyhow::bail!(
            "Configuration file already exists: {}\nUse --force to overwrite",
            output_path.display()
        );
    }

    let config_dir = output_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    tokio::fs::create_dir_all(config_dir).await?;
    Ok(())
}
