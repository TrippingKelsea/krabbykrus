# PKI and Mutual TLS

RockBot includes a built-in PKI (Public Key Infrastructure) for securing
gateway–client communication with mutual TLS. This ensures that only
authorized clients — TUI instances, agents, and the gateway itself — can
participate in the platform.

## Design Goals

1. **Zero external dependencies** — no external CA or certificate tooling required
2. **mTLS by default** — both sides verify identity, not just the server
3. **Role-based certificates** — gateway, agent, and TUI certs carry distinct EKUs
4. **Enrollment without manual CA access** — pre-shared key bootstrapping
5. **Revocation support** — CRL generation on revoke
6. **Hardware key readiness** — `KeyBackend` trait abstracts key storage

## Architecture

```
┌─────────────────────────────────────────────┐
│                  rockbot-pki                 │
│                                             │
│  ┌───────────┐  ┌──────────┐  ┌──────────┐ │
│  │ PkiManager│──│  ca.rs   │──│ index.rs │ │
│  │ (manager) │  │ (crypto) │  │ (registry)│ │
│  └─────┬─────┘  └──────────┘  └──────────┘ │
│        │                                    │
│  ┌─────▼─────┐                              │
│  │KeyBackend │  FileBackend (today)         │
│  │  (trait)  │  PKCS#11 / YubiKey (future)  │
│  └───────────┘                              │
└─────────────────────────────────────────────┘
         │                    │
    ┌────▼────┐         ┌────▼──────┐
    │ CLI     │         │ Gateway   │
    │ cert *  │         │ mTLS +    │
    │ commands│         │ /api/cert │
    └─────────┘         └───────────┘
```

### Crate: `rockbot-pki`

| Module | Purpose |
|--------|---------|
| `backend.rs` | `KeyBackend` trait, `FileBackend` (PEM on disk, 0600 perms), `KeyHandle` enum |
| `ca.rs` | CA generation, client cert signing, CSR signing, CRL generation, SHA-256 fingerprints |
| `index.rs` | `PkiIndex` (JSON registry), `CertEntry`, `CertRole`, `CertStatus`, `EnrollmentToken` |
| `manager.rs` | `PkiManager` high-level orchestrator — ties backend, CA, and index together |

## Certificate Roles

| Role | EKU | Purpose |
|------|-----|---------|
| `gateway` | ServerAuth + ClientAuth | Gateway TLS server certificate; also acts as a client in peer scenarios |
| `agent` | ClientAuth | Agent connecting to the gateway |
| `tui` | ClientAuth | TUI client connecting to the gateway |

The gateway is itself "just a special client" — it uses a client cert with
the `gateway` role, which carries both server and client auth EKUs.

## PKI Directory Layout

```
~/.config/rockbot/pki/
├── ca.crt              CA certificate (PEM)
├── ca.key              CA private key (PEM, 0600) — in keys/ via FileBackend
├── index.json          Certificate registry
├── crl.pem             Current CRL (regenerated on revocation)
├── certs/              Issued leaf certificates (<name>.crt)
└── keys/               Leaf private keys (<name>.key, 0600)
```

The `index.json` file tracks all issued certificates:

```json
{
  "next_serial": 4,
  "entries": [
    {
      "serial": 1,
      "name": "gateway",
      "role": "gateway",
      "status": "active",
      "not_before": "2026-03-15T00:00:00Z",
      "not_after": "2027-03-15T00:00:00Z",
      "fingerprint_sha256": "AA:BB:CC:...",
      "subject": "CN=gateway,O=RockBot",
      "sans": ["localhost", "127.0.0.1"]
    }
  ],
  "enrollments": []
}
```

## Configuration

### Gateway Config (`rockbot.toml`)

```toml
[gateway]
bind_host = "0.0.0.0"
port = 18080

# TLS certificate and key (gateway cert)
tls_cert = "/home/you/.config/rockbot/pki/certs/gateway.crt"
tls_key  = "/home/you/.config/rockbot/pki/keys/gateway.key"

# CA certificate — enables client certificate verification
tls_ca = "/home/you/.config/rockbot/pki/ca.crt"

# Require valid client cert (mandatory mTLS)
# false + tls_ca set = optional client auth (accepts but doesn't require)
# true  + tls_ca set = mandatory mTLS (rejects unauthenticated connections)
require_client_cert = true

# PKI directory (for enrollment endpoint and cert management)
pki_dir = "/home/you/.config/rockbot/pki"

# Pre-shared key for remote CSR enrollment (optional)
# If set, enables POST /api/cert/sign with PSK authentication
enrollment_psk = "some-secret-token"
```

### Field Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tls_cert` | `Option<PathBuf>` | None | Path to gateway TLS certificate (PEM) |
| `tls_key` | `Option<PathBuf>` | None | Path to gateway TLS private key (PEM) |
| `tls_ca` | `Option<PathBuf>` | None | Path to CA certificate for client verification |
| `require_client_cert` | `bool` | `false` | Enforce mandatory client certificates |
| `pki_dir` | `Option<PathBuf>` | None | Path to PKI directory |
| `enrollment_psk` | `Option<String>` | None | Pre-shared key for `/api/cert/sign` enrollment |

### mTLS Modes

| `tls_ca` | `require_client_cert` | Behavior |
|----------|-----------------------|----------|
| unset | false | Standard TLS (server auth only) |
| set | false | Optional mTLS — clients may present certs, but aren't required to |
| set | true | Mandatory mTLS — connections without valid client certs are rejected |

## CLI Commands

### CA Management

```bash
# Initialize a new Certificate Authority
rockbot cert ca generate --days 3650

# View CA information
rockbot cert ca info

# Rotate CA (backup + regenerate)
rockbot cert ca rotate --days 3650 --backup
```

### Client Certificates

```bash
# Generate a gateway certificate
rockbot cert client generate --name gateway --role gateway \
  --san localhost --san 127.0.0.1 --days 365

# Generate an agent certificate
rockbot cert client generate --name my-agent --role agent --days 365

# Generate a TUI certificate
rockbot cert client generate --name my-tui --role tui --days 365

# List all certificates
rockbot cert client list

# Show certificate details
rockbot cert client info --name gateway

# Revoke a certificate (regenerates CRL)
rockbot cert client revoke --name compromised-agent

# Rotate a certificate (revoke + reissue)
rockbot cert client rotate --name gateway \
  --san localhost --san 127.0.0.1 --days 365 --backup
```

### Install Into Config

```bash
# Patch rockbot.toml with certificate paths
rockbot cert install --name gateway
```

This writes `tls_cert`, `tls_key`, `tls_ca`, and `pki_dir` into the
`[gateway]` section of `rockbot.toml`. For gateway-role certs, it also
sets `require_client_cert = true`.

### CSR Signing (Offline)

```bash
# Sign an externally-generated CSR
rockbot cert sign --csr /path/to/request.csr \
  --name external-svc --role agent --days 365 \
  --output /path/to/signed.crt
```

### Certificate Inspection

```bash
# Inspect any PEM certificate
rockbot cert info --cert /path/to/cert.pem

# Verify cert/key match and chain
rockbot cert verify --cert gateway.crt --key gateway.key --ca ca.crt
```

### Remote Enrollment

Enrollment tokens allow clients to obtain certificates without direct
CA access — useful for bootstrapping remote agents and TUIs.

```bash
# On the CA host: create a limited-use enrollment token
rockbot cert enroll create --role agent --uses 1 --expires 24h

# On the client: enroll with the gateway
rockbot cert enroll submit \
  --gateway https://gateway.example.com:18080 \
  --psk <token> --name my-agent --role agent

# List active enrollment tokens
rockbot cert enroll list

# Revoke an enrollment token
rockbot cert enroll revoke --id <token-id>
```

The enrollment flow:

1. Admin creates an enrollment token on the CA host
2. Token is shared with the new client (out-of-band)
3. Client runs `cert enroll submit`, which:
   - Generates a local key pair
   - Creates a CSR
   - POSTs the CSR + token to `POST /api/cert/sign`
   - Saves the returned signed certificate and CA cert
4. Client runs `cert install` to patch their config

## Gateway mTLS Enforcement

When `tls_ca` is configured, the gateway builds a `rustls`
`WebPkiClientVerifier` from the CA certificate:

- **`require_client_cert = true`**: `WebPkiClientVerifier::builder(root_store).build()`
  — rejects any TLS handshake without a valid client cert
- **`require_client_cert = false`**: `WebPkiClientVerifier::builder(root_store).allow_unauthenticated().build()`
  — accepts connections with or without client certs

The gateway also:
- Serves `GET /api/cert/ca` — returns the CA certificate PEM (public)
- Serves `POST /api/cert/sign` — PSK-authenticated CSR signing for enrollment

## KeyBackend Trait

```rust
pub trait KeyBackend: Send + Sync {
    fn name(&self) -> &str;
    fn generate(&self, label: &str) -> anyhow::Result<KeyHandle>;
    fn load(&self, path: &Path) -> anyhow::Result<KeyHandle>;
}
```

The `FileBackend` stores PEM-encoded ECDSA keys on disk with `0600`
permissions. The `KeyHandle` enum includes a `Hardware` variant
(currently returns an error) for future PKCS#11 / YubiKey / HSM
integration.

## Quick Start

```bash
# 1. Initialize the CA
rockbot cert ca generate --days 3650

# 2. Generate gateway cert
rockbot cert client generate --name gateway --role gateway \
  --san localhost --san 127.0.0.1 --days 365

# 3. Install into config
rockbot cert install --name gateway

# 4. Generate a TUI client cert
rockbot cert client generate --name my-tui --role tui --days 365

# 5. Start the gateway (now with mTLS)
rockbot gateway run

# 6. Connect with the TUI (using client cert)
#    (client cert config TBD — currently auto-accepts self-signed)
rockbot tui
```

## Shared PKI Configuration

TLS/PKI settings are defined in a shared `PkiConfig` struct, reusable by
gateway, client, and agent consumers:

```toml
# Gateway: PkiConfig fields live at the [gateway] level (serde flatten)
[gateway]
tls_cert = "/path/to/cert.pem"
tls_key  = "/path/to/key.pem"
tls_ca   = "/path/to/ca.crt"
require_client_cert = true
pki_dir  = "/path/to/pki"
enrollment_psk = "secret"
```

The `PkiConfig` struct is used as `gateway.pki` in the Rust config, but
flattened into the `[gateway]` TOML section for backward compatibility.
Client and agent consumers can reference the same struct for outbound mTLS
identity configuration.

## Certificate Extensions (Nebula-inspired)

Inspired by [Nebula](https://github.com/slackhq/nebula), RockBot embeds
authorization metadata directly in certificates using custom x.509 v3
extensions under a private OID arc:

```
1.3.6.1.4.1.59584.1.1  — Roles  (SEQUENCE OF UTF8String)
1.3.6.1.4.1.59584.1.2  — Groups (SEQUENCE OF UTF8String)
```

Extensions are DER-encoded and marked **non-critical** so that TLS libraries
that don't understand them still accept the certificate for transport auth.

### Usage

```rust
use rockbot_pki::{PkiManager, CertRole};

let mut mgr = PkiManager::new("/tmp/pki".into())?;
mgr.init_ca(3650)?;

// Issue a cert with roles and groups baked in
let info = mgr.generate_client(
    "deploy-agent", CertRole::Agent,
    &["10.0.0.5".to_string()], 365,
    &["admin".to_string(), "deploy".to_string()],      // roles
    &["engineering".to_string(), "us-west-2".to_string()], // groups
)?;

// Later, parse extensions from a presented cert
let der = std::fs::read(info.cert_path)?;
let exts = rockbot_pki::parse_extensions(&der)?;
assert_eq!(exts.roles, vec!["admin", "deploy"]);
assert_eq!(exts.groups, vec!["engineering", "us-west-2"]);
```

The certificate itself becomes the single source of truth for identity and
authorization — no external directory lookups at connection time.

## Vault Replication

For multi-node deployments, the PKI vault (index, CRL, credentials) can be
replicated over the existing Noise Protocol links. See
[vault-replication.md](vault-replication.md) for the full design.

## Future Work

- **Hardware key backends** — PKCS#11 (HSM), PIV (YubiKey), cloud KMS
- **Client-side cert loading** — TUI/agent load client cert from `PkiConfig` for outbound TLS
- **OCSP stapling** — online certificate status protocol as alternative to CRL
- **Certificate transparency** — append-only cert log for audit
- **Automatic rotation** — cron-based cert renewal before expiry
- **Vault replication** — PKI state sync over Noise protocol (see design doc)
