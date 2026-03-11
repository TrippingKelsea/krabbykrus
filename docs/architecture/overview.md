# RockBot Architecture Overview

RockBot is a modular AI agent framework written in Rust, designed for secure, local-first operation with emphasis on credential safety.

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        User Interfaces                          │
├─────────────────┬─────────────────┬─────────────────────────────┤
│      CLI        │      TUI        │          Web UI             │
│ (rockbot-cli)│ (rockbot-cli)│     (rockbot-core)       │
└────────┬────────┴────────┬────────┴──────────────┬──────────────┘
         │                 │                       │
         └─────────────────┼───────────────────────┘
                           │
                    ┌──────▼──────┐
                    │   Gateway   │
                    │  (HTTP/WS)  │
                    └──────┬──────┘
                           │
    ┌──────────────────────┼──────────────────────┐
    │                      │                      │
┌───▼───┐           ┌──────▼──────┐         ┌────▼────┐
│Agents │           │ Credentials │         │Sessions │
│Engine │           │   Vault     │         │ Manager │
└───┬───┘           └──────┬──────┘         └────┬────┘
    │                      │                     │
┌───▼───┐           ┌──────▼──────┐         ┌────▼────┐
│ Tools │           │   Crypto    │         │ SQLite  │
└───┬───┘           └─────────────┘         └─────────┘
    │
┌───▼───┐
│  LLM  │
│Providers│
└─────────┘
```

## Component Responsibilities

### Gateway (`rockbot-core`)

The gateway is the central coordinator:

- **HTTP Server**: Handles REST API requests
- **WebSocket**: Real-time communication (planned)
- **Agent Router**: Dispatches messages to appropriate agents
- **Session Manager**: Tracks conversation state
- **Web UI**: Serves embedded HTML dashboard

### Agent Engine (`rockbot-core`)

Executes agent logic:

- **Message Processing**: Formats prompts for LLM
- **Tool Execution**: Invokes tools with security context
- **Context Management**: Compacts history to fit context window
- **Token Tracking**: Monitors usage for rate limiting

### Credential Vault (`rockbot-credentials`)

Secure credential storage:

- **Encryption**: AES-256-GCM at rest
- **Key Derivation**: Argon2id from password
- **Audit Logging**: Hash-chained tamper-evident log
- **HIL System**: Human-in-loop approval workflow
- **Permission Evaluation**: Glob-based access control

### LLM Providers (`rockbot-llm`)

Abstract interface to language models:

- **Provider Registry**: Routes requests by model ID
- **Chat Completion**: Standard request/response format
- **Streaming**: Token-by-token responses (planned)
- **Retry Logic**: Exponential backoff (planned)

### Tools (`rockbot-tools`)

Agent capabilities:

- **Tool Registry**: Profile-based loading
- **Capability Checking**: Security context validation
- **Built-in Tools**: read, write, edit, exec
- **Credential Injection**: Automatic token insertion

### Security (`rockbot-security`)

Capability and sandboxing:

- **Capabilities**: Fine-grained permissions
- **Security Context**: Session-scoped restrictions
- **Sandbox**: Process isolation (planned)

### Memory (`rockbot-memory`)

Knowledge and context:

- **Document Loading**: File-based memory
- **Search**: Keyword and semantic (planned)
- **Core Memory**: Persistent facts

## Data Flow

### Agent Message Flow

```
User Input
    │
    ▼
Gateway receives HTTP/WS request
    │
    ▼
Session Manager loads/creates session
    │
    ▼
Agent Engine processes message
    │
    ├─► Tool Call Required?
    │       │
    │       ▼
    │   Security Check (capabilities)
    │       │
    │       ▼
    │   Credential Check (permissions)
    │       │
    │       ├─► Allow: Inject credentials
    │       ├─► AllowHIL: Wait for approval
    │       └─► Deny: Return error
    │       │
    │       ▼
    │   Execute Tool
    │       │
    │       ▼
    │   Sanitize Response
    │
    ▼
LLM Provider generates response
    │
    ▼
Session Manager stores messages
    │
    ▼
Gateway returns response
```

### Credential Access Flow

```
Tool requests credential (saggyclaw://homeassistant/api/...)
    │
    ▼
CredentialManager.check_permission(path)
    │
    ├─► Allow
    │       │
    │       ▼
    │   decrypt_credential_for_endpoint()
    │       │
    │       ▼
    │   Return secret to tool
    │
    ├─► AllowHIL
    │       │
    │       ▼
    │   Create HilApprovalRequest
    │       │
    │       ▼
    │   Notify user (Web UI, TUI, channel)
    │       │
    │       ▼
    │   Wait for approval (with timeout)
    │       │
    │       ├─► Approved: decrypt and return
    │       └─► Denied: return error
    │
    └─► Deny
            │
            ▼
        Log attempt, return error
```

## Persistence

### Files

| Path | Purpose |
|------|---------|
| `~/.config/rockbot/rockbot.toml` | Configuration |
| `~/.local/share/rockbot/sessions.db` | Session history |
| `~/.local/share/rockbot/credentials/` | Encrypted vault |
| `~/.local/share/rockbot/credentials/audit.log` | Audit trail |

### Database Schema

Sessions are stored in SQLite with tables for:
- `sessions`: Session metadata
- `messages`: Conversation history
- `token_usage`: Usage tracking

## Security Model

### Defense in Depth

1. **Encryption at Rest**: Credentials encrypted with AES-256-GCM
2. **Key Derivation**: Argon2id prevents brute force
3. **Capability System**: Tools can only do what's allowed
4. **HIL Approval**: Sensitive operations require human consent
5. **Audit Trail**: All access logged with hash chain

### Trust Boundaries

```
┌─────────────────────────────────────┐
│         User (Trusted)              │
├─────────────────────────────────────┤
│       Gateway (Semi-trusted)        │
├─────────────────────────────────────┤
│        Agent (Untrusted)            │
│  ┌─────────────────────────────┐    │
│  │   Credentials never cross   │    │
│  │    this boundary directly   │    │
│  └─────────────────────────────┘    │
└─────────────────────────────────────┘
```

Credentials are injected into tool execution but never returned to the agent directly. Responses are sanitized to prevent credential leakage.
