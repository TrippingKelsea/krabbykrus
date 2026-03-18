# Encrypted Storage and PKI Refactor Proposal

## Purpose

This document captures the intended scope of the encrypted-storage and PKI
refactor at a high level. The goal is to move RockBot toward security-first
defaults without hard-coding a single deployment shape.

The refactor unifies four concerns that are currently split across multiple
subsystems:

- local at-rest storage encryption
- vault secret distribution
- PKI identity and authorization
- multi-node replication and authority

## Primary Goals

1. Encrypt redb-backed local storage by default.
2. Keep PKI identity, storage encryption, and vault encryption as separate key
   domains.
3. Support node-targeted vault sharing instead of assuming one shared cluster
   secret.
4. Separate `gateway` and `vault-provider` roles so they can be deployed
   independently.
5. Preserve row-level replication over the existing raft direction.
6. Make secure deployment operationally easy through first-class tooling.
7. Leave room for threshold/quorum-based authority as a later expansion.

## Current Problems

- `rockbot-storage` has an optional encrypted backend, but most production paths
  still open plaintext redb files.
- The current encrypted backend is confidentiality-only and does not provide
  authenticated tamper detection.
- The vault has its own unlock model, separate from the general redb storage
  model.
- PKI private keys are still stored as plain PEM files via `FileBackend`.
- Replication and authority models are not yet aligned with node-local storage
  encryption.

## Role Model

The refactor should make roles explicit and certificate-backed.

- `gateway`
  Handles client traffic, routing, agents, APIs, and execution coordination.
- `vault-provider`
  Holds authority to decrypt canonical vault objects and issue node-specific
  grants.
- `client`
  Consumes secrets and tools but does not issue grants.
- `admin`
  Optional operational role for bootstrap, enrollment, policy, and rotation.

A node may have multiple roles, but that must be an explicit deployment choice.
`gateway` and `vault-provider` should not be treated as the same thing.

## Key Domains

The refactor should maintain clear separation between:

- `identity keypair`
  Used for mTLS, Noise, enrollment, and node identity.
- `local storage key`
  Node-local key used to encrypt the node's redb files at rest.
- `vault encryption keypair`
  Node-specific keypair used to receive encrypted vault grants.
- `canonical object material`
  Logical secret material for a vault object.

Identity keys should authenticate and authorize. They should not be reused as
bulk data-encryption keys.

## At-Rest Storage Model

Each node should encrypt its own redb data locally by default.

This produces two distinct protection layers:

- outer layer: node-local redb encryption
- inner layer: vault-object and grant encryption

Local redb encryption is node-specific and must not be replicated by copying raw
store bytes between nodes.

## Vault Sharing Model

Vault data should be modeled as logical objects plus explicit grants.

Recommended logical tables:

- `node_keys`
  - node identity fingerprint
  - node vault public key
  - roles
  - binding certificate or signature
  - status and rotation metadata
- `vault_objects`
  - logical object metadata
  - namespace / tenant / owner
  - version and policy reference
- `vault_provider_grants`
  - object encrypted to each authorized vault-provider
- `vault_node_grants`
  - object encrypted to each authorized consumer node
- `vault_policies`
  - authorization rules
- `vault_audit`
  - issuance, revocation, rotation, and policy changes

Under this model, when a secret is shared with a node, the authoritative
vault-provider creates a node-specific encrypted grant addressed to that node's
vault public key. The grant replicates as a logical row, and the recipient node
can decrypt it locally.

## Distributed Authority

The system should support multiple authoritative `vault-provider` nodes.

That means:

- authority is a certified capability, not a singleton host
- any authorized provider can issue or revoke grants
- grant issuance and revocation must be committed through raft-managed cluster
  state
- providers should act only on committed state, not private local assumptions

This enables:

- gateway-only nodes
- vault-provider-only nodes
- combined small deployments
- lower-blast-radius dedicated vault tiers

## Replication Model

Replication should happen at the logical row level.

Replicate:

- node key registry rows
- role and authority metadata
- vault object metadata
- provider grants
- node grants
- audit and revocation state

Do not replicate raw redb files or node-local redb ciphertext. Each node stores
replicated rows into its own locally encrypted redb instance.

## Threshold / Quorum Future

Threshold or quorum-based authority should remain a later option.

Phase 1 should support multiple vault-provider authorities without requiring
threshold cryptography. The design should, however, avoid assumptions that make
quorum support impossible later.

Future options include:

- threshold-held authority keys
- multi-party approval for high-risk grants
- split-key recovery and rotation flows

## Operational Requirements

The deployment story matters as much as the crypto model. Secure defaults must
be easy to use.

The refactor should be accompanied by first-class tooling for:

- cluster bootstrap
- node enrollment
- vault-provider authorization
- node grant issuance and revocation
- migration from plaintext storage
- security health checks and drift detection
- key and grant rotation

## Recommended Delivery Order

1. Replace the current redb encryption backend with an authenticated design.
2. Add encrypted-by-default local storage and migration tooling.
3. Introduce node vault keypairs and a replicated `node_keys` registry.
4. Add explicit `gateway` and `vault-provider` roles.
5. Implement logical vault objects and per-node/provider grants.
6. Support multiple authoritative vault-providers.
7. Add threshold/quorum support later as an extension.

## Outcome

This refactor should move RockBot from:

- optional local encryption
- singleton-oriented vault authority
- mixed key models
- file-centric PKI state

to:

- encrypted local state by default
- explicit role separation between gateway and vault-provider
- per-node encrypted vault grants
- raft-replicated logical authority data
- PKI-backed identity and authorization
- secure deployment workflows that are easy to operate
