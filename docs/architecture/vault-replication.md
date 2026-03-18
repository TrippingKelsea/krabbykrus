# Vault Replication over Noise Protocol

> **Status:** Draft (storage layer implemented, replication protocol in progress)
> **Last updated:** 2026-03-15

## Overview

RockBot nodes (gateways, agents, TUI clients) establish encrypted peer-to-peer
links using the [Noise Protocol Framework](https://noiseprotocol.org/). Today
these links carry remote tool execution payloads. This document proposes
extending them to replicate the PKI vault — certificate index, CRL, credentials,
and configuration — across nodes in a mesh, eliminating the need for a central
coordinator while maintaining strong consistency guarantees for security-critical
state.

### Design Inspiration

This design draws from:

- **[Nebula](https://github.com/slackhq/nebula)** — Certificate-based identity
  and authorization. Roles and groups embedded in x.509 extensions make the cert
  the single source of truth.
- **HashiCorp Vault Raft** — Replicated secret storage with leader-based
  consensus.
- **WireGuard** — Noise IK pattern for authenticated key exchange with known
  static keys.

## Architecture

```
┌──────────────┐     Noise XX      ┌──────────────┐
│   Gateway A  │◄─────────────────►│   Gateway B  │
│  (CA holder) │                   │  (replica)   │
│              │     Noise XX      │              │
│  PKI Vault   │◄─────────────────►│  PKI Vault   │
│  Credentials │                   │  Credentials │
└──────┬───────┘                   └──────┬───────┘
       │           Noise XX               │
       └──────────────┬──────────────────┘
                      │
               ┌──────▼───────┐
               │   Agent C    │
               │  (leaf node) │
               │  Local cache │
               └──────────────┘
```

### Node Roles

| Role | Capabilities | Replication Behavior |
|------|-------------|---------------------|
| **CA primary** | Full PKI write (issue, revoke, CRL) | Source of truth; pushes updates |
| **CA replica** | PKI read, credential read/write | Receives PKI state; can proxy cert requests to primary |
| **Leaf node** | Credential read, config read | Receives subset relevant to its identity |

Node roles are determined by the x.509 extensions in their certificate:
- `roles: ["ca-primary"]` — full CA authority
- `roles: ["ca-replica"]` — read-only PKI replica
- `roles: ["agent"]`, `roles: ["tui"]` — leaf nodes

## Transport Layer

### Noise Protocol Selection

**Current:** `Noise_XX_25519_ChaChaPoly_SHA256` — mutual authentication, no
pre-shared static keys. Both sides prove identity during the 3-message handshake.

**Proposed for replication:** `Noise_IK_25519_ChaChaPoly_SHA256` — the initiator
knows the responder's static public key (from the PKI index or a pinned
configuration). This provides:

1. **One fewer round trip** (2 messages vs 3)
2. **Identity hiding for the initiator** — the initiator's static key is
   encrypted under the responder's key in the first message
3. **Replay protection** via ephemeral keys in every handshake

For the initial bootstrap (before any static keys are known), fall back to
`Noise_XX` as today.

### Key Persistence

The gateway's Noise static keypair must be persisted to the PKI directory:

```
~/.config/rockbot/pki/
├── noise_static.key    # X25519 private key (0600)
├── noise_static.pub    # X25519 public key
└── known_peers.json    # Pinned peer static public keys
```

The `known_peers.json` file maps node names to their static public keys,
enabling `Noise_IK` handshakes and detecting key changes (trust-on-first-use
or CA-signed key attestation).

### Wire Format

All replication messages are framed as Noise transport messages over the
existing WebSocket connection. The payload inside each Noise frame is a
length-prefixed MessagePack (or CBOR) envelope:

```
┌─────────────────────────────────────────────┐
│ WS Binary Frame                             │
│ ┌─────────────────────────────────────────┐ │
│ │ Noise Transport Message (encrypted)     │ │
│ │ ┌─────────────────────────────────────┐ │ │
│ │ │ u32 LE: payload length              │ │ │
│ │ │ msgpack envelope {                  │ │ │
│ │ │   "type": "vault_sync",            │ │ │
│ │ │   "seq": 42,                       │ │ │
│ │ │   "payload": <type-specific>       │ │ │
│ │ │ }                                   │ │ │
│ │ └─────────────────────────────────────┘ │ │
│ └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## What Gets Replicated

### PKI State

| Data | Direction | Frequency | Conflict Resolution |
|------|-----------|-----------|-------------------|
| `index.json` (cert entries) | Primary → Replicas | On every issue/revoke | Serial number ordering; primary wins |
| `ca.crt` | Primary → Replicas | On CA rotation | Version counter; primary wins |
| `crl.pem` | Primary → Replicas | On every revocation | Timestamp-based; latest wins |
| Enrollment tokens | Primary → Replicas | On create/consume | Primary is authoritative |

The CA private key (`ca.key`) is **never replicated**. Only the primary holds
it. Replicas that need to issue certificates must proxy the request to the
primary over the Noise link.

### Credentials Vault

| Data | Direction | Frequency | Conflict Resolution |
|------|-----------|-----------|-------------------|
| Credential entries | Bidirectional | On write | Last-writer-wins with Lamport timestamps |
| Credential schemas | Primary → Replicas | On provider registration | Primary authoritative |

Credentials are encrypted at rest and replicated as opaque blobs. The
encryption key is derived from the node's Noise static key + a vault-specific
salt, ensuring credentials are re-encrypted per-node and never stored in
plaintext on the wire or at rest.

### Configuration

| Data | Direction | Frequency | Conflict Resolution |
|------|-----------|-----------|-------------------|
| Agent configs | Primary → Replicas | On change | Primary authoritative |
| Routing rules | Primary → Replicas | On change | Primary authoritative |

## Replication Protocol

### Sync Phases

1. **Handshake** — Noise IK (or XX for bootstrap) over WebSocket
2. **Capability advertisement** — Exchange node role, vault version vector,
   supported sync protocols
3. **Delta sync** — Exchange only changes since the last known common state
4. **Steady-state** — Push-based incremental updates as changes occur

### Version Vectors

Each replicated dataset maintains a version vector:

```json
{
  "pki_index": { "primary": 42, "replica-b": 0 },
  "credentials": { "primary": 18, "replica-b": 12 },
  "config": { "primary": 7 }
}
```

During delta sync, a node sends its version vector; the peer responds with
all entries where its local version exceeds the requesting node's.

### Message Types

| Type | Direction | Purpose |
|------|-----------|---------|
| `vault_hello` | Both | Capability + version vector exchange |
| `vault_delta_request` | Replica → Primary | Request changes since version N |
| `vault_delta_response` | Primary → Replica | Batch of changes with new version |
| `vault_push` | Primary → Replica | Proactive push of new changes |
| `vault_ack` | Replica → Primary | Acknowledge receipt + applied version |
| `vault_cert_request` | Replica → Primary | Proxy a CSR signing request |
| `vault_cert_response` | Primary → Replica | Signed certificate |

### Consistency Model

- **PKI state:** Strongly consistent. The CA primary is the single writer.
  Replicas apply updates in serial-number order and reject out-of-order entries.
- **Credentials:** Eventually consistent with last-writer-wins. Lamport
  timestamps break ties; node ID breaks timestamp ties.
- **Config:** Primary-authoritative. Replicas overwrite local state on sync.

## Security Model

### Threat Model

| Threat | Mitigation |
|--------|-----------|
| **Eavesdropping** | All replication traffic encrypted via Noise transport (ChaChaPoly) |
| **MITM** | Noise IK with pinned static keys; XX only for bootstrap with TOFU |
| **Replay** | Ephemeral keys per session; sequence numbers within a session |
| **Rogue replica** | Certificate-based authorization: only nodes with `ca-replica` role can receive PKI state |
| **CA key theft** | CA key never leaves the primary; replicas proxy signing requests |
| **Credential exfiltration** | Credentials re-encrypted per-node; leaf nodes only receive credentials they're authorized for (group-based filtering) |
| **Split-brain** | Single-writer for PKI (primary); LWW for credentials with manual conflict resolution UI |
| **Compromised node** | Revoke its certificate via CRL; CRL push propagates to all replicas |

### Authorization via Certificate Extensions

After the Noise handshake, the peer's Noise static public key is mapped to a
certificate in the PKI index (via a key attestation extension or out-of-band
binding). The certificate's roles and groups determine what data the peer can
access:

```
roles: ["ca-replica"] → Full PKI + credential sync
roles: ["agent"]      → Own credentials + agent config only
groups: ["us-west-2"] → Credentials scoped to us-west-2 group
```

This mirrors Nebula's approach: the network layer (Noise) handles
authentication, and the certificate extensions handle authorization, with no
external directory service needed.

## Implementation Status

### Storage Layer (Implemented)

The `rockbot-storage` crate provides the unified storage backend:

- **redb** — Embedded key-value database (pure Rust, stable format)
- **ChaCha20 encryption** — Block-level storage encryption via `redb::StorageBackend`
- **10 table definitions** — All persistent state unified in one database
- **Per-table sync policies** — `Eager` (replicate), `Eventual` (on-demand), `LocalOnly`
- **OpenRaft integration** (behind `replication` feature):
  - `RedbLogStore` — Raft log backed by redb tables
  - `RedbStateMachine` — Applies Raft entries as table mutations
  - `NoiseNetwork` — Stub `RaftNetwork` implementation for Noise protocol transport

### Remaining Roadmap

### Phase 1: Foundation
- [ ] Persist Noise static keypair to PKI directory
- [ ] Apply Noise transport encryption to existing WS payloads (close current gap)
- [ ] `known_peers.json` for static key pinning
- [ ] Upgrade remote tool execution to use encrypted payloads

### Phase 2: Raft Cluster Bootstrap
- [x] OpenRaft type config and trait implementations (log store, state machine)
- [ ] Wire `NoiseNetwork` to actual Noise sessions over WebSocket
- [ ] Cluster membership management (add/remove nodes)
- [ ] Leader election and log replication end-to-end

### Phase 3: PKI Replication
- [ ] PKI index replication via Raft entries (primary → replicas)
- [ ] CRL push on revocation
- [ ] CSR signing proxy (replica → primary → replica)

### Phase 4: Credential + Config Replication
- [ ] Per-node credential encryption (Noise static key + salt derivation)
- [ ] Sync policy enforcement (only `Eager` tables enter Raft log)
- [ ] On-demand sync for `Eventual` tables (sessions)
- [ ] Agent config push and hot-reload on receipt

### Phase 5: Hardening
- [ ] Formal Noise IK integration (replace XX after bootstrap)
- [ ] Key attestation extension (bind Noise key to x.509 cert)
- [ ] Automatic failover: replica promotion to primary via quorum
- [ ] Audit log for all replication events

## Open Questions

1. **Quorum for CA failover:** Should we require N-of-M replicas to agree
   before promoting a replica to primary? Or is manual promotion sufficient
   for the initial implementation?

2. **Credential encryption key rotation:** When a node's Noise key is rotated,
   all locally-encrypted credentials need re-encryption. Should this be
   automatic or require operator intervention?

3. **Bandwidth optimization:** For large PKI indexes (thousands of certs),
   should we use Merkle trees for efficient delta detection instead of version
   vectors?

4. **Clock requirements:** Lamport timestamps don't require synchronized clocks,
   but should we add hybrid logical clocks (HLC) for better ordering guarantees
   on credential writes?
