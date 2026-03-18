# Changelog

All notable changes to RockBot are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

Every commit is automatically tagged `vX.Y.Z` (patch auto-incremented).
Release channels: `v0.2.16` (development), `v0.2.16-preview`, `v0.2.16-release`.

## [Unreleased]

### Added
- **Completions**: shell completion generation via `rockbot completion <bash|zsh|fish|powershell|elvish>`
- **TUI**: slash command autocompletion with `Tab` / `Shift+Tab`
- **Tools**: new `rockbot-tools-system` crate for system-facing tools such as read/write/exec/browser
- **Tools**: standard system tool profile now includes the browser tool
- **Architecture docs**:
  - `docs/architecture/webui-wasm-migration-plan.md`
  - `docs/architecture/browser-plugin-architecture-plan.md`
  - `docs/architecture/tooling-gap-analysis.md`
  - WebUI WASM migration plan now includes:
    - WCAG/ADA usability requirements
    - frontend stack comparison across Leptos, Dioxus, Yew, Sycamore, and Ratzilla
    - a Ratzilla contender assessment
    - a shared TUI/Web UI abstraction strategy
- **Public listener policy**: `gateway.public` now controls whether the public
  HTTPS listener serves the browser bootstrap shell, CA bundle, and enrollment
- **Web bootstrap shell**: `/` and `/static/*` now serve a minimal browser
  bootstrap app with health display, CA download, and IndexedDB-backed client
  certificate/key import persistence
  - Browser bootstrap clients can authenticate over the public WebSocket with a
    certificate challenge/response flow after importing key material
- **Docs**: Added a formal remediation plan for the current code review findings
  - `docs/architecture/code-review-round2-remediation-plan.md`
- **Gateway bootstrap**: deterministic role-targeted bootstrap config commands
  - `rockbot config init gateway --https-port ... --client-port ...`
  - `rockbot config init client --gateway-ip ... --https-port ... --client-port ...`
- **Gateway networking**: split public and client listeners
  - Public HTTPS listener for Web UI, health, and certificate enrollment
  - Dedicated client listener for TUI, WebSocket, and mTLS client traffic
- **Config**: new `[client]` bootstrap section for gateway host, public port,
  and dedicated client port
- **Config**: `[security.storage]` and `[security.roles]` for encrypted local
  storage policy, PKI-backed local key sourcing, and explicit gateway /
  vault-provider deployment intent
- **PKI**: Node-local key helpers in `rockbot-pki`
  - PKI-managed local storage keys via `ensure_local_storage_key()`
  - Age-based node vault keypairs via `ensure_vault_keypair()`
- **Store**: Distributed vault tables and replication policy coverage
  - `node_keys`
  - `vault_objects`
  - `vault_provider_grants`
  - `vault_node_grants`
  - `vault_policies`
- **Credentials**: Initial distributed vault primitives
  - Registered node vault-key records
  - Logical vault objects separate from grant payloads
  - Per-recipient Age-encrypted provider and node grants
  - Grant decryption helpers and async manager wrappers
- **Gateway startup**: Automatic local vault-node bootstrap when vault and PKI
  are available
  - Ensures a node-local vault keypair exists
  - Registers the local node's vault public key and configured roles in the vault
- **Config**: `[security.noise]` transport policy scaffolding for future
  `ws-over-noise` and `stream-over-noise` enforcement modes
- **Gateway API**: `GET /api/executors` for listing connected remote executors,
  their identities, advertised working directories, and capability sets
- **WebSocket control plane**: native clients now tunnel gateway management/data
  requests over the client-listener WebSocket instead of mixing WS on the
  client listener with REST calls on the public listener
- **Bedrock provider**: provider model inventory now includes AWS Bedrock
  inference profiles alongside foundation models so TUI and agent pickers can
  target either a direct model ID or a named/system-defined profile
- **Config**: Rich TUI theme token configuration via `[tui.theme]`
  - RGBA token overrides for border, text, AI/thinking/tool text, accents, graphs, and backgrounds
  - Backward-compatible preset resolution from legacy `color_theme`
- **Config**: Stored font preference stubs via `[tui.fonts]`
  - Interface, user, AI, thinking, and tool font family/size preferences persisted in config
  - Terminal TUI stores these preferences for future richer renderers such as the Web UI
- **TUI**: WebSocket connection monitoring — RTT latency sampling, reconnect/disconnect counters,
  live sparkline graph on Client dashboard card and detail panel
- **TUI**: `active_connections` field parsed from gateway health status and displayed in Client detail
- **TUI**: Bracketed paste support — pasted text inserted at cursor in chat input
- **TUI**: Focus change events enabled (terminal focus gained/lost)
- **TUI**: `WsConnectionChanged` and `WsLatencySample` messages for real-time WS state tracking
- **TUI**: Dashboard Client card replaced Sessions overview with WS Connection detail panel
  showing RTT, server connections, server sessions, reconnect/disconnect counts
- **TUI**: Gateway load sparkline now driven by `active_connections` instead of static data
- **TUI**: Dashboard Noise and Exec cards for remote-exec visibility and control
  - Noise card shows registration state and connected executor count
  - Exec card shows current tool locality target and a detail overlay for switching
    between the active client, gateway-local execution, and another connected executor
- **TUI**: Searchable provider/model inventory
  - `Alt+M` now presents a reorganized provider browser with provider inventory,
    inline fuzzy search, and Bedrock inference profiles separated from foundation models
  - Agent/session model pickers now support inline fuzzy search with `nucleo-matcher`
- **TUI**: `Alt+A` agent launcher for fuzzy switching between configured agents
  and creating a new agent from anywhere in the client
- **TUI**: Model-to-agent flow improvements
  - `Alt+M` now treats `Enter` on the selected model/profile as “create agent from this model”
  - Provider configuration in the models overlay moved to `Ctrl+E` so plain typing always stays search
  - The create-agent modal now uses a searchable model field with `Up/Down` selection instead of a left/right carousel
  - New agents default to temperature `0.5`, and default max tokens now follow the selected model’s advertised max output
- **Docs**: Execution locality hardening proposal and feature evaluation tracker
  - `docs/architecture/execution-locality-proposal.md`
  - `docs/feature-evaluation.md`
- **TUI**: Settings overlay tab bar (General | Paths | About | Theme | Fonts) with Left/Right/Tab navigation
- **TUI**: Theme picker in Settings — change color theme and animation style live with `[`/`]` keys
- **TUI**: Rich settings overlay editor with token-level theme controls, live preview, and
  typography preference stubs
  - Mouse-enabled wheel-style color picker for precise custom colors
  - Separate border, primary/secondary text, AI/thinking/tool text, accent, graph, and background tokens
  - Stored interface/user/AI/thinking/tool font family + size preferences
  - Automatic save of `[tui]`, `[tui.theme]`, and `[tui.fonts]` changes to `rockbot.toml`

### Fixed
- **Public surface area**: the public HTTPS listener no longer exposes the full
  management/data REST API by default; it is now limited to health, bootstrap
  assets, CA publication, and optional enrollment
- **Remote exec streaming**: fast remote tools no longer log repeated
  “Received tool output for unknown request” warnings when output chunks arrive
  just after the final response wins the race back to the gateway
- **Overseer defaults**: gateways built with the `overseer` feature now seed and
  use a default overseer configuration from encrypted storage when available,
  and fall back to an in-memory default when the store is not yet present
- **TUI startup target**: the main chat now prefers a primary enabled agent,
  then the first enabled agent, instead of landing on Butler by default
- **Bedrock fallback**: agent chat and tool-loop LLM calls now fall back to
  non-streaming completions when Bedrock streaming repeatedly returns service
  errors, instead of failing the request outright
- **Tool streaming**: remote tool stdout/stderr chunks now travel as distinct
  WebSocket `tool_output` events end-to-end instead of being mislabeled as final
  `tool_result` payloads, so native clients can render incremental output
  without duplicating the final completion text
- **Bootstrap workflow**: generated config no longer seeds agent definitions by
  default; bootstrap TOML is now connection-focused instead of embedding runtime
  agent state
- **Web and enrollment access**: browser access and client enrollment no longer
  depend on weakening client-certificate requirements for the dedicated client
  listener
- **TUI transport split bug**: provider loading, agent loading, session creation,
  cron actions, context-file operations, and provider credential saves no longer
  fail by accidentally targeting the wrong listener port
- **Gateway startup**: Session, cron, and agent-persistence stores can now use
  PKI-managed node-local storage keys through encrypted redb open paths
- **Gateway WebSocket**: agent messages no longer block the connection read loop,
  allowing remote tool responses to be processed immediately instead of timing
  out and arriving later as unknown request IDs
- **Remote execution**: TUI-originated shell/filesystem calls now default to the
  active client's current working directory instead of the gateway host cwd
- **Remote execution**: Explicit remote executor selection no longer inherits the
  requesting client's cwd; selected executors fall back to their own advertised workdir
- **Gateway WebSocket**: UTF-8-safe truncation for forwarded tool output and final tool summaries
  no longer panics on multibyte characters
- **Remote filesystem tools**: `read`, `write`, `edit`, and `patch` now accept
  common path aliases (`path`, `file`) for better client passthrough compatibility
- **Remote filesystem tools**: `write` now accepts `text` as a content alias in
  addition to `content`
- **Storage**: sessions, cron jobs, and route bindings now persist in redb
  instead of SQLite
- **Store replication**: sessions, session messages, cron jobs, and agents now
  have explicit replication policies in `rockbot-store::sync`
- **Raft state machine**: `agents` table mutations are now accepted during
  store replication
- **TUI**: Chat input box now always visible (was missing on Dashboard/Butler and agent welcome screens)
- **TUI**: Bottom status bar no longer shows persistent help text — only displays errors/success messages
- **TUI**: Terminal no longer left in broken state on unclean exit — `TerminalGuard` RAII restores
  raw mode, alternate screen, keyboard enhancement, bracketed paste, and focus change on drop
- **TUI**: Shift+Enter / Ctrl+J newline detection now goes through `normalize_for_text_input()`
  which checks all known representations (KeyCode::Enter, Char('\r'), Char('\n') with SHIFT)
- **TUI**: Modified keys (Alt+letter, Ctrl+letter) no longer accidentally insert text in chat input —
  text acceptance guarded to empty or Shift-only modifiers

### Changed
- **Feature flags**: split `noise` transport primitives from `remote-exec`, with
  `remote-exec` now layering on top of `noise`
- **WebSocket protocol**: `agent_message`, `tool_call`, and `tool_result` now carry
  execution-locality metadata or executor-target hints for remote dispatch routing
- **Remote execution**: `exec` on remote clients now streams stdout/stderr over the
  WS protocol before sending the final tool result
- **TUI**: Tool output status and session summaries now surface locality as
  `executed on: ...`
- **Client/Gateway protocol**: `tool_result` events now preserve result text and
  session keys for inline TUI streaming of tool output
- **Workspace architecture**: removed the `rockbot-core` facade crate and
  updated CLI/TUI/root crates to depend on focused subcrates directly
- **Paths**: default local state now uses `sessions.redb` and `cron.redb`
- **TUI**: Input architecture rewritten — replaced busy-loop `poll/read` inside `tokio::select!`
  with crossterm's async `EventStream` in a dedicated task, eliminating spurious wakeups
- **TUI**: Input normalization layer (`InputAction` enum + `normalize_for_text_input()`) is now
  the single source of truth for text-input key semantics across all chat/editor contexts
- **TUI**: Tick handling moved through `Message::Tick` so sparkline history buffers update in state
- **TUI**: WS health check now sends a `ping` before `health_check` to measure round-trip time
- **Client**: `GatewayEvent::HealthStatus` now includes `active_connections` field

### Changed
- **Chat-first TUI architecture**: Chat is always visible; other views are overlays
  - Main content area always renders chat (butler, session, or agent)
  - Card bar reduced to 4 modes: Dashboard, Agents, Sessions, Cron Jobs
  - Credentials, Models, Settings are now overlay modals (Alt+V/M/S)
  - Global persistent status strip: gateway / agents / sessions / vault / chat target
  - Models overlay uses dynamic provider tabs from gateway (no more hardcoded Bedrock/Anthropic/OpenAI/Ollama cards)
  - Cron filter (All/Active/Disabled) moved to inline toggle in cron overlay
  - Number keys 1-4 switch modes, 5-7 open overlays
  - Updated `docs/user-guide/tui-guide.md` and `docs/user-guide/configuration.md`

### Added
- **Color themes**: Purple (default), Blue, Green, Rose, Amber, Mono
  - Configure via `[tui] color_theme` in `rockbot.toml`
  - Theme-driven palette functions in `effects::palette`
- **Animation styles**: Coalesce (default), Fade, Slide, None
  - Configure via `[tui] animation_style` in `rockbot.toml`
  - Setting to `None` disables all overlay transitions
- **Overlay keybindings**: Alt+V (Vault), Alt+S (Settings), Alt+M (Models), Alt+C (Cron)
  - Available in both normal and chat modes
- **ChatTarget**: Butler, Session, or Agent — determines what the chat area shows

### Added
- **Butler agent** (`rockbot-butler`): Embedded queer sassy companion agent
  - Local GGUF model chat via shared `SeedModelConfig`
  - `/butler status`, `/butler mood`, `/butler help` slash commands
  - Gateway intercept (feature-gated `butler`, in `enhanced` profile)
  - Butler chat as permanent main TUI view on Dashboard
- **Card chain navigation**: Replaces sidebar with horizontal card strip
  - Multi-level drill-down (Agents → agent list, Sessions → session list)
  - Breadcrumb trail for nested navigation
  - `h/l/j/k/Enter/Esc` key navigation when focused
- **Vault agent storage**: Move agent configs from TOML to redb vault
  - `AGENTS` table in rockbot-store with CRUD operations
  - Auto-migrate from `[[agents.list]]` on first gateway startup
  - Vault-first loading with TOML fallback
- **Configurable keybindings**: Data-driven TUI key dispatch
  - `KeybindingConfig` with per-mode bindings (normal, chat, card_chain)
  - Vault-stored JSON config with 5s hot-reload polling
  - All ~30 TUI actions mapped through `TuiAction` enum
- **Seed model config**: Shared `[seed_model]` TOML section
  - Single GGUF model definition for Butler, Doctor, and Overseer
  - Defaults to Qwen2.5-1.5B-Instruct
- **Doctor TUI**: Standalone chat with Doctor AI model
  - `rockbot doctor tui` subcommand (no gateway required)
  - DoctorAi `chat()` method for free-form conversation
- **Feature profiles**: Meta feature flags for common build configurations
  - `conservative` (default): stable production features
  - `enhanced`: conservative + overseer, doctor-ai, vault-replication
  - `experimental`: enhanced + otel, bedrock-deploy
  - `enshitify`: discord
- **TUI modernization**: Full visual overhaul of the terminal UI
  - Rounded borders (`BorderType::Rounded`) on all blocks and modals
  - Native `Scrollbar` widgets on sessions chat, credentials endpoints, and model list modals
  - tachyonfx integration: modal coalesce/dissolve effects, page fade transitions, background dimming
  - Floating top bar: sidebar + card strip overlay content (behind `[tui] floating_bar` config)
  - Context menu: press `?` on any page for page-specific actions (add, edit, delete, refresh, etc.)
  - `Constraint::Fill(1)` + `Flex::Start` layout for card strips
  - New `[tui]` config section with `floating_bar` and `animations` toggles
- **rockbot-deploy**: New crate for S3 CA certificate distribution and Route53 DNS auto-provisioning
  - `CaDistributor`: S3 bucket creation, public policy, CA cert upload
  - `DnsProvisioner`: Private hosted zone management, CNAME record upsert
  - `AwsCredentialImporter`: Auto-discover and import AWS keys into vault
  - `DeployConfig`: Full configuration with defaults (bucket, region, dns_zone, etc.)
  - Compile-time + runtime S3 endpoint override for LocalStack/custom endpoints
  - Feature-gated: `bedrock-deploy` (opt-in, not in defaults)
  - CLI: `rockbot cert ca publish` — interactive S3 + DNS provisioning
  - Gateway: auto-publishes CA cert on startup when `upload_on_startup = true`
- **rockbot-store**: New crate providing unified embedded storage via redb
  - ChaCha20 block-level encryption via `redb::StorageBackend`
  - 10 table definitions for all persistent state (endpoints, credentials, permissions, KV, sessions, cron, routing, PKI)
  - Per-table sync policies (Eager, Eventual, LocalOnly)
  - OpenRaft integration behind `replication` feature (log store, state machine, network stubs)
  - Generic KV store with namespaced keys (`namespace\0key`)
- **vault-redb-migration**: `rockbot-credentials` now uses redb (via `rockbot-store`) instead of flat JSON files
  - Automatic migration: legacy `endpoints.json`/`credentials.json` imported on first open, renamed to `.json.migrated`
  - KV store methods on `CredentialVault`: `kv_put`, `kv_get`, `kv_delete`, `kv_list`
  - Permission persistence: `store_permission`, `delete_permission`, `list_permissions`
  - KV async wrappers on `CredentialManager`
- **doctor-self-learning**: Doctor AI now remembers verified fixes across sessions
  - `LearnedStore` backed by JSONL at `{data_local_dir}/rockbot/doctor/learned.jsonl`
  - SHA-256 fingerprinting of error+field for instant fix recall
  - Few-shot prompt injection: recent successful fixes as examples for the model
  - Verification loop: fixes are validated by re-parsing TOML before committing
  - `[learned]` label in migration output for fixes from the learned store
- **doctor-ai**: New `rockbot-doctor` crate with embedded GGUF model for AI-powered
  config diagnostics and auto-repair (feature-gated behind `doctor-ai`)
  - `diagnosis.rs` — AI-generated human-readable explanations for config errors
  - `repair.rs` — Structure-preserving TOML auto-repair via `toml_edit`
  - `migration.rs` — Detection and rewriting of deprecated/renamed config fields
  - `prompts.rs` — Prompt templates for local GGUF model inference
  - Startup interception: doctor runs automatically when config fails to load
  - Reuses `rockbot-overseer` candle/GGUF inference infrastructure
  - `[doctor]` config section (see `docs/user-guide/configuration.md`)
- **PkiConfig**: Extracted TLS/PKI settings into shared `PkiConfig` struct, reusable by gateway, client, and agent consumers (backward-compatible via `#[serde(flatten)]`)
- **x.509 extensions**: Nebula-inspired custom certificate extensions for roles and groups (OIDs `1.3.6.1.4.1.59584.1.{1,2}`), embedded in issued certificates and parseable at connection time
- **Vault replication design doc**: Draft architecture for replicating PKI state and credentials over Noise protocol links (`docs/architecture/vault-replication.md`)
- **Post-build test harness**: strace-based performance and security validation in `tests/post-build/` (CI-integrated)

### Changed
- Lazy Tokio runtime: `--help`/`--version` skip async runtime initialization
- `GatewayConfig` TLS fields moved to nested `pki: PkiConfig` (TOML format unchanged due to flatten)
- `generate_client()`, `sign_csr()`, and `generate_client_cert()` now accept `roles` and `groups` parameters
- `CertEntry` gains `roles` and `groups` fields (defaulting to empty, backward-compatible with existing `index.json`)
- `rotate_client()` preserves roles and groups from the previous certificate

## [0.2.15] - 2026-03-15

### Added
- CHANGELOG.md (this file), CLAUDE.md with documentation and commit standards
- Configuration reference (`docs/user-guide/configuration.md`)
- Security model documentation (`docs/architecture/security.md`)
- LICENSE file (MIT)
- CI tag-based release channels (development / preview / release)
- Post-commit hook for automatic `vX.Y.Z` git tagging
- Preview workflow (`.github/workflows/preview.yml`)

### Fixed
- Broken README links: `configuration.md`, `security.md`, `LICENSE` now exist

### Changed
- CI/CD workflows use tag-based artifact paths (`{channel}/rockbot-vX.Y.Z-{target}`)
- Release workflow triggers on `v*-release` tags only
- Build artifacts named `rockbot-vX.Y.Z-{target}.tar.gz`
- Updated CONTRIBUTING.md with versioning and release channel docs

## [0.2.14] - 2026-03-15

### Added
- Full PKI documentation (`docs/architecture/pki.md`)
- Configuration reference (`docs/user-guide/configuration.md`)
- Security model documentation (`docs/architecture/security.md`)
- CHANGELOG.md
- CLAUDE.md with documentation and commit standards
- LICENSE file

### Changed
- Updated all docs to reflect mTLS PKI system
- Updated crate count to 20 across documentation
- Updated feature matrix with PKI/mTLS status and corrected WebSocket/TLS status

## [0.2.13] - 2026-03-15

### Added
- **mTLS PKI system** — new `rockbot-pki` crate (20th crate in workspace)
  - Certificate Authority generation and management
  - Client certificate issuance with role-based EKUs (gateway, agent, tui)
  - CSR signing (local and remote via gateway API)
  - Certificate revocation and CRL generation
  - Enrollment tokens (one-time/limited-use with optional expiry)
  - `KeyBackend` trait with `FileBackend` (file-backed PEM, 0600 perms)
  - `KeyHandle::Hardware` variant stubbed for future HSM/YubiKey support
- Rewritten `cert` CLI with hierarchical subcommands:
  `cert ca`, `cert client`, `cert sign`, `cert install`, `cert verify`,
  `cert info`, `cert enroll`
- Gateway mTLS enforcement via `WebPkiClientVerifier`
- `POST /api/cert/sign` — PSK-authenticated remote CSR enrollment endpoint
- `GET /api/cert/ca` — public CA certificate retrieval
- `cert install` — patches `rockbot.toml` with TLS paths automatically
- GatewayConfig fields: `tls_ca`, `require_client_cert`, `pki_dir`, `enrollment_psk`

## [0.2.12] - 2026-03-14

### Fixed
- WebSocket client now accepts self-signed TLS certificates
  (custom `AcceptAnyCert` verifier with `connect_async_tls_with_config`)

## [0.2.11] - 2026-03-14

### Fixed
- Install rustls `CryptoProvider` at CLI entrypoint (not just gateway),
  preventing TUI panics on TLS operations

## [0.2.10] - 2026-03-14

### Fixed
- Overseer auto-detects GGUF model architecture from metadata
  (`general.architecture` field)
- Added Qwen2 model support alongside LLaMA in overseer inference

## [0.2.9] - 2026-03-14

### Fixed
- Overseer tokenizer download falls back to base model repo when GGUF
  repo lacks `tokenizer.json` (strips `-GGUF` suffix automatically)
- Added configurable `tokenizer_repo` field to overseer config

## [0.2.8] - 2026-03-14

### Fixed
- Install rustls `CryptoProvider` on gateway startup before TLS operations
- Expand tilde (`~`) in TLS cert/key paths during certificate rotation

## [0.2.7] - 2026-03-14

### Fixed
- Expand tilde (`~`) in `tls_cert` and `tls_key` config paths at gateway
  startup (paths like `~/.config/rockbot/gateway.crt` now resolve correctly)

## [0.2.6] - 2026-03-14

### Added
- `rockbot cert` CLI for certificate management (generate, info, rotate,
  verify with SAN display and chain verification)

## [0.2.3] - 2026-03-13

### Added
- **TLS gateway** — HTTPS/WSS by default with `rustls`
- `http-insecure` feature flag to allow plain HTTP/WS connections
- Flexible URL parsing — `rockbot tui -g host:port` works without scheme prefix
- `rockbot config init` generates self-signed TLS certificate

## [0.2.2] - 2026-03-13

### Fixed
- Remote tool response retry on WebSocket disconnect
- Overseer initialization error reporting improvements

## [0.2.0] - 2026-03-12

### Changed
- **Major workspace refactor** — monolithic `rockbot-core` split into
  8 focused crates:
  - `rockbot-config` — config types, message types, errors
  - `rockbot-session` — session management and SQLite persistence
  - `rockbot-agent` — agent execution engine, hooks, guardrails, trajectory
  - `rockbot-client` — gateway WS client, ACP, remote exec
  - `rockbot-gateway` — HTTP/WS server, A2A, cron, routing
  - `rockbot-webui` — embedded web dashboard (static HTML)
  - `rockbot-core` — thin re-export facade for backward compatibility
- Workflow data types moved to `rockbot-config`
- Version bumped from 0.1.x to 0.2.0

## [0.1.23] - 2026-03-11

### Added
- End-to-end remote tool execution wiring
- Slash command system for agents

## [0.1.21] - 2026-03-11

### Added
- **Remote tool execution** over Noise Protocol encrypted channels
- Agent SOUL.md self-access for identity context

## [0.1.19] - 2026-03-10

### Added
- `--gateway` flag on TUI for remote server connections

## [0.1.18] - 2026-03-10

### Added
- **rockbot-overseer** crate — embedded local model for agent oversight
  (GGUF inference via candle, judgment/verdict system, decision logging)

## [0.1.17] - 2026-03-10

### Changed
- Added GitHub CI/CD workflows (build, test, release)
- Restricted CI to `main` branch and version tags

## [0.1.10] - 2026-03-09

### Added
- Cron jobs TUI pane with full CRUD key handling

## [0.1.8] - 2026-03-09

### Changed
- Removed HTTP fallback from TUI — WebSocket-only communication

## [0.1.5] - 2026-03-08

### Added
- Real-time LLM streaming through WebSocket path for text deltas

## [0.1.3] - 2026-03-08

### Added
- Dynamic thinking indicator with rotating words, token count, tok/s display

## [0.1.1] - 2026-03-07

### Added
- **Multi-agent orchestration** — handoffs, swarm blackboards, graph workflows
  - `ToolResult::Handoff` variant, `HandoffTool`, `HandoffSignal`
  - `SwarmBlackboard` with `BlackboardReadTool` / `BlackboardWriteTool`
  - `WorkflowDefinition` / `WorkflowNode` / `WorkflowEdge` with DAG executor
- LLM and tool per-call timeouts (`llm_timeout_secs`, `tool_timeout_secs`)
- Cron scheduler with SQLite persistence, wired into gateway lifecycle

## [0.1.0] - 2026-03-01

### Added
- Initial release
- **Gateway server** — HTTP API with agent lifecycle management
- **Agent engine** — iterative tool-use loop with context injection
- **LLM providers** — Anthropic, OpenAI, AWS Bedrock (streaming for all)
- **Channel integrations** — Discord (Serenity), Telegram (Teloxide)
- **Encrypted credential vault** — AES-256-GCM, Argon2id, hash-chained audit
- **Tool system** — read, write, edit, exec, glob, grep, patch, memory tools
- **TUI** — ratatui-based terminal interface with card strip navigation
- **Web UI** — embedded single-page dashboard
- **MCP client** — stdio transport with dynamic tool registration
- **A2A protocol** — agent-to-agent JSON-RPC dispatch
- **Hook system** — pre/post message and tool call middleware
- **Observability** — metrics crate facade with request/tool tracking
- **SSE streaming** — `POST /api/agents/{id}/stream`
- **Structured output** — JSON mode for LLM providers
- **Sandbox enforcement** — path, executable, and timeout guards
- **HIL approval** — human-in-the-loop tool call gating
- **Subagent delegation** — `invoke_agent` tool with depth limits
- **Web tools** — `web_fetch` and `web_search`
- **Hybrid memory search** — TF-IDF + embedding-based ranking
- **Guardrails** — PII detection, prompt injection guard
- **Trajectory recording** — full execution trace with JSONL export
- **Plan-and-execute** — optional planning phase before tool use
- **Reflection loop** — post-tool-loop self-critique
- **Episodic memory** — cross-session keyword recall
- **Codebase indexing** — symbol extraction, repo map, TF-IDF ranking

[0.2.14]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.13...v0.2.14
[0.2.13]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.12...v0.2.13
[0.2.12]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.11...v0.2.12
[0.2.11]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.10...v0.2.11
[0.2.10]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.9...v0.2.10
[0.2.9]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.8...v0.2.9
[0.2.8]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.3...v0.2.6
[0.2.3]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/TrippingKelsea/rockbot/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.23...v0.2.0
[0.1.23]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.21...v0.1.23
[0.1.21]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.19...v0.1.21
[0.1.19]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.18...v0.1.19
[0.1.18]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.17...v0.1.18
[0.1.17]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.10...v0.1.17
[0.1.10]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.8...v0.1.10
[0.1.8]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.5...v0.1.8
[0.1.5]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.3...v0.1.5
[0.1.3]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.1...v0.1.3
[0.1.1]: https://github.com/TrippingKelsea/rockbot/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/TrippingKelsea/rockbot/releases/tag/v0.1.0
