# krabbykrus Implementation Gap Analysis

**Generated:** 2026-03-08
**Based on:** SPEC.md v1.0
**Codebase:** 72 Rust files, ~14,000 LOC across 11 crates

---

## Executive Summary

krabbykrus is approximately **25-30% complete** relative to the full SPEC.md specification. The credential vault system is the most mature component (~85% complete). Core gateway infrastructure exists but many subsystems are stubbed or partial. The biggest gaps are in channel support (only Discord implemented), the cron/scheduling system (not implemented), and the full agent execution pipeline.

---

## Implementation Status by Section

### ✅ Substantially Complete (>70%)

| Section | Status | Notes |
|---------|--------|-------|
| **14. Security Model** | ~85% | Credential vault, permissions, audit logging, HIL approval all implemented |
| **6. Session Management** | ~75% | SQLite persistence, session CRUD, message history working |
| **3. Gateway Protocol** | ~60% | WebSocket server running, basic RPC methods, auth flow |

### 🟡 Partially Implemented (30-70%)

| Section | Status | Notes |
|---------|--------|-------|
| **7. Agent System** | ~50% | Basic agent execution works, missing thinking levels, failover, streaming |
| **2. System Architecture** | ~45% | Gateway + Session Store exist; Channel Manager, Routing Engine partial |
| **10. Configuration** | ~40% | Config loading works, missing hot reload, many fields unparsed |
| **8. Tool System** | ~35% | read tool implemented, exec partial; missing edit, patch, glob, grep, browser_*, memory_* |
| **11. CLI Interface** | ~35% | gateway, credentials, doctor commands; missing agents, models, cron, plugins, message |
| **15. Data Storage** | ~40% | Session SQLite works; transcript JSONL partial |

### 🔴 Minimal/Stubbed (<30%)

| Section | Status | Notes |
|---------|--------|-------|
| **4. Channel System** | ~15% | Only Discord; missing Telegram, Slack, Signal, iMessage, WhatsApp, etc. |
| **5. Routing System** | ~10% | No binding resolution, no route matching, no session key formatting |
| **9. Plugin System** | ~10% | Manifest loading only; no WASM, no hook execution |
| **13. Cron System** | ~5% | Heartbeat config field exists; no scheduler, no job execution |
| **16. Skills System** | ~0% | Not implemented |
| **17. Media Pipeline** | ~0% | Not implemented |
| **12. Message Context** | ~20% | Basic message struct; MsgContext not fully populated |
| **18-19. Error/Observability** | ~25% | Basic tracing; no structured error handling, no health monitoring |

---

## Detailed Gap Analysis

### 1. Gateway Protocol (Section 3)

#### Implemented
- [x] WebSocket server on configurable port
- [x] JSON frame serialization
- [x] Basic request/response correlation
- [x] Auth token validation
- [x] Connection handshake (partial)

#### Missing
- [ ] Protocol version negotiation
- [ ] Event broadcasting with sequence numbers
- [ ] State version synchronization
- [ ] Full RPC method coverage:
  - [ ] `agent.invoke`, `agent.wait`, `agent.wake`, `agent.identity`
  - [ ] `chat.send`, `chat.abort`, `chat.inject`, `chat.history`
  - [ ] `sessions.compact`, `sessions.usage`
  - [ ] `channels.status`, `channels.send`, `channels.poll`
  - [ ] `config.set`, `config.apply`, `config.schema`
  - [ ] `cron.*` methods
- [ ] Reconnection handling
- [ ] Heartbeat/ping-pong

---

### 2. Channel System (Section 4)

#### Implemented
- [x] `ChannelPlugin` trait definition
- [x] Discord channel (Serenity-based)
  - [x] Connect/disconnect
  - [x] Send/edit/delete messages
  - [x] Event stream
  - [x] Embeds support
  - [ ] Components/buttons
  - [ ] Threads

#### Missing Channels (all 0%)
| Channel | Library | Priority |
|---------|---------|----------|
| **Telegram** | grammY/teloxide | High |
| **Signal** | signal-cli | High |
| **Slack** | Bolt | Medium |
| **WhatsApp** | Baileys | Medium |
| **iMessage** | BlueBubbles | Medium |
| Matrix | matrix-sdk | Low |
| IRC | irc-rust | Low |
| Google Chat | API | Low |
| MS Teams | Graph API | Low |

#### Missing Infrastructure
- [ ] Channel Manager (multi-channel coordination)
- [ ] Outbound delivery abstraction
- [ ] Text chunking per channel limits
- [ ] Media handling per channel
- [ ] Unified event normalization

---

### 3. Routing System (Section 5)

#### Implemented
- [x] Basic session key generation
- [x] Agent lookup by ID

#### Missing
- [ ] Binding system
  - [ ] Peer bindings
  - [ ] Guild bindings (Discord)
  - [ ] Account bindings
  - [ ] Channel bindings
- [ ] Route resolution priority chain
- [ ] Session key format parsing (`{scope}:{channel}:{identifier}`)
- [ ] Session scoping modes (per-sender, global, per-peer, etc.)
- [ ] Binding persistence and hot-update

---

### 4. Agent System (Section 7)

#### Implemented
- [x] Agent struct with config
- [x] Basic LLM invocation (Anthropic, OpenAI)
- [x] Tool calling (single tool per turn)
- [x] Session transcript persistence

#### Missing
- [ ] **Streaming responses** (marked TODO in both providers)
- [ ] Thinking levels (off/minimal/low/medium/high)
- [ ] Model failover chain
- [ ] Rate limit detection and backoff
- [ ] Auth profile fallback
- [ ] Tool execution loop (multi-tool per turn)
- [ ] System prompt assembly pipeline
- [ ] Context injection (AGENTS.md, SOUL.md, skills)
- [ ] Response delivery and chunking
- [ ] Abort handling mid-execution

---

### 5. Tool System (Section 8)

#### Implemented
| Tool | Status | Notes |
|------|--------|-------|
| `read` | ✅ Complete | File reading with offset/limit |
| `write` | 🟡 Partial | Exists but needs verification |
| `exec` | 🟡 Partial | Shell execution stubbed |

#### Missing Tools
- [ ] `edit` - File editing with diff
- [ ] `patch` - Unified diff application
- [ ] `glob` - File pattern matching
- [ ] `grep` - Content searching
- [ ] `browser_navigate` - Browser automation
- [ ] `browser_screenshot` - Page capture
- [ ] `memory_get` - Memory retrieval
- [ ] `memory_search` - Memory search

#### Missing Infrastructure
- [ ] Tool registry with capability filtering
- [ ] Sandboxed execution
- [ ] Before/after tool hooks
- [ ] Tool timeout handling
- [ ] Credential injection into tool context
- [ ] Tool result sanitization

---

### 6. Plugin System (Section 9)

#### Implemented
- [x] `PluginManager` struct
- [x] `PluginManifest` schema
- [x] Load/unload lifecycle methods
- [x] Tool/channel definition extraction

#### Missing
- [ ] **Actual plugin execution** (no WASM, no native)
- [ ] Hook registration and dispatch
- [ ] HTTP route registration
- [ ] Gateway method extension
- [ ] CLI command extension
- [ ] Service lifecycle management
- [ ] Plugin isolation/sandboxing
- [ ] Plugin discovery (global, workspace, bundled)
- [ ] Plugin configuration injection

---

### 7. Cron System (Section 13)

#### Implemented
- [x] `heartbeat_interval` config field
- [x] `last_heartbeat` tracking

#### Missing (entire subsystem)
- [ ] Cron job schema
- [ ] Schedule types (at, every, cron expression)
- [ ] Job persistence
- [ ] Scheduler loop
- [ ] Job execution
- [ ] Payload types (systemEvent, agentTurn)
- [ ] Delivery modes (none, announce, webhook)
- [ ] Job state tracking (nextRun, lastRun, errors)
- [ ] CLI: `cron list/add/edit/remove/run`
- [ ] Gateway RPC: `cron.*` methods

---

### 8. Skills System (Section 16)

#### Implemented
- Nothing

#### Missing (entire subsystem)
- [ ] Skill definition schema
- [ ] Skill discovery (bundled, workspace, agent-specific)
- [ ] Skill prompt injection
- [ ] Install specifications (brew, node, go, uv, download)
- [ ] Skill invocation policy
- [ ] Skill metadata (always, requires, os filters)

---

### 9. Media Pipeline (Section 17)

#### Implemented
- Nothing

#### Missing (entire subsystem)
- [ ] Media type detection
- [ ] Image processing (resize, convert to JPEG)
- [ ] Audio transcription (STT)
- [ ] TTS synthesis
- [ ] Video thumbnail extraction
- [ ] Document text extraction (PDF, etc.)
- [ ] Media caching
- [ ] Per-channel media format adaptation

---

### 10. Configuration System (Section 10)

#### Implemented
- [x] JSON5 config parsing
- [x] Gateway config section
- [x] Credentials config section
- [x] Agent definitions (partial)
- [x] Logging config (partial)

#### Missing
- [ ] Hot reload on config change
- [ ] Full `auth.profiles` handling
- [ ] Model definitions and aliases
- [ ] Session config (scope, idleMinutes, typingMode, reset)
- [ ] Per-channel settings parsing
- [ ] Tool allow/disallow lists
- [ ] Environment variable expansion
- [ ] Config validation against schema
- [ ] CLI: `config get/set/apply/schema`

---

### 11. CLI Commands (Section 11)

#### Implemented
| Command | Status |
|---------|--------|
| `gateway run` | ✅ Working |
| `gateway dev` | 🟡 Partial |
| `credentials *` | ✅ Most subcommands |
| `doctor` | 🟡 Basic checks |
| `session` | 🟡 Basic ops |

#### Missing Commands
- [ ] `setup` - Workspace initialization
- [ ] `onboard` - Interactive wizard
- [ ] `configure` - Guided config
- [ ] `status` - Channel health overview
- [ ] `agent` - Agent invocation
- [ ] `agents` - Agent management
- [ ] `message send` - Direct message sending
- [ ] `channels` - Channel management
- [ ] `models` - Model configuration
- [ ] `cron` - Cron job management
- [ ] `plugins` - Plugin management

---

### 12. Security Model (Section 14)

This is the **most complete** section.

#### Implemented
- [x] Master key derivation (Argon2id)
- [x] Credential encryption (AES-256-GCM)
- [x] Credential storage with nonces
- [x] 4-tier permission levels
- [x] Path pattern matching (glob)
- [x] Permission evaluation
- [x] HIL approval queue
- [x] HIL notification channel
- [x] Hash-chained audit log
- [x] Audit log verification
- [x] Multiple unlock methods (password, keyfile, Age)
- [x] TUI for credential management

#### Missing
- [ ] Keyring integration (macOS Keychain, etc.)
- [ ] YubiKey/hardware key support
- [ ] 2FA for AllowHIL2FA level
- [ ] Response sanitization (credential stripping)
- [ ] Memory protection (mlock)
- [ ] Credential rotation
- [ ] API endpoints (REST interface in gateway)
- [ ] Mobile push notifications for HIL

---

### 13. TUI Application

#### Implemented
- [x] Main dashboard
- [x] Agents list view
- [x] Sessions list view
- [x] Credentials management
  - [x] Endpoints tab
  - [x] Permissions tab
  - [x] Audit log tab
  - [x] Settings tab
- [x] Chat interface (partial)
- [x] Modals (password entry, confirmations)

#### Missing
- [ ] Models configuration tab
- [ ] Channels status view
- [ ] Cron jobs view
- [ ] Plugins view
- [ ] Full chat streaming display
- [ ] Session detail drilldown
- [ ] Real-time updates via WebSocket

---

### 14. Web UI

#### Implemented
- [x] Basic HTTP server in gateway
- [x] Static file serving
- [x] Status endpoint

#### Missing
- [ ] Full web application
- [ ] WebSocket client integration
- [ ] Chat interface
- [ ] Dashboard
- [ ] Mobile-responsive design

---

## Priority Recommendations

### Phase 1: Core Agent Loop (High Priority)
1. **Streaming responses** - Critical for UX
2. **Tool execution loop** - Multi-tool turns
3. **System prompt assembly** - AGENTS.md injection
4. **Error handling** - Structured errors with retry

### Phase 2: Multi-Channel (High Priority)
1. **Telegram channel** - Most requested
2. **Signal channel** - Privacy-focused users
3. **Channel manager** - Unified coordination
4. **Message routing** - Binding system

### Phase 3: Automation (Medium Priority)
1. **Cron scheduler** - Background jobs
2. **Skills system** - Tool packaging
3. **Config hot reload** - Live updates

### Phase 4: Polish (Lower Priority)
1. **Plugin execution** - WASM or native
2. **Media pipeline** - Full media handling
3. **Web UI** - Full application
4. **CLI completeness** - All commands

---

## Lines of Code by Crate

| Crate | LOC | Primary Purpose |
|-------|-----|-----------------|
| `krabbykrus-core` | ~3,500 | Gateway, session, agent, config |
| `krabbykrus-credentials` | ~3,300 | Vault, permissions, audit |
| `krabbykrus-cli` | ~2,800 | Commands, TUI |
| `krabbykrus-llm` | ~1,500 | Anthropic, OpenAI providers |
| `krabbykrus-channels` | ~1,000 | Discord, channel traits |
| `krabbykrus-tools` | ~700 | Tool registry, builtins |
| `krabbykrus-memory` | ~400 | Memory system (stubbed) |
| `krabbykrus-plugins` | ~200 | Plugin manager (scaffold) |
| `krabbykrus-security` | ~300 | Capabilities, context |

---

## Technical Debt

### TODOs in Codebase (20 items)
```
crates/krabbykrus-cli/src/tui/app.rs:        // TODO: Actually unlock via SSH agent
crates/krabbykrus-cli/src/tui/app.rs:        // TODO: Load session details from gateway API
crates/krabbykrus-cli/src/tui/credentials.rs: // TODO: Reload endpoints (4 items)
crates/krabbykrus-cli/src/commands/credentials.rs: // TODO: delete_endpoint, list_permissions, remove_permission
crates/krabbykrus-core/src/gateway.rs:        // TODO: Implement keyring support
crates/krabbykrus-core/src/gateway.rs:        uptime_seconds: 0, // TODO: Track actual uptime
crates/krabbykrus-core/src/agent.rs:          tool_calls: None, // TODO: Handle tool calls
crates/krabbykrus-llm/src/anthropic.rs:       // TODO: Implement streaming with SSE
crates/krabbykrus-llm/src/openai.rs:          // TODO: Implement streaming with SSE
```

### Missing Tests
- [ ] Integration tests for gateway protocol
- [ ] E2E tests for full message flow
- [ ] Channel adapter tests
- [ ] Credential injection tests
- [ ] Cron scheduler tests

### Documentation Gaps
- [ ] API documentation (rustdoc incomplete)
- [ ] User guide
- [ ] Channel setup guides
- [ ] Plugin development guide

---

## Conclusion

The foundation is solid: the gateway runs, sessions persist, credentials are secure, and Discord works. The path forward is:

1. **Complete the agent execution loop** (streaming, multi-tool, prompts)
2. **Add Telegram/Signal** (cover the most-requested channels)
3. **Build the cron scheduler** (enable automation)
4. **Fill in CLI/TUI gaps** (usability)

Estimated effort to reach SPEC.md parity: **3-4 months** at current pace, or **6-8 weeks** with focused full-time development.
