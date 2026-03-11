# Getting Started with RockBot

This guide will help you install, configure, and run RockBot for the first time.

## Prerequisites

- **Rust 1.75+** - Install via [rustup](https://rustup.rs/)
- **SQLite 3** - Usually pre-installed on Linux/macOS
- **OpenSSL** - For TLS support

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/TrippingKelsea/rockbot.git
cd rockbot

# Build release binary
cargo build --release

# Binary is at ./target/release/rockbot
```

### Verify Installation

```bash
./target/release/rockbot --version
# rockbot 0.1.0

./target/release/rockbot doctor
# Runs diagnostic checks
```

## Initial Configuration

### Generate Default Config

```bash
rockbot config init
# Creates ~/.config/rockbot/rockbot.toml
```

### Minimal Configuration

```toml
# ~/.config/rockbot/rockbot.toml

[gateway]
bind_host = "127.0.0.1"
port = 8765

[agents.defaults]
model = "anthropic/claude-sonnet-4-20250514"
workspace = "~/.local/share/rockbot/agents"

[[agents.list]]
id = "main"

[tools]
profile = "standard"

[credentials]
enabled = true
vault_path = "~/.local/share/rockbot/credentials"
```

See [Configuration Reference](configuration.md) for all options.

## First Run

### 1. Start the Gateway

```bash
rockbot gateway
# INFO Starting gateway on 127.0.0.1:8765
```

The gateway runs in the foreground. Use `Ctrl+C` to stop.

### 2. Check Health

```bash
curl http://localhost:8765/health
# {"status":"ok","version":"0.1.0"}
```

### 3. Open the Web UI

Navigate to [http://localhost:8765](http://localhost:8765) in your browser.

### 4. Or Use the TUI

```bash
rockbot tui
```

Use arrow keys to navigate, `q` to quit.

## Setting Up Credentials

### Initialize the Vault

The credential vault is automatically created on first use. You'll be prompted for a master password.

```bash
rockbot credentials status
# Vault: Not initialized
# Would you like to create a new vault? [y/N]
```

### Add Your First Endpoint

```bash
# Add a Home Assistant endpoint
rockbot credentials add homeassistant \
  --type home_assistant \
  --url http://homeassistant.local:8123

# You'll be prompted for the access token
Enter secret (will not echo): ********
```

### List Endpoints

```bash
rockbot credentials list
# ID                                    Name           Type              URL
# a1b2c3d4-...                         homeassistant  home_assistant    http://homeassistant.local:8123
```

## Next Steps

- [Configure your agents](configuration.md#agents)
- [Learn the CLI commands](cli-reference.md)
- [Explore the TUI](tui-guide.md)
- [Set up credential permissions](credentials.md)

## Troubleshooting

### Gateway Won't Start

Check if the port is already in use:
```bash
lsof -i :8765
```

### Vault Won't Unlock

If you forgot your password, the vault must be recreated:
```bash
rm -rf ~/.local/share/rockbot/credentials
rockbot credentials status  # Will prompt to create new vault
```

### Configuration Errors

Validate your config:
```bash
rockbot config validate
```

Show current config with resolved paths:
```bash
rockbot config show
```
