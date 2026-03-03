# RustClaw Status

**Last Updated:** 2026-03-02 12:15

## Build Status

✅ `cargo build` - Compiles successfully  
✅ `cargo test` - All 79 tests pass (53 in rustclaw-credentials)

## What's Working

### Core Components (`rustclaw-core`)
- **Config System** (`config.rs`)
  - TOML-based configuration with validation
  - Environment variable expansion (`${VAR}` syntax)
  - Hot-reloading via file watcher (notify)
  - Type conversions to subcrate types
  - **NEW:** Credentials configuration section

- **Session Manager** (`session.rs`)
  - SQLite-backed session persistence
  - Message history storage and retrieval
  - Session CRUD operations
  - Token usage tracking

- **Message System** (`message.rs`)
  - Rich message types (Text, Rich, ToolResult, System, Error)
  - Message builder pattern
  - Attachments support
  - Full serde serialization

- **Gateway** (`gateway.rs`)
  - HTTP server (hyper-based)
  - Health check endpoint (`/health`)
  - Agent listing endpoint (`/api/agents`)
  - Agent message endpoint (`/api/agents/{id}/message`)
  - WebSocket upgrade placeholder
  - **NEW:** Credential management integration
  - **NEW:** Credentials API endpoints (see below)

- **Agent Engine** (`agent.rs`)
  - Message processing pipeline
  - Tool execution integration
  - Context management and compaction
  - Token tracking and statistics

### LLM Providers (`rustclaw-llm`)
- **Provider Abstraction**
  - Async trait-based provider interface
  - Provider registry with model routing
  - Mock provider for testing
  - Chat completion request/response types

### Tools (`rustclaw-tools`)
- **Tool Registry**
  - Profile-based tool loading (minimal/standard/full)
  - Capability-based tool filtering
  - Tool execution with security context
  - Built-in tools: read, write, edit, exec (skeleton implementations)

### Security (`rustclaw-security`)
- **Capability System**
  - Fine-grained capabilities (FilesystemRead/Write, ProcessExecute, NetworkAccess)
  - Session-scoped security contexts
  - Capability checking and enforcement
  - Sandbox configuration (placeholder)

### Credentials (`rustclaw-credentials`) ✅ INTEGRATED
- **Secure Credential Storage** (ported from SAGgyClaw)
  - AES-256-GCM encryption at rest
  - Master key derivation via Argon2id
  - Hierarchical path-based credential organization (`saggyclaw://endpoint/path`)
  
- **Permission System**
  - Four permission levels: `Allow`, `AllowHIL` (human-in-loop), `AllowHIL2FA`, `Deny`
  - Glob pattern matching for path-based permissions (`saggyclaw://api/**`)
  - Policy evaluation with explicit deny override
  
- **HIL (Human-in-the-Loop) System** ✅ NEW
  - Approval request queue with notifications
  - Configurable timeout for pending approvals
  - Approval/denial flow via HTTP API or programmatic interface
  - Non-blocking permission check (`check_permission()`)
  - Blocking credential request (`request_credential()`) with HIL wait
  - AllowHIL2FA stubbed for future YubiKey integration
  
- **Audit Logging**
  - Hash-chained tamper-evident audit log
  - Operations tracked: Create, Read, Update, Delete, PermissionChange
  - Automatic previous-hash linking for integrity verification
  - Verification CLI command
  
- **Credential Types**
  - API keys, OAuth2 tokens, username/password pairs, certificates, raw secrets
  - Expiration tracking with refresh token support
  - Metadata storage (labels, descriptions)
  
- **High-Level Manager API** (`CredentialManager`)
  - Thread-safe, async-compatible interface for gateway integration
  - Path-based permission policies (`PathPermission`)
  - Automatic permission evaluation before credential access
  - Vault locking/unlocking with master key
  - Clone-friendly (shares state via Arc<RwLock<...>>)
  - HIL notification subscription
  
- **Gateway Integration** ✅ NEW
  - CredentialManager wired into gateway startup
  - Config options: `[credentials]` section with vault_path, unlock_method, etc.
  - Auto-unlock via environment variable (`RUSTCLAW_VAULT_PASSWORD`)
  
- **HTTP API Endpoints** ✅ COMPLETE
  - `GET /api/credentials` or `/api/credentials/endpoints` - List endpoints (no secrets)
  - `POST /api/credentials` or `/api/credentials/endpoints` - Create new endpoint
  - `DELETE /api/credentials/:id` or `/api/credentials/endpoints/:id` - Remove endpoint
  - `POST /api/credentials/endpoints/:id/credential` - Store credential
  - `GET /api/credentials/permissions` - List permission rules
  - `POST /api/credentials/permissions` - Add permission rule
  - `DELETE /api/credentials/permissions/:id` - Remove permission rule
  - `GET /api/credentials/audit?limit=N` - View audit log entries
  - `GET /api/credentials/approvals` - List pending HIL approvals
  - `POST /api/credentials/approvals/:id/approve` - Approve HIL request
  - `POST /api/credentials/approvals/:id/deny` - Deny HIL request
  - `POST /api/credentials/approvals/respond` - Generic approval response
  - `GET /api/credentials/status` - Vault status (enabled, locked, counts)
  - `POST /api/credentials/unlock` - Unlock vault with password
  - `POST /api/credentials/lock` - Lock vault
  
- **CLI Commands** ✅ NEW (`rustclaw credentials ...`)
  - `add` - Add new endpoint with optional secret
  - `list` - List configured endpoints
  - `remove` - Remove endpoint (stubbed)
  - `permissions add/list/remove` - Manage permission rules
  - `audit [--verify]` - View/verify audit log
  - `status` - Show vault status
  - `unlock` - Unlock vault
  - `lock` - Lock vault

### Memory (`rustclaw-memory`)
- **Memory Manager**
  - Document loading and indexing
  - Keyword-based search
  - Core memory (JSON) management
  - Vector index placeholder

### CLI (`rustclaw-cli`)
- **Command Structure**
  - Gateway command
  - Config commands (show, validate, init)
  - Session commands (list, show, history, archive, delete)
  - Agent commands (list, status, message, create)
  - Tool commands (list, info, test)
  - **NEW:** Credentials commands
  - Doctor command
  - Migrate commands

## What Needs Implementation

### High Priority (MVP)

1. **Real LLM Provider (Anthropic)**
   - API client implementation
   - Authentication handling
   - Streaming support
   - Error handling and retries

2. **Built-in Tool Implementations**
   - `read` - File reading with encoding detection
   - `write` - File writing with atomic operations
   - `edit` - Text editing with diff support
   - `exec` - Process execution with sandboxing

3. **Credential-Aware Tool Execution** ⏳ Partially Done
   - ✅ CredentialManager in gateway
   - ✅ `saggyclaw://` URI resolution
   - ⏳ Tool call interception and credential injection
   - ⏳ Response sanitization (strip credentials from tool output)

4. **WebSocket Handler**
   - Protocol implementation
   - Session management
   - Streaming responses
   - HIL notification push

5. **CLI Demo Mode**
   - Interactive REPL
   - Direct agent messaging

### Medium Priority

1. **Channel Integrations**
   - Discord adapter
   - HTTP/WebSocket channels
   - Event routing
   - HIL approval via channel messages

2. **Real Security**
   - Sandbox implementation (container or process-based)
   - Path canonicalization
   - Command allowlisting

3. **Memory Enhancements**
   - Vector embeddings (optional external provider)
   - Semantic search
   - Memory compaction strategies

4. **Persistent Permissions**
   - Save/load permission rules to vault
   - Permission rule management in CLI

### Lower Priority

1. **OpenAI/Ollama Providers**
2. **Plugin System**
3. **Migration Tools** (from OpenClaw)
4. **Metrics/Observability**
5. **AllowHIL2FA with YubiKey**

## Architecture Notes

### Crate Structure
```
rustclaw/
├── crates/
│   ├── rustclaw/          # Binary crate (main entry point)
│   ├── rustclaw-core/     # Core framework (gateway, agent, session, config)
│   ├── rustclaw-llm/      # LLM provider abstraction
│   ├── rustclaw-tools/    # Tool system
│   ├── rustclaw-security/ # Security/capability system
│   ├── rustclaw-credentials/ # Secure credential storage (from SAGgyClaw)
│   ├── rustclaw-memory/   # Memory/search system
│   ├── rustclaw-channels/ # Channel integrations
│   ├── rustclaw-plugins/  # Plugin system
│   └── rustclaw-cli/      # CLI interface
```

### Credentials Flow (End-to-End)
```
Agent Request: "Call Home Assistant API"
    │
    ▼
Tool Call with saggyclaw://homeassistant/api/services/light/turn_on
    │
    ▼
CredentialManager.check_permission("saggyclaw://homeassistant/**")
    │
    ├── Allow → retrieve credential, inject into request
    ├── AllowHIL → create approval request, wait for human
    │               └── Human approves/denies via Web UI or CLI
    └── Deny → return error to agent
    │
    ▼
Execute tool with injected credentials
    │
    ▼
Sanitize response (strip any leaked credentials)
    │
    ▼
Return result to agent
```

### Type Alignment
Some types are duplicated between crates (e.g., `ToolConfig` in both `rustclaw-core` and `rustclaw-tools`). This is handled via `From` implementations for conversion. Future work could consolidate these.

### Testing Strategy
- Unit tests in each crate (80 total)
- Mock implementations for dependency injection
- HIL tests use short timeouts for fast execution
- Integration tests would require full infrastructure setup

## Running

```bash
# Build
cargo build

# Run tests
cargo test

# Run gateway
cargo run -- gateway

# Credential management examples
cargo run -- credentials status
cargo run -- credentials add homeassistant -t home_assistant -u http://homeassistant:8123
cargo run -- credentials list
cargo run -- credentials unlock
```

## Configuration Example

```toml
[gateway]
bind_host = "127.0.0.1"
port = 8765

[agents.defaults]
model = "anthropic/claude-sonnet-4-20250514"
workspace = "~/.rustclaw/agents"

[[agents.list]]
id = "main"

[tools]
profile = "standard"

[security.sandbox]
mode = "tools"
scope = "session"

# NEW: Credentials configuration
[credentials]
enabled = true
vault_path = "~/.local/share/rustclaw/credentials"
unlock_method = "env"  # or "password", "keyring"
password_env_var = "RUSTCLAW_VAULT_PASSWORD"
default_permission = "deny"
```

## API Reference (Credentials)

### HTTP Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/credentials/status` | Vault status |
| GET | `/api/credentials` | List endpoints (alias: `/api/credentials/endpoints`) |
| POST | `/api/credentials` | Create endpoint (alias: `/api/credentials/endpoints`) |
| DELETE | `/api/credentials/:id` | Delete endpoint (alias: `/api/credentials/endpoints/:id`) |
| POST | `/api/credentials/endpoints/:id/credential` | Store credential |
| GET | `/api/credentials/permissions` | List permission rules |
| POST | `/api/credentials/permissions` | Add permission rule |
| DELETE | `/api/credentials/permissions/:id` | Remove permission rule |
| GET | `/api/credentials/audit` | View audit log (query: `?limit=N`) |
| GET | `/api/credentials/approvals` | List pending HIL approvals |
| POST | `/api/credentials/approvals/:id/approve` | Approve HIL request |
| POST | `/api/credentials/approvals/:id/deny` | Deny HIL request |
| POST | `/api/credentials/approvals/respond` | Respond to HIL approval (generic) |
| POST | `/api/credentials/unlock` | Unlock vault |
| POST | `/api/credentials/lock` | Lock vault |

### Request/Response Examples

Create endpoint:
```json
POST /api/credentials/endpoints
{
  "name": "Home Assistant",
  "endpoint_type": "home_assistant",
  "base_url": "http://homeassistant:8123"
}
```

Store credential:
```json
POST /api/credentials/endpoints/<uuid>/credential
{
  "credential_type": "bearer_token",
  "secret": "<base64-encoded-secret>"
}
```

Respond to HIL approval:
```json
POST /api/credentials/approvals/respond
{
  "request_id": "<uuid>",
  "approved": true,
  "resolved_by": "kelsea",
  "denial_reason": null
}
```
