# RockBot Feature Matrix

This document tracks feature implementation status and helps identify gaps between planned functionality and current implementation.

**Legend:**
- ✅ Implemented and tested
- 🚧 Partially implemented / in progress
- 📋 Planned / designed but not started
- ❌ Not planned for MVP

---

## Core Framework (`rockbot-core`)

### Gateway Server

| Feature | Status | Notes |
|---------|--------|-------|
| HTTP server (hyper) | ✅ | Async, production-ready |
| Health check endpoint | ✅ | `GET /health` |
| Agent listing | ✅ | `GET /api/agents` |
| Agent messaging | ✅ | `POST /api/agents/{id}/message` |
| WebSocket support | 📋 | Protocol defined, handler needed |
| TLS/HTTPS | 📋 | Via reverse proxy for now |
| Rate limiting | 📋 | |
| Authentication | 📋 | Bearer token planned |

### Configuration

| Feature | Status | Notes |
|---------|--------|-------|
| TOML-based config | ✅ | |
| Environment variable expansion | ✅ | `${VAR}` syntax |
| Hot-reload via file watcher | ✅ | notify crate |
| Config validation | ✅ | |
| Config migration | 📋 | From OpenClaw format |

### Session Management

| Feature | Status | Notes |
|---------|--------|-------|
| SQLite persistence | ✅ | |
| Message history | ✅ | |
| Token usage tracking | ✅ | |
| Session CRUD | ✅ | |
| Session archival | 🚧 | CLI command exists |
| Session export | 📋 | JSON/Markdown export |

### Agent Engine

| Feature | Status | Notes |
|---------|--------|-------|
| Message processing pipeline | ✅ | |
| Tool execution | 🚧 | Registry works, tools are stubs |
| Context management | ✅ | |
| Context compaction | 🚧 | Basic implementation |
| Streaming responses | 📋 | |
| Multi-turn conversation | ✅ | |

---

## Credential Management (`rockbot-credentials`)

### Vault

| Feature | Status | Notes |
|---------|--------|-------|
| AES-256-GCM encryption | ✅ | |
| Argon2id key derivation | ✅ | |
| Password unlock | ✅ | |
| Keyfile unlock | ✅ | |
| Age encryption | 🚧 | Stubbed |
| SSH key unlock | 🚧 | Stubbed |
| Auto-unlock via env var | ✅ | `ROCKBOT_VAULT_PASSWORD` |

### Endpoint Types

| Type | Status | Notes |
|------|--------|-------|
| Home Assistant | ✅ | Long-lived access token |
| Generic REST API | ✅ | Bearer token |
| OAuth2 Service | 🚧 | Token storage works, flow not automated |
| API Key Service | ✅ | Custom header support |
| Basic Auth | ✅ | Username/password |
| Bearer Token | ✅ | Generic bearer |

### Permission System

| Feature | Status | Notes |
|---------|--------|-------|
| Allow | ✅ | Immediate grant |
| AllowHIL (Human-in-Loop) | ✅ | Approval queue |
| AllowHIL2FA | 📋 | YubiKey integration |
| Deny | ✅ | |
| Glob pattern matching | ✅ | `saggyclaw://api/**` |
| Persistent rules | 📋 | Currently in-memory |

### Audit Logging

| Feature | Status | Notes |
|---------|--------|-------|
| Hash-chained log | ✅ | Tamper-evident |
| Operation tracking | ✅ | CRUD + permission changes |
| Verification | ✅ | CLI command |
| Log rotation | 📋 | |
| Log export | 📋 | |

### HTTP API

| Endpoint | Status | Notes |
|----------|--------|-------|
| `GET /api/credentials/status` | ✅ | |
| `GET /api/credentials` | ✅ | List endpoints |
| `POST /api/credentials` | ✅ | Create endpoint |
| `DELETE /api/credentials/:id` | ✅ | |
| `POST .../credential` | ✅ | Store secret |
| `GET /api/credentials/permissions` | ✅ | |
| `POST /api/credentials/permissions` | ✅ | |
| `DELETE /api/credentials/permissions/:id` | ✅ | |
| `GET /api/credentials/audit` | ✅ | |
| `GET /api/credentials/approvals` | ✅ | |
| `POST /api/credentials/approvals/:id/approve` | ✅ | |
| `POST /api/credentials/approvals/:id/deny` | ✅ | |
| `POST /api/credentials/unlock` | ✅ | |
| `POST /api/credentials/lock` | ✅ | |

---

## LLM Providers (`rockbot-llm`)

| Provider | Status | Notes |
|----------|--------|-------|
| Mock provider | ✅ | For testing |
| Anthropic Claude | 📋 | High priority |
| OpenAI | 📋 | |
| Ollama (local) | 📋 | |
| AWS Bedrock | 📋 | |
| Streaming support | 📋 | |
| Retry/backoff | 📋 | |

---

## Tools (`rockbot-tools`)

### Built-in Tools

| Tool | Status | Notes |
|------|--------|-------|
| `read` | 🚧 | Skeleton only |
| `write` | 🚧 | Skeleton only |
| `edit` | 🚧 | Skeleton only |
| `exec` | 🚧 | Skeleton only |
| `web_search` | 📋 | |
| `web_fetch` | 📋 | |
| `browser` | 📋 | |

### Tool System

| Feature | Status | Notes |
|---------|--------|-------|
| Tool registry | ✅ | |
| Profile-based loading | ✅ | minimal/standard/full |
| Capability-based filtering | ✅ | |
| JSON Schema generation | 📋 | For LLM function calling |
| Tool result types | ✅ | |

---

## Security (`rockbot-security`)

| Feature | Status | Notes |
|---------|--------|-------|
| Capability enum | ✅ | FilesystemRead/Write, ProcessExecute, etc. |
| Security context | ✅ | Session-scoped |
| Capability checking | ✅ | |
| Sandbox (container) | 📋 | |
| Sandbox (process) | 📋 | |
| Path canonicalization | 📋 | |
| Command allowlisting | 📋 | |

---

## Memory (`rockbot-memory`)

| Feature | Status | Notes |
|---------|--------|-------|
| Document loading | ✅ | |
| Keyword search | ✅ | |
| Core memory (JSON) | ✅ | |
| Vector index | 📋 | Placeholder |
| Semantic search | 📋 | |
| Memory compaction | 📋 | |

---

## CLI (`rockbot-cli`)

### Commands

| Command | Status | Notes |
|---------|--------|-------|
| `gateway` | ✅ | Start gateway server |
| `config show` | ✅ | |
| `config validate` | ✅ | |
| `config init` | ✅ | |
| `session list` | ✅ | |
| `session show` | ✅ | |
| `session history` | ✅ | |
| `session archive` | 🚧 | |
| `session delete` | ✅ | |
| `agent list` | ✅ | |
| `agent status` | ✅ | |
| `agent message` | ✅ | |
| `agent create` | 🚧 | |
| `tool list` | ✅ | |
| `tool info` | ✅ | |
| `tool test` | 🚧 | |
| `credentials status` | ✅ | |
| `credentials list` | ✅ | |
| `credentials add` | ✅ | |
| `credentials remove` | 🚧 | |
| `credentials unlock` | ✅ | |
| `credentials lock` | ✅ | |
| `credentials permissions` | ✅ | |
| `credentials audit` | ✅ | |
| `doctor` | 🚧 | |
| `migrate` | 📋 | |

### TUI (Terminal UI)

| Feature | Status | Notes |
|---------|--------|-------|
| Async event loop | ✅ | tokio::select! |
| Dashboard view | ✅ | |
| Credentials view | ✅ | |
| Add credential modal | ✅ | Dynamic fields per type |
| Agents view | ✅ | |
| Sessions view | ✅ | |
| Models view | ✅ | |
| Settings view | 🚧 | |
| Vault unlock modal | ✅ | Auto-unlock for keyfile |
| Real data binding | 📋 | Currently mock data |
| Gateway API calls | 📋 | |

---

## Web UI (`rockbot-core::web_ui`)

| Feature | Status | Notes |
|---------|--------|-------|
| Embedded HTML | ✅ | |
| Dashboard | ✅ | |
| Credentials page | ✅ | |
| Add credential form | ✅ | Dynamic fields |
| Agents page | ✅ | |
| Sessions page | ✅ | |
| Models page | ✅ | |
| Settings page | 🚧 | |
| Real-time updates | 📋 | WebSocket needed |

---

## Channels (`rockbot-channels`)

| Channel | Status | Notes |
|---------|--------|-------|
| HTTP/REST | 📋 | |
| WebSocket | 📋 | |
| Discord | 📋 | |
| Telegram | 📋 | |
| Slack | 📋 | |
| IRC | 📋 | |

---

## Plugins (`rockbot-plugins`)

| Feature | Status | Notes |
|---------|--------|-------|
| Plugin trait | ✅ | |
| Plugin registry | 🚧 | |
| WASM runtime | 📋 | |
| Plugin discovery | 📋 | |
| Plugin isolation | 📋 | |

---

## Gap Analysis Summary

### Critical Path to MVP

1. **LLM Provider** - Need at least Anthropic working
2. **Tool Implementations** - read/write/edit/exec need real implementations
3. **Credential Injection** - Tools need to use credentials from vault
4. **Real Data Binding** - TUI/Web UI need to call actual gateway APIs

### Nice to Have (Post-MVP)

1. WebSocket for real-time updates
2. Additional LLM providers (OpenAI, Ollama)
3. Channel integrations (Discord, Telegram)
4. WASM plugin system
5. Sandbox implementation

### Technical Debt

1. Type duplication between crates (needs consolidation)
2. Mock implementations need replacement
3. Error handling inconsistencies
4. Test coverage gaps in some modules
