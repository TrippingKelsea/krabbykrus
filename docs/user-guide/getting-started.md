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

### Open the Web UI

Navigate to `https://localhost:18080` in your browser. Accept the
self-signed certificate when prompted.

## Remote Tool Execution

Build with the `remote-exec` feature to let the gateway dispatch tool calls
(file reads, shell commands) to your local machine:

```bash
cargo build --release -F remote-exec
```

When the TUI connects, it automatically registers as a remote executor via
a Noise Protocol encrypted channel.

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
| `remote-exec` | Noise Protocol remote tool dispatch |
| `overseer` | Embedded local-model agent oversight |
| `otel` | OpenTelemetry export |
| `http-insecure` | Allow plain HTTP (TLS is default) |
| `anthropic` | Anthropic API provider |
| `openai` | OpenAI API provider |
| `ollama` | Ollama local models |

## Troubleshooting

**Gateway won't start:**
```bash
# Check if port is in use
ss -tlnp | grep 18080
```

**TLS certificate issues:**
```bash
# Regenerate certificate
rockbot config init --force
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
