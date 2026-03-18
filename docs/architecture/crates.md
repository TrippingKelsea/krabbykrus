# Crate Structure

RockBot is a Cargo workspace with 33 crates organized by responsibility.

## Workspace Layout

```
rockbot/
├── crates/
│   ├── rockbot/                   # Binary entry point and top-level feature profiles
│   ├── rockbot-cli/               # Clap CLI surface and command dispatch
│   ├── rockbot-config/            # Config types, message types, shared errors
│   ├── rockbot-session/           # Session management and persistence
│   ├── rockbot-agent/             # Agent execution engine
│   ├── rockbot-client/            # Gateway client, ACP, remote exec
│   ├── rockbot-gateway/           # HTTP/WS server, A2A, cron, routing
│   ├── rockbot-ui-model/          # Shared UI-facing view models and semantic tokens
│   ├── rockbot-webui/             # Leptos-rendered web bootstrap shell + embedded assets
│   ├── rockbot-tui/               # Terminal UI application
│   ├── rockbot-chat/              # Shared chat primitives/UI-facing types
│   ├── rockbot-editor/            # Text editor helpers for interactive clients
│   ├── rockbot-shell/             # Shell/session helpers for interactive clients
│   ├── rockbot-llm/               # LLM provider abstraction
│   ├── rockbot-tools/             # Tool trait, registry, core framework tools
│   ├── rockbot-tools-system/      # System-facing tools (read/write/exec/browser/etc.)
│   ├── rockbot-tools-credentials/ # Credential vault access tool
│   ├── rockbot-tools-mcp/         # MCP server connection tool
│   ├── rockbot-tools-markdown/    # Markdown processing tool
│   ├── rockbot-channels/          # Channel traits and registry
│   ├── rockbot-channels-discord/  # Discord (Serenity)
│   ├── rockbot-channels-telegram/ # Telegram (Teloxide)
│   ├── rockbot-channels-signal/   # Signal channel scaffold
│   ├── rockbot-memory/            # Memory and search system
│   ├── rockbot-security/          # Capability system and sandboxing
│   ├── rockbot-storage/             # Unified embedded storage (redb + optional OpenRaft)
│   ├── rockbot-credentials/       # Encrypted credential vault
│   ├── rockbot-credentials-schema/ # Shared credential schema types
│   ├── rockbot-pki/               # PKI: CA, client certs, CRL, enrollment
│   ├── rockbot-overseer/          # Embedded local-model oversight
│   ├── rockbot-doctor/            # Config diagnostics and auto-repair
│   ├── rockbot-butler/            # Optional companion agent
│   ├── rockbot-deploy/            # S3 CA distribution + Route53 DNS
│   └── rockbot-plugins/           # Plugin system scaffold
```

## Dependency Graph

The crate hierarchy follows a strict DAG — no cycles.

```
rockbot-config             (leaf: config, message, error types)
rockbot-credentials-schema (leaf: shared schema types)
rockbot-ui-model           (leaf: shared UI-facing view models)
rockbot-webui              → rockbot-ui-model
rockbot-chat               (leaf: shared chat types)
rockbot-editor             (leaf-ish: editor helpers)
rockbot-shell              (leaf-ish: shell helpers)

rockbot-session            → rockbot-config
rockbot-security           → (standalone)
rockbot-memory             → (standalone)
rockbot-storage              → (standalone: redb, chacha20; optional: openraft)
rockbot-credentials        → rockbot-storage, rockbot-security

rockbot-llm                → rockbot-credentials-schema
rockbot-tools              → rockbot-security, rockbot-credentials-schema
rockbot-tools-system       → rockbot-tools
rockbot-channels           → rockbot-credentials-schema

rockbot-agent              → rockbot-config, rockbot-session, rockbot-llm,
                              rockbot-tools, rockbot-tools-system,
                              rockbot-memory, rockbot-security
                              [optional: rockbot-client for remote-exec]

rockbot-client             → rockbot-config
                              [optional: snow for remote-exec]
rockbot-pki                → rcgen, x509-parser, rustls, ring, chrono
rockbot-doctor             → rockbot-overseer, rockbot-config
rockbot-butler             → rockbot-overseer, rockbot-config
rockbot-deploy             → rockbot-pki, rockbot-config, rockbot-credentials

rockbot-gateway            → rockbot-config, rockbot-session, rockbot-agent,
                              rockbot-webui, rockbot-client, rockbot-llm,
                              rockbot-tools, rockbot-tools-system,
                              rockbot-channels, rockbot-credentials, rockbot-pki

rockbot-tui                → rockbot-config, rockbot-agent, rockbot-gateway,
                              rockbot-client, rockbot-credentials,
                              rockbot-tools-system,
                              rockbot-chat, rockbot-editor, rockbot-shell
rockbot-cli                → rockbot-config, rockbot-agent, rockbot-gateway,
                              rockbot-session, rockbot-client, rockbot-pki,
                              rockbot-tools-system, rockbot-tui
rockbot                    → rockbot-cli
```

## Feature Flags

Features propagate from the binary through the crate chain. Only enabled
features are compiled.

### LLM Providers

| Feature | Default | Description |
|---------|---------|-------------|
| `bedrock` | yes | AWS Bedrock (Claude, Titan, etc.) |
| `anthropic` | no | Anthropic API direct |
| `openai` | no | OpenAI API |
| `ollama` | no | Local Ollama models |
| `all-providers` | no | Enable all of the above |

### Channels

| Feature | Default | Description |
|---------|---------|-------------|
| `discord` | yes | Discord via Serenity |
| `telegram` | yes | Telegram via Teloxide |
| `signal` | yes | Signal (placeholder) |
| `all-channels` | no | Enable all |

### Tools

| Feature | Default | Description |
|---------|---------|-------------|
| `tools-credentials` | yes | Credential vault access |
| `tools-mcp` | yes | MCP server proxy |
| `tools-markdown` | yes | Markdown processing |
| `all-tools` | no | Enable all |

### Security and Infrastructure

| Feature | Default | Description |
|---------|---------|-------------|
| `noise` | no | Noise handshake and transport primitives |
| `remote-exec` | no | Remote tool dispatch built on the Noise transport |
| `overseer` | no | Embedded local-model agent oversight |
| `doctor-ai` | no | AI-powered config diagnostics and auto-repair |
| `otel` | no | OpenTelemetry trace/metric export |
| `bedrock-deploy` | no | S3 CA distribution + Route53 DNS provisioning |
| `http-insecure` | no | Allow plain HTTP/WS (TLS is default) |
| `vault-replication` | no | OpenRaft-based vault replication across nodes |

### Meta Profiles

Additive feature profiles for common build configurations:

| Profile | Includes | Description |
|---------|----------|-------------|
| `conservative` | default features | Stable production build |
| `enhanced` | conservative + overseer, doctor-ai, vault-replication | Production with AI oversight and HA |
| `experimental` | enhanced + otel, bedrock-deploy | Full feature set for dev/staging |
| `enshitify` | discord | Discord channel support |

### Build Examples

```bash
# Default (conservative: Bedrock + all channels + all tools)
cargo build --release

# Enhanced profile (adds overseer, doctor-ai, vault-replication)
cargo build --release --features enhanced --no-default-features

# Experimental profile (everything)
cargo build --release --features experimental --no-default-features

# Conservative + cherry-pick extras
cargo build --release --features "conservative,otel"

# Anthropic-only, no channels
cargo build --release --no-default-features -F anthropic

# Everything à la carte
cargo build --release -F all-providers,all-channels,all-tools,remote-exec,overseer,otel

# Minimal size
cargo build --profile release-small --no-default-features -F anthropic
```

## Key Modules by Crate

### rockbot-cli
- `lib.rs` — clap command tree, feature passthrough, logging/bootstrap
- `commands/gateway.rs` — gateway service and foreground server commands
- `commands/cert.rs` — CA, client cert, enrollment, verification commands
- `commands/credentials.rs` — vault lifecycle, CRUD, permissions, audit, standalone credentials UI
- `commands/doctor.rs` — diagnostics, repair, and Doctor TUI entry points

### rockbot-tui
- `app.rs` — main terminal app, update loop, gateway integration
- `state.rs` — application state and view models
- `components/` — page renderers, overlays, cards, modals
- `credentials.rs` — standalone credential-management TUI used by `rockbot credentials ui`
- `effects.rs` — visual effects, palette, animation helpers

### rockbot-gateway
- `gateway.rs` — HTTP/WS server, agent lifecycle, TLS listener
- `routing.rs` — Multi-agent routing engine
- `a2a.rs` — Agent-to-Agent protocol (JSON-RPC)
- `cron.rs` — Cron scheduler with redb persistence
- `slash_commands.rs` — Gateway-level slash command dispatch
- `error.rs` — `RockBotError` aggregator

### rockbot-agent
- `agent.rs` — Agent execution loop, tool calls, streaming
- `hooks.rs` — Hook system (pre/post message, tool calls)
- `guardrails.rs` — PII detection, prompt injection guard
- `trajectory.rs` — Full execution trajectory recording
- `orchestration.rs` — Swarm blackboard, workflow executor
- `skills.rs` — Skill manager, slash commands, SKILL.md parsing
- `tokenizer.rs` — BPE token counting (tiktoken)
- `indexer.rs` — Codebase symbol extraction, TF-IDF
- `sandbox.rs` — Docker container sandbox

### rockbot-client
- `client.rs` — `GatewayClient` WS connection with protocol probing
- `acp.rs` — Agent Client Protocol (JSON-RPC over stdio)
- `remote_exec.rs` — Noise Protocol remote tool execution

### rockbot-pki
- `backend.rs` — `KeyBackend` trait, `FileBackend`, `KeyHandle`
- `ca.rs` — CA generation, client cert signing, CSR signing/generation, CRL
- `index.rs` — `PkiIndex`, `CertEntry`, `CertRole`, `CertStatus`, `EnrollmentToken`
- `manager.rs` — `PkiManager` orchestrator, enrollment tokens

### rockbot-storage
- `lib.rs` — `Store` struct wrapping redb: `put`/`get`/`delete`/`list`/`range` + KV convenience methods
- `encrypted_backend.rs` — `redb::StorageBackend` impl with ChaCha20 stream encryption
- `tables.rs` — All 10 table definitions (endpoints, credentials, permissions, KV, sessions, cron, routing, PKI)
- `sync.rs` — Per-table `SyncPolicy` (Eager, Eventual, LocalOnly)
- `raft/` — OpenRaft integration (feature-gated: `replication`): log store, state machine, network

### rockbot-doctor
- `diagnosis.rs` — AI-driven config error analysis, human-readable explanations
- `repair.rs` — Automatic TOML config repair with `toml_edit` (structure-preserving)
- `migration.rs` — Detection and rewriting of deprecated/renamed config fields
- `prompts.rs` — Prompt templates for GGUF model inference
- `learned.rs` — Self-learning fix store (JSONL), SHA-256 fingerprinting, few-shot recall

### rockbot-butler
- `lib.rs` — `Butler` struct, `ButlerConfig`, `ButlerSession`, chat(), init()
- `commands.rs` — `/butler` slash command dispatch (status, mood, help)
- Uses shared `SeedModelConfig` for GGUF model coordinates
- Feature-gated: `butler` in enhanced profile

### Support crates
- `rockbot-chat` — shared chat/session presentation types
- `rockbot-editor` — editor abstractions for interactive clients
- `rockbot-shell` — shell/session helpers for terminal integrations

### rockbot-config
- `config.rs` — `Config`, `GatewayConfig`, `AgentInstance`, `SeedModelConfig`, feature types
- `message.rs` — `Message`, `MessageContent`, `ContentPart`
- `error.rs` — `ConfigError` sub-enum
