# API Reference

The gateway exposes a minimal public HTTP surface plus an authenticated
WebSocket control plane. All endpoints serve JSON unless otherwise noted.

When TLS is configured (default), use `https://` and `wss://` schemes.

## Public HTTP Endpoints

### Health and Status

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Health check for monitoring |
| GET | `/` | Browser bootstrap shell |
| GET | `/static/app.css` | Embedded web bootstrap stylesheet |
| GET | `/static/app.js` | Embedded web bootstrap application |

### Certificates (PKI)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/cert/ca` | Get the CA certificate PEM (public, optional) |
| POST | `/api/cert/sign` | Sign a CSR with PSK authentication (optional enrollment) |

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

Requires `pki.enrollment_psk` to be set in gateway config, or an enrollment
token created via `rockbot cert enroll create`, and
`gateway.public.enrollment_enabled = true`.

## WebSocket Protocol

Native clients connect to `wss://host:client_port/ws` (or `ws://` with
`http-insecure`).

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
| `api_request` | Tunnel a management/data request over the authenticated WS control plane |

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
| `api_response` | Response to a tunneled WS API request |

## Rust API Documentation

Generate from source:

```bash
cargo doc --open --no-deps
```
