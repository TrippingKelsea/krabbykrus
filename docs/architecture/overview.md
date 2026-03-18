# Architecture Overview

RockBot is a modular AI agent framework written in Rust. It runs as a
self-hosted gateway that accepts messages from multiple channels, routes them
to configured agents, and returns responses — with credentials managed in an
encrypted vault that agents never see directly.

## High-Level Architecture

```
┌───────────────────────────────────────────────────────────┐
│                      Interfaces                           │
│ CLI (rockbot-cli)  TUI (rockbot-tui)  Web App  Channels  │
│         ▲                 ▲              ▲    (Telegram, │
│         │                 │              │     Telegram, │
│         │                 │ WebSocket    │ Bootstrap     │
└─────────┼────────────────────┼──────────┬─────────────────┘
          │                    │          │
     ┌────▼────────────────────▼──────────▼────┐
     │              Gateway (TLS)               │
     │         rockbot-gateway                  │
     │  ┌──────────┬──────────┬──────────────┐  │
     │  │ Routing  │  A2A     │ Cron         │  │
     │  │ Engine   │ Protocol │ Scheduler    │  │
     │  └──────────┴──────────┴──────────────┘  │
     └──────────────────┬──────────────────────┘
                        │
          ┌─────────────┼─────────────┐
          │             │             │
     ┌────▼───┐   ┌─────▼─────┐  ┌───▼─────┐
     │ Agent  │   │ Credential│  │ Session │
     │ Engine │   │   Vault   │  │ Manager │
     └────┬───┘   └───────────┘  └─────────┘
          │
     ┌────┼────────────────┐
     │    │                │
┌────▼──┐ │  ┌─────────────▼──────────────┐
│ Tools │ │  │    Remote Executors         │
│       │ │  │  (TUI / CLI clients over   │
└───────┘ │  │   Noise Protocol)          │
          │  └────────────────────────────┘
     ┌────▼────┐
     │   LLM   │
     │Providers │
     └─────────┘
```

## Core Concepts

### Gateway

The gateway (`rockbot-gateway`) is the single source of truth. It owns:

- **Agent lifecycle** — creates, configures, and destroys agents
- **Provider state** — LLM, channel, and tool provider registries
- **TLS termination** — serves a public HTTPS bootstrap listener and a
  dedicated authenticated client listener
- **Multi-agent routing** — routes messages to agents by channel, pattern, or keyword
- **WebSocket protocol** — real-time streaming, health checks, remote tool dispatch
- **A2A protocol** — agent-to-agent communication via JSON-RPC
- **Cron scheduler** — timed jobs with redb persistence

### Agents

Each agent (`rockbot-agent`) runs an iterative tool-use loop:

1. Assemble system prompt from context files (SOUL.md, AGENTS.md, MEMORY.md)
2. Send conversation to LLM provider
3. If the LLM requests tool calls, execute them (with security checks)
4. Loop back to step 2 with tool results until the LLM produces a text response

Agents support planning modes, reflection passes, guardrail pipelines,
trajectory recording, and handoff delegation to other agents.

### Credentials

The credential vault (`rockbot-credentials`) provides defense in depth:

1. **Encryption at rest** — AES-256-GCM
2. **Key derivation** — Argon2id prevents brute-force
3. **Capability system** — tools can only access what's explicitly allowed
4. **HIL approval** — sensitive operations require human consent
5. **Audit trail** — hash-chained tamper-evident logs

Credentials never cross the agent boundary. They are injected into tool
execution and sanitized from responses.

### Remote Execution

With the `remote-exec` feature, interactive clients can register as remote
executors over a Noise Protocol encrypted channel. In practice this is used by
the TUI and by `rockbot agent run --exec`. The gateway dispatches tool calls
(file reads, shell commands, etc.) to the client's local machine, enabling
agents to work on remote workstations.

### Multi-Agent Orchestration

- **Handoffs** — agents delegate to other agents mid-conversation
- **Swarm blackboard** — shared key-value store for agent coordination
- **Graph workflows** — DAG-based execution with parallel fan-out

## Data Flow

### Message Processing

```
User sends message (via TUI WebSocket or HTTP POST)
  │
  ▼
Gateway receives request
  │
  ▼
Routing engine selects agent
  │
  ▼
Session manager loads/creates session
  │
  ▼
Agent processes message (iterative tool loop)
  │
  ├──► Tool call?
  │      │
  │      ▼
  │    Security check (capabilities)
  │      │
  │      ▼
  │    Local or remote execution
  │      │
  │      ▼
  │    Results fed back to LLM
  │
  ▼
Response streamed back via WebSocket
Session updated with new messages
```

### TLS and Connection Security

By default, the gateway serves a public HTTPS bootstrap listener and a
dedicated client listener. `rockbot config init gateway` generates a
self-signed certificate for quick bootstrap. For production use, the built-in
PKI system (`rockbot-pki`) provides a full certificate authority:

- **CA management** — `rockbot cert ca generate` creates a local CA
- **Client certificates** — issued per role (gateway, agent, tui)
- **Mutual TLS** — the dedicated client listener can enforce mTLS without
  blocking browser bootstrap or enrollment on the public listener
- **Public bootstrap surface** — `/`, `/static/*`, `/health`, `GET /api/cert/ca`,
  and optionally `POST /api/cert/sign`
- **Remote enrollment** — `POST /api/cert/sign` with a pre-shared key lets
  new clients obtain certificates without direct CA access when enabled
- **Revocation** — `rockbot cert client revoke` updates the CRL

The browser app is no longer a public admin SPA. It is a bootstrap shell that
can persist imported client key material locally and prepare for an
authenticated WebSocket session; sensitive application APIs belong on the
authenticated control plane instead of the public listener.

See [PKI and mTLS](pki.md) for full details.

Plain HTTP requires building with the `http-insecure` feature flag.

## Persistence

| Path | Purpose |
|------|---------|
| `~/.config/rockbot/rockbot.toml` | Configuration |
| `~/.config/rockbot/gateway.crt` | TLS certificate (legacy self-signed) |
| `~/.config/rockbot/gateway.key` | TLS private key (legacy self-signed) |
| `~/.config/rockbot/pki/` | PKI directory (CA, certs, keys, index, CRL) |
| `~/.config/rockbot/agents/{id}/` | Per-agent context files |
| `~/.config/rockbot/data/sessions.redb` | Session history |
| `~/.config/rockbot/data/cron.redb` | Cron jobs |
| `~/.local/share/rockbot/credentials/` | Encrypted credential vault |
