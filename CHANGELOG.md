# Changelog

All notable changes to RockBot are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

Every commit is automatically tagged `vX.Y.Z` (patch auto-incremented).
Release channels: `v0.2.16` (development), `v0.2.16-preview`, `v0.2.16-release`.

## [Unreleased]

### Changed
- **TUI layout unification**: Removed inner page_split card strip from all modes
  - SlottedCardBar (Row 0) is now the single source of card navigation
  - All modes get the full `main_area` as their content area (+5 rows gained)
  - Sessions grouped by agent in card bar (one card per agent, badge = session count)
  - Vault init/locked errors now shown in status strip, not as full content takeover
  - Card detail overlay (`Alt+Enter`) now has per-mode rendering with sparklines
  - Updated help text to reflect `Alt+←/→` as primary card navigation
  - Updated `docs/user-guide/tui-guide.md` with unified navigation docs

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
