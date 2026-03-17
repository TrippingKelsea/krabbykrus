# API Reference

The gateway exposes HTTP REST and WebSocket endpoints. All endpoints serve
JSON unless otherwise noted.

When TLS is configured (default), use `https://` and `wss://` schemes.

## HTTP Endpoints

### Health and Status

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Health check (returns `{"status":"ok"}`) |
| GET | `/api/status` | Gateway status (version, uptime, connections, agents) |
| GET | `/api/executors` | List connected remote executors and their advertised workdirs/capabilities |
| GET | `/api/metrics` | Prometheus-style metrics |

### Agents

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/agents` | List agents (model, status, session count) |
| POST | `/api/agents` | Create agent |
| PUT | `/api/agents/:id` | Update agent config |
| DELETE | `/api/agents/:id` | Delete agent |
| POST | `/api/agents/:id/message` | Send message (synchronous response) |
| POST | `/api/agents/:id/stream` | Send message (SSE streaming response) |
| POST | `/api/agents/:id/approve` | Approve a pending HIL tool call |
| GET | `/api/agents/:id/files` | List agent context files |
| GET | `/api/agents/:id/files/:name` | Read a context file |
| PUT | `/api/agents/:id/files/:name` | Write a context file |
| DELETE | `/api/agents/:id/files/:name` | Delete a context file |
| GET | `/api/agents/:id/sessions/:sid/export` | Export session as JSON |

### Providers

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/providers` | List registered LLM providers |
| POST | `/api/providers/:id/test` | Test provider connectivity |
| POST | `/api/chat` | Route a chat completion through the gateway |

### Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/sessions` | List sessions |
| POST | `/api/sessions` | Create session |
| GET | `/api/sessions/:key/messages` | Get message history |
| DELETE | `/api/sessions/:key` | Delete session |

### Credentials

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/credentials/status` | Vault status |
| GET | `/api/credentials/schemas` | Dynamic credential schemas from all plugins |
| GET | `/api/credentials` | List credential endpoints |
| POST | `/api/credentials` | Create credential endpoint |
| POST | `/api/credentials/init` | Initialize vault |
| POST | `/api/credentials/unlock` | Unlock vault |
| POST | `/api/credentials/lock` | Lock vault |
| GET | `/api/credentials/permissions` | List permission rules |
| POST | `/api/credentials/permissions` | Add permission rule |
| GET | `/api/credentials/audit` | View audit log |
| GET | `/api/credentials/approvals` | Pending HIL approvals |
| POST | `/api/credentials/approvals/:id/approve` | Approve HIL request |
| POST | `/api/credentials/approvals/:id/deny` | Deny HIL request |

### Cron Jobs

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/cron/jobs` | List cron jobs |
| POST | `/api/cron/jobs` | Create cron job |
| GET | `/api/cron/jobs/:id` | Get job details |
| PUT | `/api/cron/jobs/:id` | Update job |
| DELETE | `/api/cron/jobs/:id` | Delete job |
| POST | `/api/cron/jobs/:id/trigger` | Trigger job immediately |
| GET | `/api/cron/clients` | List connected clients for dispatch |

### Certificates (PKI)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/cert/ca` | Get the CA certificate PEM (public) |
| POST | `/api/cert/sign` | Sign a CSR with PSK authentication (enrollment) |

**`POST /api/cert/sign`** request body:

```json
{
  "csr": "-----BEGIN CERTIFICATE REQUEST-----\n...",
  "psk": "enrollment-token-string",
  "name": "my-agent",
  "role": "agent"
}
```

Response (200):

```json
{
  "certificate": "-----BEGIN CERTIFICATE-----\n...",
  "ca_certificate": "-----BEGIN CERTIFICATE-----\n..."
}
```

Requires `enrollment_psk` to be set in gateway config, or an enrollment
token created via `rockbot cert enroll create`.

### A2A (Agent-to-Agent)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/.well-known/agent.json` | Agent card (A2A discovery) |
| POST | `/a2a` | JSON-RPC dispatch (tasks/send, tasks/get, etc.) |

### Web UI

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/` | Web dashboard (embedded SPA) |

## WebSocket Protocol

Connect to `wss://host:port/ws` (or `ws://` with `http-insecure`).

All messages are JSON with a `"type"` field.

### Client → Server

| Type | Description |
|------|-------------|
| `agent_message` | Send message to an agent (`agent_id`, `session_key`, `message`, optional `workspace`, `executor_target`, `allow_active_client_tools`) |
| `health_check` | Request health status |
| `ping` | Keepalive |
| `client_identify` | Identify client with a label (for cron dispatch) |
| `noise_handshake` | Noise Protocol handshake step (remote-exec) |
| `remote_capabilities` | Advertise remote execution capabilities |
| `remote_tool_response` | Return result of a remote tool execution |

### Server → Client

| Type | Description |
|------|-------------|
| `stream_chunk` | Incremental streaming text delta |
| `agent_response` | Final complete response |
| `agent_error` | Processing error |
| `tool_call` | Tool execution started (`locality` may be `gateway`, `active_client`, or `remote:<target>`) |
| `tool_result` | Tool execution completed (`locality` mirrors the execution site) |
| `client_identity_assigned` | Confirms the gateway-side client UUID/label after `client_identify` |
| `token_usage` | Token usage update |
| `thinking_status` | Agent processing phase update |
| `health_status` | Health status response |
| `pong` | Keepalive response |
| `error` | Protocol-level error |
| `noise_handshake` | Noise Protocol handshake step |
| `remote_capabilities_ack` | Capabilities registration result |
| `remote_tool_request` | Request to execute a tool locally |
| `cron_dispatch` | Cron job dispatch to targeted client |

## Rust API Documentation

Generate from source:

```bash
cargo doc --open --no-deps
```
