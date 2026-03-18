# Virtual Disk Architecture Plan

## Purpose

This document formalizes the architecture for a Rust-native virtual disk layer
for RockBot.

The goal is to replace scattered on-disk application data with a single
container file:

- `rockbot.data`

This container becomes the persistent data substrate for RockBot application
state. It is intended to hold all durable app data except bootstrap
configuration and operational logs.

## Scope

`rockbot.data` should contain:

- session storage
- cron storage
- routing state
- agent persistence
- vault metadata and credential storage
- PKI index metadata
- replicated store metadata
- overseer/runtime state that is meant to persist
- downloaded model artifacts
- embeddings, indexes, caches, and model-side metadata that should survive
  restarts
- future snapshots and backup metadata

`rockbot.data` should not contain:

- `rockbot.toml`
- run logs
- debug logs
- ephemeral temp files
- OS-level TLS trust stores

## Design Goals

1. One file for durable app storage.
2. Rust-native implementation with no dependency on OS disk tools.
3. Authenticated encryption by default.
4. Multiple logical volumes inside one physical container.
5. Compatible with `redb` through a storage-backend adapter.
6. Able to store large binary model artifacts as well as structured DB data.
7. Safe crash recovery and integrity verification.
8. Clean migration path from current multi-file storage.
9. Local persistence only. Replication remains logical, not file-level.

## Non-Goals

This is not intended to be:

- a mounted filesystem
- a FUSE volume
- a raw block device exposed to the OS
- a replacement for Raft replication

If RockBot ever wants a mountable disk, that is a separate feature with
different platform constraints.

## Current State

Today RockBot persists data across multiple paths and backends:

- multiple `.redb` files for sessions, cron, routing, and agents
- vault state in a separate credential store
- PKI material and metadata in separate files/directories
- model downloads outside the core store boundary

The current encrypted storage layer is not sufficient as the long-term base for
this design:

- it is file-by-file rather than volume-based
- the current `EncryptedBackend` is confidentiality-only, not authenticated
- it does not unify downloaded model artifacts with application state

## Architectural Direction

Introduce a new storage layer, likely as:

- `crates/rockbot-vdisk`

This crate owns the `rockbot.data` container format and exposes named logical
volumes that RockBot subsystems can open.

The stack becomes:

```text
Domain crates
  -> rockbot-storage / model cache / vault / PKI metadata
    -> rockbot-vdisk logical volume API
      -> rockbot.data container file
```

## Core Model

### Physical Container

A single file:

- `rockbot.data`

This file contains:

- disk superblock
- format/version metadata
- allocation metadata
- volume table
- per-volume metadata
- encrypted data pages/extents
- journal / recovery records
- snapshot metadata

### Logical Volumes

The container is partitioned into named logical volumes. Recommended initial
volume set:

- `sessions`
- `cron`
- `routing`
- `agents`
- `vault`
- `pki`
- `replication_meta`
- `overseer`
- `models`
- `indexes`
- `cache`

Each volume is local to the node and independently addressable by RockBot.

### Storage Types

The virtual disk should support two usage patterns:

1. `redb`-backed structured volumes
2. blob/object storage volumes for large binary data

That distinction matters because downloaded models should not be forced through
`redb` row storage.

## Volume Classes

### Structured Volumes

Used for:

- sessions
- cron
- routing
- agents
- vault
- PKI metadata
- replicated state metadata

These volumes should expose a `redb::StorageBackend` adapter so `redb` can
continue to provide tables and transactions.

### Blob Volumes

Used for:

- downloaded LLM models
- tokenizer data
- embeddings/index segments
- large cached artifacts

These should expose a simple object/blob API:

- create object
- read object
- replace object
- delete object
- stream object
- list objects

For large models, support sparse or extent-based allocation and chunked reads.

## Recommended File Layout Inside `rockbot.data`

At a high level:

```text
rockbot.data
├── superblock
├── journal
├── allocator metadata
├── volume directory
├── structured volumes
│   ├── sessions
│   ├── cron
│   ├── routing
│   ├── agents
│   ├── vault
│   ├── pki
│   └── replication_meta
└── blob volumes
    ├── models
    ├── indexes
    └── cache
```

## Crypto Model

### Requirement

The virtual disk must use authenticated encryption.

The current store backend based on raw ChaCha20 should not be reused as the
final disk format.

### Recommended Scheme

Per-page or per-extent AEAD, using one of:

- `XChaCha20-Poly1305`
- `AES-256-GCM-SIV`

Recommended default:

- `XChaCha20-Poly1305`

### Associated Data

Each encrypted page/extent should authenticate:

- volume id
- logical page number / extent id
- generation/version
- container format version

This prevents page swapping and silent tampering.

### Key Hierarchy

Use the existing PKI/storage refactor direction:

- node-local storage root key
- per-volume keys derived via HKDF

Suggested derivation:

- `HKDF(root_key, "rockbot-vdisk:<volume-name>")`

This preserves node-local at-rest protection and cleanly composes with the
existing encrypted-storage plan.

## Page and Extent Model

### Structured Volumes

Use fixed-size encrypted pages.

Recommended initial page size:

- `16 KiB`

Why:

- good fit for `redb`-style random IO
- lower metadata overhead than very small pages
- simpler recovery semantics

### Blob Volumes

Use extent-based allocation:

- contiguous where possible
- sparse where useful
- chunked reads for large objects

Recommended object chunk size:

- `1 MiB` logical chunks over larger extents

This is more suitable for model files than forcing them into small pages.

## Concurrency and Crash Recovery

### Required Properties

- process-safe file locking
- atomic metadata updates
- crash recovery after partial writes
- integrity verification on startup

### Journal

The container should include a write-ahead journal for:

- allocator updates
- volume metadata changes
- object metadata changes
- snapshot creation/deletion

The journal should replay or roll back incomplete mutations on open.

### Verification

Provide an integrity verifier that can:

- verify superblock
- verify volume directory
- verify page/extent authentication tags
- detect leaked/unreachable extents
- verify snapshot metadata

## API Shape

### Core API

Suggested top-level types:

- `VirtualDisk`
- `VolumeHandle`
- `StructuredVolume`
- `BlobVolume`
- `SnapshotHandle`

### Operations

`VirtualDisk`:

- open
- create
- verify
- list_volumes
- create_volume
- delete_volume
- create_snapshot
- restore_snapshot

`StructuredVolume`:

- open_redb_backend
- stats
- snapshot
- compact

`BlobVolume`:

- put_object
- get_object
- delete_object
- list_objects
- stream_object
- object_metadata

## Integration with Existing RockBot Crates

### `rockbot-storage`

`rockbot-storage` should stop owning the disk file directly.

Instead:

- `rockbot-vdisk` owns `rockbot.data`
- `rockbot-storage` opens a named structured volume
- `rockbot-storage` wraps `redb` over that volume

This likely replaces:

- `Store::open(path)`
- `Store::open_encrypted(path, key)`

with an API closer to:

- `Store::open_volume(disk, "sessions")`

### `rockbot-credentials`

The vault should move into the `vault` structured volume inside `rockbot.data`.

That means:

- vault metadata
- credential rows
- permissions
- distributed vault grants

all live inside the container instead of a separate free-standing store.

### `rockbot-pki`

Not all PKI material should automatically move inside the disk.

Recommended boundary:

- PKI index metadata: inside `rockbot.data`
- active identity key material needed before disk unlock: outside
- optional protected copies / exports: inside

This preserves bootstrap trust while still consolidating operational PKI state.

### Model Storage

Downloaded models should move into the `models` blob volume.

That includes:

- model binaries
- tokenizer files
- downloaded metadata
- integrity/fingerprint metadata

Benefits:

- one backup boundary
- no separate model cache drift
- unified quota/accounting
- easier snapshotting and migration

## Config and Path Model

The bootstrap config should point to the disk file, but not micromanage all
subpaths.

Suggested bootstrap config:

```toml
[storage]
data_file = "~/.config/rockbot/rockbot.data"
```

Everything else should be discovered from the volume directory inside the disk.

## What Stays Outside `rockbot.data`

Keep these outside:

- `rockbot.toml`
- active run logs
- debug logs
- bootstrap TLS/client identity material needed before disk unlock
- ephemeral temp files

That keeps bootstrap and diagnostics simple.

## Replication Model

`rockbot.data` is local persistence only.

RockBot should not replicate raw container bytes between nodes.

Replication should continue to operate on:

- logical rows
- logical vault objects/grants
- committed application mutations

Each node stores replicated logical state into its own local `rockbot.data`
container, encrypted with its own local storage keys.

## Migration Plan

### Phase 1

Introduce `rockbot-vdisk` with:

- container file creation
- superblock
- allocator
- named volumes
- authenticated encryption
- basic verification

### Phase 2

Move structured stores first:

- sessions
- cron
- routing
- agents

This is the lowest-risk initial migration.

### Phase 3

Move vault and PKI metadata:

- vault rows
- permissions
- grants
- PKI index

### Phase 4

Move model and cache artifacts:

- model downloads
- tokenizers
- indexes
- embeddings/caches

### Phase 5

Add:

- snapshots
- compaction
- export/import
- fsck/repair tooling

## Operational Commands

Recommended CLI surface:

- `rockbot storage init`
- `rockbot storage verify`
- `rockbot storage status`
- `rockbot storage migrate`
- `rockbot storage snapshot create`
- `rockbot storage snapshot list`
- `rockbot storage snapshot restore`
- `rockbot storage export-volume`
- `rockbot storage import-volume`

## Risks

### 1. `redb` Backend Correctness

The most important technical risk is correctness of the `redb::StorageBackend`
adapter over virtual volumes.

This needs strong crash/recovery testing.

### 2. Blob/Model Performance

Large model artifacts will stress allocation and read paths differently from
structured DB workloads.

Blob volumes should not be treated as an afterthought.

### 3. Migration Complexity

Moving from multiple stores to one container touches:

- sessions
- vault
- PKI metadata
- model cache

Migration tooling must be explicit and reversible.

### 4. Bootstrap Boundary

Do not accidentally move bootstrap-required trust material entirely inside the
disk if the disk itself needs that trust boundary to open.

## Recommended Initial Build Scope

The first implementation target should be:

1. `rockbot-vdisk`
2. `rockbot.data`
3. authenticated encrypted named volumes
4. `redb` adapter for structured volumes
5. migration of sessions/cron/agents/routing

Then:

6. vault and PKI metadata
7. model downloads and caches
8. snapshots and tooling

## Summary

RockBot should adopt a single Rust-native virtual disk container:

- `rockbot.data`

This container should hold all durable application data except bootstrap config
and run/debug logs. It should support both structured `redb`-backed volumes and
blob volumes for model artifacts. It should be encrypted with authenticated
per-volume storage keys, integrate cleanly with the existing PKI/storage key
hierarchy, and remain strictly node-local while Raft continues to replicate
logical state above it.
