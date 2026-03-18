# Code Review Round 2 Remediation Plan

## Purpose

This document converts the findings in [`CODE_REVIEW.md`](../../CODE_REVIEW.md)
into an implementation plan with explicit execution order.

The review mixes:

- exploitable security defects
- correctness bugs introduced by recent refactors
- architectural hardening gaps
- test and quality debt

The order here is intentionally not the same as the review order. Security and
trust-boundary failures come first, then correctness regressions, then
structural cleanup.

## Validated Findings

The following findings were spot-checked against the current tree and should be
treated as real:

1. Browser auth signature verification is mismatched.
   - Server verifies with `ECDSA_P256_SHA256_ASN1` in
     [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
   - Browser signs with WebCrypto ECDSA in
     [lib.rs](/home/kelsea/Projects/RockBot/crates/rockbot-webui/src/lib.rs)
   - WebCrypto returns fixed-width P1363 encoding, not ASN.1 DER.

2. Command injection via test/lint filter remains open.
   - [builtin.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tools/src/builtin.rs)

3. TLS verification is still disabled for the native client.
   - `AcceptAnyCert` in
     [client.rs](/home/kelsea/Projects/RockBot/crates/rockbot-client/src/client.rs)

4. Vault unlock still generates a fresh salt per request.
   - [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

5. `agent_id` path handling is still not validated before filesystem joins.
   - [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

6. The config test break is real.
   - `GatewayConfig` initializer is missing `public` in
     [config.rs](/home/kelsea/Projects/RockBot/crates/rockbot-config/src/config.rs)

## Priority Model

### P0

Remote compromise, authentication bypass, arbitrary file write, or broken trust.

### P1

Major correctness failures and availability regressions affecting normal use.

### P2

Operational hardening, boundary cleanup, and exploitability reduction.

### P3

Quality debt, stale code, and maintainability cleanup.

## Phase 0: Immediate Security Fixes

These should land before further feature work.

### 0.1 Browser auth correctness and reset hygiene

Scope:

- switch server verification from ASN.1 to fixed-width ECDSA for WebCrypto
- reset stale `cert_name` and `cert_role` on new auth attempts
- ensure auth failure leaves no prior identity attached to the connection

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
- [lib.rs](/home/kelsea/Projects/RockBot/crates/rockbot-webui/src/lib.rs)

Acceptance:

- browser auth succeeds with a valid imported keypair
- repeated failed auth attempts do not preserve prior identity

### 0.2 Remove command injection from test/lint helper paths

Scope:

- stop interpolating untrusted `filter` values into shell command strings
- use structured argument assembly per language/runtime
- validate filters when they must be passed through

Files:

- [builtin.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tools/src/builtin.rs)

Acceptance:

- no `sh -c` construction from free-form test filter input
- add unit tests for malicious filter payloads

### 0.3 Restore TLS trust verification for clients

Scope:

- remove `AcceptAnyCert`
- load and trust `[pki].tls_ca`
- fail closed when TLS is configured but CA trust cannot be loaded

Files:

- [client.rs](/home/kelsea/Projects/RockBot/crates/rockbot-client/src/client.rs)

Acceptance:

- valid gateway cert signed by configured CA connects
- untrusted or mismatched certs fail

### 0.4 Fix vault unlock salt handling

Scope:

- stop generating a fresh salt during unlock
- load the persisted vault salt / metadata instead
- add regression tests for password unlock roundtrips

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
- vault storage/manager paths as needed

Acceptance:

- password unlock works across restarts
- wrong password fails deterministically

### 0.5 Block arbitrary path writes on gateway-managed file creation

Scope:

- validate `agent_id`
- validate `sign_req.name`
- validate credential-init `keyfile_path` or remove caller-controlled path support

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

Acceptance:

- path traversal inputs are rejected
- gateway only writes within intended directories

## Phase 1: Public and Internal Boundary Hardening

### 1.1 Finish removal of public sensitive APIs

The public listener was reduced, but this should be completed and tested as an
explicit boundary.

Scope:

- verify no sensitive CRUD/data endpoints remain reachable on the public port
- document the public surface as policy, not convention

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
- [api.md](/home/kelsea/Projects/RockBot/docs/api.md)
- [security.md](/home/kelsea/Projects/RockBot/docs/architecture/security.md)

### 1.2 Body size limits for both HTTP and WS API tunnel

Scope:

- remove `collect().await.unwrap()`
- impose request size ceilings on HTTP request bodies
- impose equivalent ceilings on `api_request` over WebSocket

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

Acceptance:

- oversized payloads are rejected cleanly
- no panic path remains

### 1.3 Browser key storage hardening

Current IndexedDB storage is plaintext PEM. That is acceptable only as a
temporary bootstrap convenience.

Scope:

- move toward non-extractable `CryptoKey` storage where possible
- if PEM persistence remains temporarily, label it as transitional and add
  explicit “forget identity” UX
- add CSP and security headers before relying on browser-held keys

Files:

- [lib.rs](/home/kelsea/Projects/RockBot/crates/rockbot-webui/src/lib.rs)
- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

### 1.4 HTTP security headers

Scope:

- `Content-Security-Policy`
- `X-Frame-Options`
- `X-Content-Type-Options`
- `Referrer-Policy`
- `Strict-Transport-Security` when appropriate

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

## Phase 2: Remote Execution and Streaming Correctness

### 2.1 Restore complete tool lifecycle signaling

Scope:

- emit `ToolDone` consistently from agent paths
- verify gateway forwarding and TUI consumption
- fix status-bar / completion-state drift

Files:

- [agent.rs](/home/kelsea/Projects/RockBot/crates/rockbot-agent/src/agent.rs)
- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
- [app.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tui/src/app.rs)

### 2.2 Fix streaming fallback consistency

Scope:

- ensure non-streaming fallback still emits text to stream consumers
- align main chat path and tool-loop path behavior

Files:

- [agent.rs](/home/kelsea/Projects/RockBot/crates/rockbot-agent/src/agent.rs)

### 2.3 Reintroduce bounded tool output handling

Scope:

- cap or chunk `tool_output` and `tool_result` transport payloads
- prevent WebSocket flooding and TUI spam

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)
- [client.rs](/home/kelsea/Projects/RockBot/crates/rockbot-client/src/client.rs)
- [app.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tui/src/app.rs)

### 2.4 Identity spoofing on WS client registration

Scope:

- stop trusting arbitrary `client_uuid`, `hostname`, and `label`
- bind identity to authenticated transport where possible
- at minimum separate display labels from dispatch identity

Files:

- [gateway.rs](/home/kelsea/Projects/RockBot/crates/rockbot-gateway/src/gateway.rs)

## Phase 3: Tool and Network Surface Hardening

### 3.1 SSRF protections for `web_fetch`

Scope:

- reject localhost, link-local, metadata service, RFC1918, and other protected
  destinations by policy
- optionally introduce allowlist mode

Files:

- [builtin.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tools/src/builtin.rs)
- security config docs

### 3.2 Absolute path and alias handling for file tools

Scope:

- align path enforcement across `path`, `file`, and `file_path`
- stop absolute path escape from bypassing workspace constraints

Files:

- [builtin.rs](/home/kelsea/Projects/RockBot/crates/rockbot-tools/src/builtin.rs)

### 3.3 `tools-mcp` feature propagation

Scope:

- ensure gateway feature enables corresponding agent feature
- add feature-level regression check

Files:

- workspace and crate `Cargo.toml` files

## Phase 4: Availability and Concurrency Hardening

### 4.1 Bounded WS task fanout

Scope:

- stop spawning an unbounded task per inbound WS message
- introduce a bounded queue or semaphore

### 4.2 Noise handshake cleanup

Scope:

- add TTL / disconnect cleanup for handshake state maps

### 4.3 SSE cancellation and expensive abandoned work

Scope:

- cancel or short-circuit agent execution when SSE clients disconnect

### 4.4 TOCTOU on primary-agent creation

Scope:

- make primary-agent assignment atomic under concurrent creation

## Phase 5: Test and Quality Recovery

### 5.1 Fix broken tests and examples

Scope:

- fix missing `public` field in config tests
- remove or repair [demo.rs](/home/kelsea/Projects/RockBot/examples/demo.rs)

### 5.2 Remove stale compatibility/dead code

Scope:

- replace `atty`
- address dead-code allowances where no longer justified
- collapse the duplicate/stub TUI credentials implementation
- finish or remove stale CLI credential TODOs

### 5.3 Clippy debt reduction

Scope:

- drive first-party warnings down crate by crate
- start with `rockbot-tui`, then `rockbot-cli`

## Recommended Execution Order

1. Browser auth fix and stale state reset
2. Command injection removal
3. TLS verification restoration
4. Vault salt fix
5. Path write/traversal fixes
6. Body size limits
7. Tool lifecycle and streaming correctness
8. Browser key storage + security headers
9. SSRF and file-tool hardening
10. WS concurrency / Noise cleanup
11. Test/example/clippy cleanup

## Non-Goals for This Pass

These are important, but they should not block the high-risk fixes above:

- broad large-function refactors
- response-builder style cleanup
- uptime/memory cosmetic health fields
- generic dead-code cleanup not tied to correctness or exposure

## Success Criteria

This review is considered remediated when:

- no known P0 issue remains exploitable
- browser auth succeeds correctly and no longer stores raw PEM by default
- client TLS trust is enforced
- gateway body and WS API size limits exist
- remote tool lifecycle and streaming regressions are resolved
- tests are green for the touched crates
