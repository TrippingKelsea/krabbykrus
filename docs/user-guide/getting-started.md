# Getting Started

## Prerequisites

- **Rust 1.75+** — install via [rustup](https://rustup.rs/)

## Installation

```bash
git clone https://github.com/TrippingKelsea/rockbot.git
cd rockbot
make release
```

The binary is at `./target/release/rockbot`.

The Make targets default to the `enhanced` feature profile:

```bash
make dev
make release
make test
```

You can also generate shell completions directly from the CLI:

```bash
./rockbot completion zsh
./rockbot completion bash
./rockbot completion fish
```

```bash
rockbot --version
rockbot doctor        # diagnostic checks
```

The top-level `rockbot` crate defaults to the `conservative` feature profile:
Bedrock, Telegram, Signal, and the built-in tool crates. Additional profiles and
feature bundles are available when you need more providers or infrastructure.

## Initial Setup

### Generate Gateway Config

```bash
rockbot config init gateway --https-port 18181 --client-port 18182
# Creates ~/.config/rockbot/rockbot.toml
# Generates TLS certificate at ~/.config/rockbot/gateway.{crt,key}
```

This creates a bootstrap-only gateway config and a self-signed TLS
certificate. Runtime entities such as agents should live in the replicated
store, not in the TOML file.

### Minimal Configuration

```toml
# ~/.config/rockbot/rockbot.toml

[gateway]
bind_host = "0.0.0.0"
port = 18181
client_port = 18182

[gateway.public]
serve_webapp = true
serve_ca = true
enrollment_enabled = true

[pki]
tls_cert = "/home/you/.config/rockbot/gateway.crt"
tls_key = "/home/you/.config/rockbot/gateway.key"

[client]
gateway_host = "127.0.0.1"
https_port = 18181
client_port = 18182
```

## Running

### Start the Gateway

```bash
rockbot gateway run
# INFO Gateway public listener on 0.0.0.0:18181 (TLS)
# INFO Gateway client listener on 0.0.0.0:18182 (TLS/mTLS)
```

### Connect with the TUI

From the same machine:
```bash
rockbot tui
```

From another machine on the network:
```bash
rockbot config init client --gateway-ip 192.168.1.10 --https-port 18181 --client-port 18182
rockbot tui
```

The client bootstrap config points the TUI at the dedicated client listener.
You can still override it with `-g host:port` when needed.

Native clients use the client-listener WebSocket for both chat traffic and
gateway control-plane requests such as provider, agent, and session management.
The public HTTPS listener is intentionally minimal: browser bootstrap shell,
`/static/*`, health, CA publication, and optional enrollment.

### Credential Management UI

For vault setup and inspection from the terminal, you can also launch the
standalone credential UI:

```bash
rockbot credentials ui
```

### Open the Web UI Bootstrap

Navigate to `https://localhost:18181` in your browser and accept the
self-signed certificate when prompted. The browser app is now a bootstrap shell
served from the public listener. It exposes only:

- `/`
- `/static/*`
- `/health`
- `/api/cert/ca`
- `/api/cert/sign` when `gateway.public.enrollment_enabled = true`

The page lets you import a client certificate/key bundle into browser storage
and authenticate to the browser WebSocket control plane without exposing the
full REST management surface publicly.

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

# Install into rockbot.toml (writes [pki] and enables gateway mTLS policy)
rockbot cert install --name gateway

# Generate a TUI client certificate
rockbot cert client generate --name my-tui --role tui --days 365
```

### Enroll a Remote Client

Enrollment happens over the public HTTPS listener, so you do not need to
temporarily disable client-certificate enforcement for the client listener. If
you do not want browser/bootstrap enrollment exposed, set:

```toml
[gateway.public]
enrollment_enabled = false
```

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

If you only want the Noise transport primitives without remote executor
dispatch, build with:

```bash
cargo build --release -F noise
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
| `noise` | Noise handshake and transport primitives |
| `remote-exec` | Remote tool dispatch built on Noise |
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
