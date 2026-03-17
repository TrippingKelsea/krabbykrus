# Getting Started

## Prerequisites

- **Rust 1.75+** — install via [rustup](https://rustup.rs/)

## Installation

```bash
git clone https://github.com/TrippingKelsea/rockbot.git
cd rockbot
cargo build --release
```

The binary is at `./target/release/rockbot`.

```bash
rockbot --version
rockbot doctor        # diagnostic checks
```

The top-level `rockbot` crate defaults to the `conservative` feature profile:
Bedrock, Telegram, Signal, and the built-in tool crates. Additional profiles and
feature bundles are available when you need more providers or infrastructure.

## Initial Setup

### Generate Config

```bash
rockbot config init
# Creates ~/.config/rockbot/rockbot.toml
# Generates TLS certificate at ~/.config/rockbot/gateway.{crt,key}
```

This creates a default configuration with one agent (`main`) using AWS
Bedrock, and a self-signed TLS certificate for the gateway.

### Minimal Configuration

```toml
# ~/.config/rockbot/rockbot.toml

[gateway]
bind_host = "0.0.0.0"
port = 18080
tls_cert = "/home/you/.config/rockbot/gateway.crt"
tls_key = "/home/you/.config/rockbot/gateway.key"

[agents.defaults]
model = "anthropic/claude-sonnet-4-20250514"

[[agents.list]]
id = "main"

[tools]
profile = "standard"
```

## Running

### Start the Gateway

```bash
rockbot gateway run
# INFO Gateway server listening on 0.0.0.0:18080 (TLS)
```

### Connect with the TUI

From the same machine:
```bash
rockbot tui
```

From another machine on the network:
```bash
rockbot tui -g 192.168.1.10:18080
```

The `-g` flag accepts bare `host:port` — no need to specify `https://`.

### Credential Management UI

For vault setup and inspection from the terminal, you can also launch the
standalone credential UI:

```bash
rockbot credentials ui
```

### Open the Web UI

Navigate to `https://localhost:18080` in your browser. Accept the
self-signed certificate when prompted.

## Mutual TLS (mTLS)

For production deployments, use the built-in PKI instead of self-signed
certificates. This ensures only authorized clients can connect.

### Set Up the CA and Certificates

```bash
# Initialize a Certificate Authority (valid 10 years)
rockbot cert ca generate --days 3650

# Generate a gateway certificate
rockbot cert client generate --name gateway --role gateway \
  --san localhost --san 127.0.0.1 --days 365

# Install into rockbot.toml (sets tls_cert, tls_key, tls_ca, require_client_cert)
rockbot cert install --name gateway

# Generate a TUI client certificate
rockbot cert client generate --name my-tui --role tui --days 365
```

### Enroll a Remote Client

If you need to provision a client on a different machine:

```bash
# On the CA host: create a one-time enrollment token
rockbot cert enroll create --role agent --uses 1 --expires 24h
# Output: Token: <uuid>

# On the remote client: enroll with the gateway
rockbot cert enroll submit \
  --gateway https://gateway-host:18080 \
  --psk <token> --name remote-agent --role agent

# Install into the client's config
rockbot cert install --name remote-agent
```

### View and Manage Certificates

```bash
rockbot cert client list           # list all issued certs
rockbot cert client info --name X  # details for one cert
rockbot cert client revoke --name X  # revoke (regenerates CRL)
rockbot cert client rotate --name X --days 365 --backup  # rotate
rockbot cert ca info               # CA details
```

See `docs/architecture/pki.md` for the full PKI reference.

## Remote Tool Execution

Build with the `remote-exec` feature to let the gateway dispatch tool calls
(file reads, shell commands) to your local machine:

```bash
cargo build --release -F remote-exec
```

When the TUI connects, it automatically registers as a remote executor after
the Noise handshake completes. The Dashboard also exposes Noise and execution
target cards so you can verify registration state and choose whether tools run
on the active client, the gateway, or another connected executor.

## Setting Up Credentials

### Initialize the Vault

```bash
rockbot credentials init
```

### Add an Endpoint

```bash
rockbot credentials add homeassistant \
  --type home_assistant \
  --url http://homeassistant.local:8123
# You'll be prompted for the access token
```

### List Endpoints

```bash
rockbot credentials list
```

## Feature Flags

| Flag | Description |
|------|-------------|
| `conservative` | Default profile: bedrock + telegram + signal + built-in tool crates |
| `enhanced` | Conservative plus overseer, doctor-ai, and vault replication |
| `experimental` | Enhanced plus telemetry and S3/Route53 deployment helpers |
| `remote-exec` | Noise Protocol remote tool dispatch |
| `overseer` | Embedded local-model agent oversight |
| `doctor-ai` | Local AI-powered configuration diagnostics and repair |
| `bedrock-deploy` | S3 CA distribution and Route53 DNS provisioning |
| `otel` | OpenTelemetry export |
| `http-insecure` | Allow plain HTTP (TLS is default) |
| `anthropic` | Anthropic API provider |
| `openai` | OpenAI API provider |
| `ollama` | Ollama local models |
| `all-providers` | Enable Anthropic, OpenAI, Ollama, and Bedrock together |
| `all-channels` | Enable Discord, Telegram, and Signal together |
| `all-tools` | Enable all built-in tool provider crates together |

## Troubleshooting

**Gateway won't start:**
```bash
# Check if port is in use
ss -tlnp | grep 18080
```

**TLS certificate issues:**
```bash
# Regenerate self-signed certificate (quick bootstrap)
rockbot config init --force

# Or inspect and verify existing certs
rockbot cert info --cert ~/.config/rockbot/pki/certs/gateway.crt
rockbot cert verify --cert gateway.crt --key gateway.key --ca ca.crt

# Rotate an expiring certificate
rockbot cert client rotate --name gateway --san localhost --days 365 --backup
```

**Vault won't unlock:**
```bash
# If you forgot your password, recreate the vault
rm -rf ~/.local/share/rockbot/credentials
rockbot credentials init
```

**Configuration errors:**
```bash
rockbot config validate
rockbot config show
```
