# RockBot Status

**Version:** 0.2.3
**Last Updated:** 2026-03-15

## Build Status

- `cargo check` — Compiles with all feature combinations:
  - Default features (`bedrock`, `discord`, `telegram`, `signal`, `tools-*`)
  - `--no-default-features`
  - `--features all-providers,all-channels,all-tools`
- `cargo test` — 334 tests, all passing
- `cargo clippy` — workspace lint configuration enforced:
  - Zero code quality warnings (redundant closures, derivable impls, unused imports, etc.)
  - `unwrap_used`/`expect_used`/`panic` tracked at warn level (test modules allowed)

## Codebase Overview

- **27 crates** in workspace (19 focused crates after decomposition)
- **80 Rust source files**, ~51,000 LOC
- Minimum Rust version: 1.75

| Crate | LOC | Purpose |
|-------|-----|---------|
| `rockbot-core` | 19,601 | Gateway server, agent engine, sessions, config, orchestration |
| `rockbot-cli` | 14,841 | TUI, CLI commands, gateway startup |
| `rockbot-credentials` | 4,472 | Encrypted credential vault, permissions, audit |
| `rockbot-llm` | 4,105 | LLM provider trait, Anthropic/OpenAI/Bedrock/Ollama |
| `rockbot-tools` | 3,854 | Tool trait, registry, 14 built-in tools |
| `rockbot-memory` | 1,162 | Memory manager, keyword + embedding search |
| `rockbot-channels-discord` | 599 | Discord channel (Serenity) |
| `rockbot-tools-mcp` | 576 | MCP server connection tool |
| `rockbot-channels` | 458 | Channel trait, registry, manager |
| `rockbot-security` | 453 | Capability system, security contexts, sandbox |
| `rockbot-channels-telegram` | 446 | Telegram channel (Teloxide) |
| `rockbot-plugins` | 185 | Plugin manager (scaffold) |
| `rockbot-channels-signal` | 148 | Signal channel (placeholder) |
| `rockbot-tools-markdown` | 90 | Markdown processing tool |
| `rockbot-tools-credentials` | 83 | Credential vault access tool |
| `rockbot-credentials-schema` | 64 | Shared credential schema types |
| `rockbot` | 10 | Binary entry point |

---

## Architecture

### Plugin System

All providers (LLM, Channel, Tool) are **self-contained plugins** that register their own credential schemas. The gateway dynamically collects schemas from three registries at startup:

```
rockbot-credentials-schema (leaf crate, only serde)
    ^                ^               ^
rockbot-llm    rockbot-channels   rockbot-tools     <- trait + registry crates
    ^                ^               ^
  bedrock    channels-discord   tools-mcp           <- per-provider crates (optional deps)
  anthropic  channels-telegram  tools-credentials
  openai     channels-signal    tools-markdown
  ollama
```

**No cyclic dependencies.** Trait crates define interfaces. Per-provider crates implement them. Registration happens in `Gateway::new()` via `#[cfg(feature = "...")]` guards.

### Feature Flags

| Flag | Default | Crate |
|------|---------|-------|
| `bedrock` | Yes | AWS Bedrock via Converse API |
| `anthropic` | No | Claude via Claude Code SDK (OAuth) |
| `openai` | No | OpenAI models |
| `ollama` | No | Ollama local models |
| `discord` | Yes | Discord channel |
| `telegram` | Yes | Telegram channel |
| `signal` | Yes | Signal channel (placeholder) |
| `tools-credentials` | Yes | Credential vault tool |
| `tools-mcp` | Yes | MCP server tool |
| `tools-markdown` | Yes | Markdown processing tool |
| `otel` | No | OpenTelemetry export |

Feature passthrough: `rockbot` -> `rockbot-cli`/`rockbot-core` -> per-crate.

### Gateway-Centric Design

The gateway is the **single source of truth** for all runtime state. TUI, WebUI, and CLI are presentation layers that query the gateway API.

- Providers: loaded at startup, queryable via `/api/providers`
- Agents: owned by gateway, persisted to TOML config + per-agent directories
- Credentials: managed via vault, exposed through `/api/credentials/*`
- Schemas: dynamically collected from all registered plugins

---

## What's Working

### Gateway Server (`rockbot-core`)

- **HTTP API** (hyper-based) with 30+ endpoints
- **WebSocket** real-time streaming (hyper upgrade + tokio-tungstenite)
- **Agent CRUD** — create, update, delete, list with full config persistence
- **Agent execution** — message processing, multi-tool calling, retry with exponential backoff
- **Session management** — SQLite-backed, CRUD, message history, token tracking
- **Config system** — TOML parsing, env var expansion, hot-reload watcher
- **Credential management** — vault integration, auto-unlock, full CRUD API
- **Dynamic provider registration** — LLM, Channel, and Tool schemas collected at startup
- **Clippy lint configuration** — workspace-level lint rules with `[workspace.lints.clippy]`
- **Agent directory management** — per-agent context files (SOUL.md, AGENTS.md, MEMORY.md)
- **Web UI** — embedded HTML SPA with cyberpunk theme, 6 navigation sections
- **Cron scheduler** — job CRUD, scheduling, per-agent bindings, enable/disable/trigger
- **A2A protocol** — `/.well-known/agent.json`, JSON-RPC task dispatch
- **ACP protocol** — JSON-RPC over stdio for IDE integration

### Agent Engine (`rockbot-core/agent.rs`)

- System prompt assembly from SOUL.md, AGENTS.md, skills section, episodic memory
- Tool execution loop with dynamic iteration limits (32-160 based on available tools)
- **Streaming responses** — real-time text deltas + reasoning content via WebSocket
- Tool loop detection with warn/critical/circuit-breaker thresholds
- Semantic context compaction via LLM (not naive truncation)
- Continuation nudges with 2-level escalation for stalled agents
- `<think>` reasoning block support — streamed in real-time, stripped from final output
- Plan-and-execute mode (never/auto/always/approval_required)
- Reflection loop — post-tool-loop self-critique pass
- Configurable temperature, max_tokens, max_context_tokens per agent
- LLM retry with exponential backoff, jitter, and error classification
- Token usage tracking (tiktoken BPE for accurate counting)
- Trajectory recording (13 event types, JSONL export)
- Parallel guardrails (PII detection, prompt injection)
- Subagent delegation via `invoke_agent` tool (max depth 3)
- Agent-as-tool (`expose_as_tool` config)
- HIL breakpoints per tool

### LLM Providers (`rockbot-llm`)

| Provider | Status | Auth | Streaming |
|----------|--------|------|-----------|
| AWS Bedrock | Working | AWS credentials (env/profile) | Converse Stream API (text + reasoning + tool calls) |
| Anthropic | Working | Claude Code OAuth or API key | SSE |
| OpenAI | Working | API key | SSE |
| Ollama | Working | None (local) | SSE |
| Mock | Testing | None | N/A |

- Provider registry with model routing (`get_provider_for_model`)
- Credential schemas self-registered per provider
- Chat completion request/response types with tool calling, images, structured output

### Channels

| Channel | Status | Notes |
|---------|--------|-------|
| Discord | Implemented | Serenity-based, embeds, events, self-registering schema |
| Telegram | Implemented | Teloxide-based, self-registering schema |
| Signal | Placeholder | Schema registered, `connect()` returns not-yet-implemented |

### Tools

| Tool | Category | Notes |
|------|----------|-------|
| `read` | Standard | File reading with offset/limit |
| `write` | Standard | File writing |
| `edit` | Standard | Text editing with fuzzy match fallback |
| `exec` | Standard | Shell execution with sandbox |
| `glob` | Standard | File pattern matching |
| `grep` | Standard | Content searching |
| `patch` | Standard | Diff application |
| `invoke_agent` | Standard | Subagent delegation |
| `web_fetch` | Standard | HTTP GET + HTML strip |
| `web_search` | Standard | Brave Search API |
| `test` | Standard | Auto-detect language, run tests |
| `lint` | Standard | Auto-detect language, run linter |
| `clarify` | Standard | Ask user for clarification |
| `memory_get` | Full | Memory retrieval |
| `memory_search` | Full | Memory keyword search |
| `browser` | Full | Headless Chrome + HTTP fallback |
| `handoff` | Full | Transfer control to another agent |
| `blackboard_read` | Full | Swarm shared state read |
| `blackboard_write` | Full | Swarm shared state write |

### Credentials (`rockbot-credentials`)

- AES-256-GCM encryption at rest
- Master key derivation via Argon2id
- Multiple unlock methods: password, keyfile, Age, SSH key
- 4-tier permission levels: Allow, AllowHIL, AllowHIL2FA, Deny
- Glob pattern matching for path-based permissions
- HIL (Human-in-the-Loop) approval queue
- Hash-chained tamper-evident audit log
- Full HTTP API (15 endpoints)

### TUI (`rockbot-cli`)

| Section | Status | Notes |
|---------|--------|-------|
| Dashboard | Complete | Gateway status, agent summary cards, vault status |
| Credentials | Complete | 4 sub-tabs (Endpoints/Providers/Permissions/Audit), full CRUD |
| Agents | Complete | List, create, edit, context file editor |
| Sessions | Complete | Session list, real-time chat with streaming, tool call display |
| Cron Jobs | Complete | Job list, filtering, enable/disable/delete/trigger |
| Models | Complete | Dynamic provider list from gateway |
| Settings | Partial | Gateway control (start/stop/restart) |

- Elm-like architecture: State -> Message -> Update -> View
- Compact scrollable menu + card strip layout
- WebSocket streaming for real-time agent responses
- Real-time thinking/reasoning text display during model inference
- Token usage + tokens/sec display during streaming
- Keyboard shortcuts: Ctrl+Q quit, Ctrl+J newline in chat, 1-7 quick nav
- Falls back to HTTP when WebSocket unavailable

### Web UI (`rockbot-core/web_ui.rs`)

- Embedded HTML SPA served from gateway (no external deps, vanilla JS)
- 6 navigation sections at parity with TUI
- Cyberpunk dark theme with CSS custom properties
- Full credential management with schema-driven config modals
- Real-time chat with streaming, thinking indicator, token stats

### Multi-Agent Orchestration

- **Handoffs** — transfer conversation control between agents
- **Swarms** — coordinated specialist teams with shared blackboard state
- **Graph Workflows** — declarative DAG execution with parallel fan-out, conditional edges

---

## HTTP API Reference

### Gateway
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health`, `/api/status` | Health check / gateway status |
| GET | `/api/gateway/pending` | List pending agents |
| POST | `/api/gateway/reload` | Reload gateway config |
| GET | `/ws` | WebSocket upgrade for real-time streaming |

### Agents
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/agents` | List all agents |
| POST | `/api/agents` | Create agent |
| PUT | `/api/agents/:id` | Update agent |
| DELETE | `/api/agents/:id` | Delete agent |
| POST | `/api/agents/:id/message` | Send message to agent |
| POST | `/api/agents/:id/stream` | SSE streaming response |
| GET | `/api/agents/:id/files` | List context files |
| GET | `/api/agents/:id/files/:name` | Get context file |
| PUT | `/api/agents/:id/files/:name` | Update context file |
| DELETE | `/api/agents/:id/files/:name` | Delete context file |

### Sessions
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/agents/:id/sessions` | List sessions |
| POST | `/api/agents/:id/sessions` | Create session |
| DELETE | `/api/agents/:id/sessions/:sid` | Delete session |
| GET | `/api/agents/:id/sessions/:sid/messages` | Get messages |
| GET | `/api/agents/:id/sessions/:sid/export` | Export session |

### Cron
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/cron/jobs` | List cron jobs |
| POST | `/api/cron/jobs` | Create cron job |
| PUT | `/api/cron/jobs/:id` | Update cron job |
| DELETE | `/api/cron/jobs/:id` | Delete cron job |
| POST | `/api/cron/jobs/:id/trigger` | Trigger job now |

### Providers
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/providers` | List registered LLM providers |
| GET | `/api/providers/:id` | Get provider details |
| POST | `/api/providers/:id/test` | Test provider connectivity |
| POST | `/api/chat` | Route chat completion through gateway |
| GET | `/api/credentials/schemas` | Dynamic credential schemas from all plugins |

### Credentials
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/credentials/status` | Vault status |
| GET | `/api/credentials[/endpoints]` | List endpoints |
| POST | `/api/credentials[/endpoints]` | Create endpoint |
| DELETE | `/api/credentials[/endpoints]/:id` | Delete endpoint |
| POST | `/api/credentials/endpoints/:id/credential` | Store credential |
| POST | `/api/credentials/init` | Initialize vault |
| POST | `/api/credentials/unlock` | Unlock vault |
| POST | `/api/credentials/lock` | Lock vault |
| GET | `/api/credentials/permissions` | List permission rules |
| POST | `/api/credentials/permissions` | Add permission rule |
| DELETE | `/api/credentials/permissions/:id` | Remove permission rule |
| GET | `/api/credentials/audit` | View audit log |
| GET | `/api/credentials/approvals` | List pending HIL approvals |
| POST | `/api/credentials/approvals/:id/approve` | Approve HIL request |
| POST | `/api/credentials/approvals/:id/deny` | Deny HIL request |

### A2A Protocol
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/.well-known/agent.json` | Agent card discovery |
| POST | `/a2a` | JSON-RPC task dispatch |

### Metrics
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/metrics` | Prometheus-style metrics |

---

## Known Issues

- `aws_smithy_types::Document` doesn't impl `Serialize` — manual converters in `bedrock.rs`
- Gateway uptime tracking returns 0 (TODO)
- Memory usage reporting returns 0 (TODO)
- SSH agent vault unlock not yet implemented
- Shift+Enter for newline requires Kitty keyboard protocol — use Ctrl+J as universal fallback

---

## Configuration Example

```toml
[gateway]
bind_host = "127.0.0.1"
port = 18080

[agents.defaults]
model = "bedrock/moonshotai.kimi-k2.5"
workspace = "~/.config/rockbot/agents"

[[agents.list]]
id = "main"
model = "bedrock/moonshotai.kimi-k2.5"

[[agents.list]]
id = "researcher"
model = "bedrock/anthropic.claude-sonnet-4-20250514-v1:0"
parent_id = "main"

[tools]
profile = "standard"

[security.sandbox]
mode = "tools"
scope = "session"

[credentials]
enabled = true
vault_path = "~/.config/rockbot/vault"
unlock_method = "env"
password_env_var = "ROCKBOT_VAULT_PASSWORD"
default_permission = "deny"

[providers.bedrock]
region = "us-east-1"
```

## Running

```bash
# Build (default features)
cargo build

# Build (all providers)
cargo build --features all-providers,all-channels,all-tools

# Run tests
cargo test --features all-providers

# Run gateway
cargo run -- --config ~/.config/rockbot/config.toml gateway run

# TUI
cargo run -- --config ~/.config/rockbot/config.toml tui

# Credential management
cargo run -- credentials status
cargo run -- credentials add homeassistant -t home_assistant -u http://homeassistant:8123
cargo run -- credentials list
```
