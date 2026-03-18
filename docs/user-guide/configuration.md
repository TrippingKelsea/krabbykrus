# Configuration Reference

RockBot is configured via a single TOML file, typically located at
`~/.config/rockbot/rockbot.toml`. Generate a default config with:

```bash
rockbot config init gateway --https-port 18181 --client-port 18182
```

Environment variables can be referenced with `${VAR}` or `${VAR:default}`
syntax. The config file supports hot-reloading — changes are picked up
automatically while the gateway is running.

## Top-Level Sections

```toml
[gateway]        # Gateway listener settings and mTLS policy
[pki]            # Shared cert/key/CA paths and enrollment bootstrap
[client]         # Remote gateway bootstrap target for clients/TUI
[agents]         # Deprecated agent bootstrap/migration section
[tools]          # Tool profiles and restrictions
[security]       # Sandbox and capability config
[credentials]    # Encrypted vault settings
[providers]      # LLM provider configuration
[overseer]       # Embedded oversight model (optional)
[doctor]         # Embedded doctor AI config (optional)
[deploy]         # S3 CA distribution + Route53 DNS (optional, requires bedrock-deploy feature)
[tui]            # TUI display preferences
[seed_model]     # Shared local GGUF model coordinates
```

---

## `[gateway]`

```toml
[gateway]
bind_host = "0.0.0.0"           # Host to bind (default: "127.0.0.1")
port = 18181                     # Public HTTPS / Web UI port
client_port = 18182              # Dedicated client / mTLS port
max_connections = 100            # Max concurrent connections (default: 100)
request_timeout = 30             # Request timeout in seconds (default: 30)
require_api_key = false          # Legacy programmatic access toggle

[gateway.public]
serve_webapp = true              # Serve / and /static/*
serve_ca = true                  # Serve GET /api/cert/ca
enrollment_enabled = true        # Serve POST /api/cert/sign
```

## `[pki]`

```toml
[pki]
tls_cert = "~/.config/rockbot/pki/certs/gateway.crt"
tls_key  = "~/.config/rockbot/pki/keys/gateway.key"

tls_ca = "~/.config/rockbot/pki/ca.crt"        # CA cert enables client verification
pki_dir = "~/.config/rockbot/pki"               # PKI directory for enrollment
require_client_cert = true                      # mTLS policy for the dedicated client listener
enrollment_psk = "secret-token"                 # PSK for POST /api/cert/sign
```

## `[client]`

```toml
[client]
gateway_host = "172.30.200.146"  # Remote gateway host/IP
https_port = 18181               # Public HTTPS / enrollment / Web UI port
client_port = 18182              # Dedicated client / mTLS listener
```

The public listener is intentionally narrow: browser bootstrap shell, `/static/*`,
health, CA publication, and optional enrollment. The client listener is for the
authenticated control plane: TUI, agent, remote-exec, and native WS API traffic.

### mTLS Modes

| `tls_ca` | `require_client_cert` | Behavior |
|----------|-----------------------|----------|
| unset | false | Standard TLS (server auth only) |
| set | false | Optional mTLS (accepts but doesn't require client certs) |
| set | true | Mandatory mTLS (rejects unauthenticated connections) |

---

## `[agents]`

### `[agents.defaults]`

```toml
[agents.defaults]
workspace = "~/.config/rockbot/workspace"   # Default workspace directory
model = "anthropic/claude-sonnet-4-20250514" # Default model
heartbeat_interval = "5m"                   # Agent heartbeat interval
max_context_tokens = 128000                 # Max context window (tokens)
```

### `[[agents.list]]` (deprecated)

> **Deprecated:** Agent configs should be stored in the vault instead. On first
> gateway startup with non-empty `agents.list`, entries are auto-migrated to the
> vault. After migration, this section can be removed from the TOML config.

Each agent is defined as an entry in the `agents.list` array:

```toml
[[agents.list]]
id = "main"                          # Unique agent identifier
model = "anthropic/claude-sonnet-4-20250514"  # Model override (optional)
workspace = "/home/user/projects"    # Workspace override (optional)
enabled = true                       # Enable/disable (default: true)
parent_id = "orchestrator"           # Parent agent (for subagents, optional)
system_prompt = "You are a helpful assistant."  # Override (optional)
temperature = 0.3                    # LLM temperature (default: 0.3)
max_tokens = 16000                   # Max response tokens (default: 16000)
max_tool_calls = 32                  # Max tool calls per turn (optional)
max_context_tokens = 128000          # Context window (default: 128000)
llm_timeout_secs = 45               # Per-LLM-call timeout (default: 45)
tool_timeout_secs = 120              # Per-tool-execution timeout (default: 120)

# Advanced features
planning_mode = "never"              # "never", "auto", "always", "approval_required"
reflection_enabled = false           # Self-critique after tool loop
episodic_memory = false              # Cross-session memory recall
guardrails = ["pii", "prompt_injection"]  # Enabled guardrails
breakpoint_tools = ["exec"]          # Tools that always require approval

# Expose as a callable tool for other agents
[agents.list.expose_as_tool]
tool_name = "code_reviewer"
description = "Reviews code for quality and security issues"

# MCP server connections
[agents.list.mcp_servers.filesystem]
command = "npx"
args = ["-y", "@anthropic/mcp-server-filesystem", "/home/user"]

# Workflow definition (DAG-based execution)
# [agents.list.workflow]
# See orchestration documentation for workflow schema
```

---

## `[tools]`

```toml
[tools]
profile = "standard"     # "minimal", "standard", or "full"
deny = ["exec"]          # Explicitly denied tools

# Tool-specific configuration
[tools.configs.web_search]
api_key = "${BRAVE_API_KEY}"
```

### Tool Profiles

| Profile | Tools |
|---------|-------|
| `minimal` | read, write, glob, grep |
| `standard` | + edit, exec, patch, invoke_agent, handoff, web_fetch, web_search, test, lint, clarify |
| `full` | + memory_get, memory_search, browser, blackboard_read, blackboard_write |

---

## `[security]`

```toml
[security.sandbox]
mode = "disabled"        # "disabled", "tools", "all"
scope = "session"        # "session", "tool", "none"
image = "rockbot-sandbox:latest"  # Container image (optional)

[security.capabilities.filesystem]
read_paths = ["/home/user/projects"]
write_paths = ["/home/user/projects"]
forbidden_paths = ["/etc", "/root"]

[security.capabilities.network]
allowed_domains = ["api.example.com"]
blocked_domains = ["internal.corp"]
max_request_size = 10485760    # 10 MB

[security.capabilities.process]
allowed_commands = ["git", "cargo", "npm"]
blocked_commands = ["rm", "dd"]
max_execution_time = 60

[security.storage]
enabled = true                # Encrypt supported redb stores by default
mode = "encrypted_by_default" # "disabled", "preferred", "encrypted_by_default"
key_source = "pki_local"      # "pki_local", "data_local", "external"
legacy_plaintext_fallback = false

[security.roles]
gateway = true
vault_provider = false
client = true
admin = false

[security.noise]
websocket_mode = "disabled"   # "disabled", "preferred", "required"
stream_mode = "disabled"      # "disabled", "preferred", "required"
```

[security.storage]` controls the local redb at-rest encryption policy. The
first implementation uses PKI-managed node-local storage keys when
`key_source = "pki_local"`.

`[security.noise]` is transport-policy scaffolding for future hardening. It
does not yet wrap the main WebSocket or streaming channels in Noise transport,
but it provides the config surface for `ws-over-noise` / `stream-over-noise`
enforcement modes.

## Public Web Bootstrap

The browser app is delivered from:

- `/`
- `/static/app.css`
- `/static/app.js`

The embedded page is a bootstrap shell, not the old public admin SPA. It
supports importing a client certificate/key bundle into browser storage and
fetching health / CA material from the public listener. Sensitive runtime APIs
belong on the authenticated WebSocket control plane instead of public REST.

---

## `[credentials]`

```toml
[credentials]
enabled = true                                   # Enable vault (default: true)
vault_path = "~/.local/share/rockbot/credentials" # Vault directory
unlock_method = "password"                        # "password", "env", "keyring"
password_env_var = "ROCKBOT_VAULT_PASSWORD"       # Env var for auto-unlock
auto_lock_timeout = 0                             # Auto-lock seconds (0 = never)
default_permission = "deny"                       # Default permission for unspecified paths
```

---

## `[providers]`

### Anthropic

```toml
[providers.anthropic]
enabled = true
auth_mode = "auto"           # "auto", "api", or "oauth"
api_url = "https://api.anthropic.com"  # API endpoint override (optional)
```

### OpenAI

```toml
[providers.openai]
enabled = true
api_url = "https://api.openai.com"  # Endpoint override (optional, for Azure etc.)
```

### AWS Bedrock

```toml
[providers.bedrock]
enabled = true
region = "us-east-1"
auth_mode = "aws_credentials"  # "aws_credentials" or "agentcore"
credential_provider_name = "my-agentcore-provider" # optional
agentcore_auth_flow = "client_credentials"         # optional
agentcore_scopes = "scope1 scope2"                 # optional
credentials_secret_arn = "arn:aws:secretsmanager:..." # optional
```

### Ollama

```toml
[providers.ollama]
enabled = true
url = "http://localhost:11434"
```

---

## `[overseer]`

Requires the `overseer` feature flag.

```toml
[overseer]
model_repo = "bartowski/Qwen2.5-0.5B-Instruct-GGUF"
model_file = "Qwen2.5-0.5B-Instruct-Q4_K_M.gguf"
tokenizer_repo = "Qwen/Qwen2.5-0.5B-Instruct"
```

---

## `[doctor]`

Requires the `doctor-ai` feature flag. Controls the embedded AI model used by
`rockbot doctor` to diagnose and repair config errors at startup or on demand.

```toml
[doctor]
model_id       = "Qwen/Qwen2.5-1.5B-Instruct-GGUF"   # HuggingFace repo for auto-download
model_filename = "qwen2.5-1.5b-instruct-q4_k_m.gguf"  # Filename within the repo
tokenizer_repo = ""                                     # HF repo for tokenizer (defaults to model_id)
model_path     = ""                                     # Absolute path to local GGUF file (overrides download)
tokenizer_path = ""                                     # Absolute path to local tokenizer.json (overrides download)
max_tokens     = 512                                    # Max tokens generated per diagnosis (default: 512)
temperature    = 0.05                                   # Sampling temperature (default: 0.05)
top_p          = 0.9                                    # Top-p nucleus sampling (default: 0.9)
repeat_penalty = 1.1                                    # Repetition penalty (default: 1.1)
seed           = 42                                     # RNG seed for reproducible output (default: 42)
auto_fix       = false                                  # Automatically apply repairs without prompting (default: false)
```

When `model_path` is set it takes precedence over `model_id`/`model_filename`.
Similarly, `tokenizer_path` overrides `tokenizer_repo`. Set `auto_fix = true`
to let the doctor apply TOML repairs in-place; leave it `false` (default) to
review proposed changes before applying.

---

## `[deploy]`

Requires the `bedrock-deploy` feature flag. Configures S3-based CA certificate
distribution and Route53 DNS auto-provisioning.

```toml
[deploy]
bucket = "my-rockbot-ca"              # S3 bucket name (required)
region = "us-east-1"                  # AWS region (default: "us-east-1")
ca_cert_key = "pki/ca.crt"           # S3 object key (default: "pki/ca.crt")
public = false                        # Apply public-read bucket policy (default: false)
endpoint_url = "http://localhost:4566" # S3 endpoint override (e.g. LocalStack)
auto_create_bucket = true             # Auto-create bucket if missing (default: true)
upload_on_startup = true              # Upload CA cert on gateway start (default: true)
dns_zone = "rockbot.internal"         # Route53 hosted zone domain (default: "rockbot.internal")
cluster_name = "prod-east"            # Human-friendly cluster name for DNS (optional)
```

The CA certificate is uploaded to S3 and a CNAME record is created in Route53
pointing to the bucket. This allows clients to fetch the CA cert for mTLS trust
verification without needing a running gateway.

Build with: `cargo build --features bedrock-deploy`

CLI command: `rockbot cert ca publish` — provisions S3 + DNS interactively.

---

## Environment Variable Expansion

```toml
[providers.anthropic]
api_url = "${ANTHROPIC_URL:https://api.anthropic.com}"

[credentials]
password_env_var = "${VAULT_ENV:ROCKBOT_VAULT_PASSWORD}"
```

Syntax: `${VAR}` (required) or `${VAR:default}` (with fallback).

## `[tui]`

```toml
[tui]
floating_bar = true         # Show top bar as floating overlay (default: true)
animations = true           # Enable animated transitions and effects (default: true)
color_theme = "Purple"      # Color theme: Purple, Blue, Green, Rose, Amber, Mono
animation_style = "Coalesce" # Animation style: Coalesce, Fade, Slide, None

[tui.theme]
border = { r = 147, g = 112, b = 219, a = 255 }
text_primary = { r = 245, g = 240, b = 255, a = 255 }
text_secondary = { r = 165, g = 155, b = 185, a = 255 }
ai_text_color = { r = 235, g = 222, b = 255, a = 255 }
thinking_text_color = { r = 191, g = 169, b = 239, a = 255 }
tool_text_color = { r = 255, g = 214, b = 153, a = 255 }
accent_primary = { r = 147, g = 112, b = 219, a = 255 }
accent_secondary = { r = 186, g = 85, b = 211, a = 255 }
accent_tertiary = { r = 218, g = 112, b = 214, a = 255 }
graph_primary = { r = 190, g = 140, b = 255, a = 255 }
graph_secondary = { r = 120, g = 215, b = 255, a = 255 }
bg_primary = { r = 14, g = 10, b = 22, a = 255 }
bg_secondary = { r = 24, g = 18, b = 38, a = 255 }
bg_overlay = { r = 10, g = 8, b = 18, a = 220 }

[tui.fonts]
interface_font_family = "terminal-default"
interface_font_size = 14
user_font_family = "terminal-default"
user_font_size = 14
ai_font_family = "terminal-default"
ai_font_size = 14
thinking_font_family = "terminal-default"
thinking_font_size = 14
tool_font_family = "terminal-default"
tool_font_size = 14
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `floating_bar` | bool | `true` | Render the top navigation bar as a floating overlay above page content |
| `animations` | bool | `true` | Enable tachyonfx-powered transitions (modal open/close, page transitions, glow) |
| `color_theme` | string | `"Purple"` | Preset palette source. Options: `Purple`, `Blue`, `Green`, `Rose`, `Amber`, `Mono` |
| `theme` | table | preset-derived | Optional per-token color overrides for borders, text, accents, graphs, and backgrounds |
| `animation_style` | string | `"Coalesce"` | Modal transition style. Options: `Coalesce`, `Fade`, `Slide`, `None` |
| `fonts` | table | terminal-default / 14 | Stored font preferences for richer renderers such as the Web UI; the terminal TUI persists these but cannot force terminal fonts |

Settings changed in the TUI settings overlay are autosaved back into these
sections. In the terminal UI, alpha is stored exactly and used approximately
for overlay dimming because terminals do not support true per-cell alpha
blending.

### `[tui.theme]`

Each color token is an RGBA object:

```toml
accent_primary = { r = 147, g = 112, b = 219, a = 255 }
```

Available tokens:

- `border`
- `text_primary`
- `text_secondary`
- `ai_text_color`
- `thinking_text_color`
- `tool_text_color`
- `accent_primary`
- `accent_secondary`
- `accent_tertiary`
- `graph_primary`
- `graph_secondary`
- `bg_primary`
- `bg_secondary`
- `bg_overlay`

When `[tui.theme]` is omitted, RockBot derives these values from `color_theme`.
The settings overlay can edit these tokens live with a mouse-enabled
wheel-style picker and writes the results back to `rockbot.toml`.

### `[tui.fonts]`

These preferences are persisted now so future richer clients can apply them
directly. The terminal TUI stores them but does not control the terminal
emulator’s actual font rendering. The settings overlay lets you choose stored
font families and sizes per interface, user, AI, thinking, and tool text role.

## `[seed_model]`

Shared local GGUF model definition used by Butler, Doctor, and Overseer.

This section provides shared defaults for local-model features so you do not
need to repeat the same model coordinates under `[doctor]`, `[overseer]`, and
future local-model consumers.

```toml
[seed_model]
model_id = "Qwen/Qwen2.5-1.5B-Instruct-GGUF"
model_filename = "qwen2.5-1.5b-instruct-q4_k_m.gguf"
tokenizer_repo = "Qwen/Qwen2.5-1.5B-Instruct"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_id` | string | `"Qwen/Qwen2.5-1.5B-Instruct-GGUF"` | HuggingFace model repo ID |
| `model_filename` | string | `"qwen2.5-1.5b-instruct-q4_k_m.gguf"` | GGUF filename within the repo |
| `tokenizer_repo` | string | `"Qwen/Qwen2.5-1.5B-Instruct"` | HuggingFace repo ID for the tokenizer |

---

## CLI Commands

```bash
rockbot config show       # Display current configuration
rockbot config validate   # Validate configuration file
rockbot config init       # Generate default configuration
```
