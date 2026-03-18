# Security Model

RockBot's security architecture provides defense-in-depth: encrypted
credential storage, capability-based access control, mutual TLS for
transport, and human-in-the-loop approval for sensitive operations.

## Trust Boundaries

```
┌──────────────────────────────────────────────────────────────┐
│                        Network                               │
│                                                              │
│   ┌──────────┐    mTLS     ┌─────────────────────────┐      │
│   │ TUI/     │◄───────────►│     Gateway             │      │
│   │ Clients  │  client     │  ┌─────────────────┐    │      │
│   └──────────┘  certs      │  │  Agent Engine    │    │      │
│                             │  │  ┌───────────┐  │    │      │
│                             │  │  │ Tool Loop │  │    │      │
│                             │  │  └─────┬─────┘  │    │      │
│                             │  └────────┼────────┘    │      │
│                             │           │             │      │
│                             │  ┌────────▼────────┐    │      │
│                             │  │ Credential Vault │   │      │
│                             │  │ (AES-256-GCM)    │   │      │
│                             │  └─────────────────┘    │      │
│                             └─────────────────────────┘      │
│                                        │                     │
│                                        │ API calls           │
│                                        ▼                     │
│                              ┌──────────────────┐            │
│                              │   LLM Providers  │            │
│                              └──────────────────┘            │
└──────────────────────────────────────────────────────────────┘
```

### Boundary 1: Transport and Listener Separation

The gateway separates public bootstrap traffic from the authenticated control
plane:

- **Public HTTPS listener** — `/`, `/static/*`, `/health`, `GET /api/cert/ca`,
  and optional enrollment
- **Client listener** — authenticated WebSocket control plane, remote exec,
  native chat traffic, and mTLS client sessions

All gateway communication uses TLS. On the client listener, the built-in PKI
system provides mutual TLS so both sides verify identity:

- **Gateway** presents its server certificate (gateway role)
- **Clients** present their client certificates (agent/tui role)
- Both are signed by the same CA and verified via `WebPkiClientVerifier`
- Revoked certificates are tracked in the CRL

The browser-facing bootstrap shell is intentionally not the place for sensitive
REST management APIs. See [PKI and mTLS](pki.md) for certificate management
details.

### Boundary 2: Credential Isolation

Credentials never cross the agent boundary:

1. Secrets are encrypted at rest with **AES-256-GCM**
2. Key derivation uses **Argon2id** (resists brute-force and side-channel attacks)
3. The vault is locked by default — must be explicitly unlocked
4. Tools access credentials through a **capability-scoped accessor**
5. Credential values are **sanitized from agent responses** before display

```
Agent                    Tool                      Vault
  │                       │                          │
  │  tool_call(params)    │                          │
  ├──────────────────────►│                          │
  │                       │  get_credential(scope)   │
  │                       ├─────────────────────────►│
  │                       │         secret           │
  │                       │◄─────────────────────────┤
  │                       │                          │
  │  result (sanitized)   │                          │
  │◄──────────────────────┤                          │
```

### Boundary 3: Capability System

Each tool execution runs within a `SecurityContext` that restricts:

| Capability | Controls |
|-----------|----------|
| `FilesystemRead` | Which paths can be read |
| `FilesystemWrite` | Which paths can be written |
| `ProcessExecute` | Which commands can be run |
| `NetworkAccess` | Which domains can be contacted |

Capabilities are configured in `[security.capabilities]` in `rockbot.toml`.

### Boundary 4: Human-in-the-Loop (HIL)

Multiple layers of human approval:

1. **Tool-level HIL** — tools declare `requires_approval()` in their trait impl
2. **Breakpoint tools** — per-agent `breakpoint_tools` config forces approval
3. **Command allowlist** — `command_allowlist` on `ToolExecutionContext`
4. **Credential permissions** — `AllowHIL` permission level requires approval per-access
5. **API approval** — `POST /api/agents/{id}/approve` to approve pending calls

## Credential Permission Model

```
┌─────────────────────────────────────────────────┐
│               Permission Rules                   │
│                                                  │
│  Rule: Allow   "homeassistant://api/**"         │
│  Rule: AllowHIL "aws://iam/**"                  │
│  Rule: Deny    "production://**"                │
│                                                  │
│  Pattern matching: glob-style                    │
│  Default: configurable (deny recommended)        │
└─────────────────────────────────────────────────┘
```

| Level | Behavior |
|-------|----------|
| `Allow` | Immediate grant, no user interaction |
| `AllowHIL` | Queued for human approval before granting |
| `Deny` | Blocked unconditionally |

## Audit Trail

The credential vault maintains a **hash-chained audit log**:

- Every operation (read, write, delete, permission change) is logged
- Each entry includes a SHA-256 hash of the previous entry
- Tampering with any entry breaks the chain
- Verify integrity: `rockbot credentials audit --verify`

## Sandbox Enforcement

Tool execution can be sandboxed at multiple levels:

| Mode | Description |
|------|-------------|
| `disabled` | No sandboxing (default) |
| `tools` | Tool calls run in sandbox |
| `all` | All agent operations sandboxed |

Enforcement functions in `rockbot-security`:
- `enforce_path()` — validates filesystem access against allowed paths
- `enforce_executable()` — checks command against allowlist
- `enforce_timeout()` — caps execution time

Container sandbox (`sandbox.rs`) provides Docker-based isolation with
`--network none` for fully air-gapped execution.

## Guardrails

Agent-level guardrails run before message processing:

| Guardrail | Detection |
|-----------|-----------|
| `PiiGuardrail` | Regex-based PII detection (SSN, email, phone, credit card) |
| `PromptInjectionGuardrail` | Pattern matching for injection attempts |

Configured per-agent via `guardrails = ["pii", "prompt_injection"]`.

## Overseer (Optional)

With the `overseer` feature, an embedded local language model reviews
agent actions before execution:

- Runs inference locally (no data leaves the machine)
- Produces `OverseerVerdict` (Allow/Warn/Deny) with reasoning
- Maintains a `DecisionLog` for audit
- Configurable trust levels per agent

## Configuration

See [Configuration Reference](../user-guide/configuration.md) for all
security-related config fields (`[security]`, `[credentials]`,
`[gateway]` TLS/mTLS settings).
