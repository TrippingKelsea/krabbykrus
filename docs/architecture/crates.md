# Crate Structure

RockBot is a Cargo workspace with 19 crates organized by responsibility.

## Workspace Layout

```
rockbot/
├── crates/
│   ├── rockbot/                  # Binary entry point
│   ├── rockbot-cli/              # CLI commands and TUI
│   ├── rockbot-core/             # Re-export facade (backward compat)
│   ├── rockbot-config/           # Config types, message types, errors
│   ├── rockbot-session/          # Session management and persistence
│   ├── rockbot-agent/            # Agent execution engine
│   ├── rockbot-client/           # Gateway WS client, ACP, remote exec
│   ├── rockbot-gateway/          # HTTP/WS server, A2A, cron, routing
│   ├── rockbot-webui/            # Embedded web dashboard (static HTML)
│   ├── rockbot-llm/              # LLM provider abstraction
│   ├── rockbot-tools/            # Tool trait and registry
│   ├── rockbot-tools-credentials/# Credential vault access tool
│   ├── rockbot-tools-mcp/        # MCP server connection tool
│   ├── rockbot-tools-markdown/   # Markdown processing tool
│   ├── rockbot-channels/         # Channel traits and registry
│   ├── rockbot-channels-discord/ # Discord (Serenity)
│   ├── rockbot-channels-telegram/# Telegram (Teloxide)
│   ├── rockbot-channels-signal/  # Signal (placeholder)
│   ├── rockbot-memory/           # Memory and search system
│   ├── rockbot-security/         # Capability system and sandboxing
│   ├── rockbot-credentials/      # Encrypted credential vault
│   ├── rockbot-credentials-schema/# Shared credential schema types
│   ├── rockbot-overseer/         # Embedded local-model oversight
│   └── rockbot-plugins/          # Plugin system (scaffold)
```

## Dependency Graph

The crate hierarchy follows a strict DAG — no cycles.

```
rockbot-config            (leaf: config, message, error types)
rockbot-credentials-schema (leaf: shared schema types)
rockbot-webui             (leaf: pure static HTML)

rockbot-session           → rockbot-config
rockbot-security          → (standalone)
rockbot-memory            → (standalone)
rockbot-credentials       → rockbot-security

rockbot-llm               → rockbot-credentials-schema
rockbot-tools             → rockbot-security, rockbot-credentials-schema
rockbot-channels          → rockbot-credentials-schema

rockbot-agent             → rockbot-config, rockbot-session, rockbot-llm,
                             rockbot-tools, rockbot-memory, rockbot-security
                             [optional: rockbot-client for remote-exec]

rockbot-client            → rockbot-config
                             [optional: snow for remote-exec]

rockbot-gateway           → rockbot-config, rockbot-session, rockbot-agent,
                             rockbot-webui, rockbot-client, rockbot-llm,
                             rockbot-tools, rockbot-channels, rockbot-credentials
                             [optional: channel/tool provider crates, overseer]

rockbot-core              → facade: re-exports all of the above
rockbot-cli               → rockbot-core, rockbot-client
rockbot                   → rockbot-cli, rockbot-core
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
| `remote-exec` | no | Noise Protocol encrypted remote tool dispatch |
| `overseer` | no | Embedded local-model agent oversight |
| `otel` | no | OpenTelemetry trace/metric export |
| `http-insecure` | no | Allow plain HTTP/WS (TLS is default) |

### Build Examples

```bash
# Default (Bedrock + all channels + all tools)
cargo build --release

# Anthropic-only, no channels
cargo build --release --no-default-features -F anthropic

# Everything
cargo build --release -F all-providers,all-channels,all-tools,remote-exec,overseer,otel

# Remote development setup
cargo build --release -F remote-exec

# Minimal size
cargo build --profile release-small --no-default-features -F anthropic
```

## Key Modules by Crate

### rockbot-gateway
- `gateway.rs` — HTTP/WS server, agent lifecycle, TLS listener
- `routing.rs` — Multi-agent routing engine
- `a2a.rs` — Agent-to-Agent protocol (JSON-RPC)
- `cron.rs` — Cron scheduler with SQLite persistence
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

### rockbot-config
- `config.rs` — `Config`, `GatewayConfig`, `AgentInstance`, feature types
- `message.rs` — `Message`, `MessageContent`, `ContentPart`
- `error.rs` — `ConfigError` sub-enum
