# RockBot Documentation

## User Guide

- [Getting Started](user-guide/getting-started.md) — Installation, first run, adding credentials
- [Configuration Reference](user-guide/configuration.md) — All config options and feature flags
- [TUI Guide](user-guide/tui-guide.md) — Terminal user interface

## Architecture

- [Overview](architecture/overview.md) — High-level architecture and data flow
- [Crate Structure](architecture/crates.md) — Workspace layout, dependency graph, feature flags
- [Execution Locality Proposal](architecture/execution-locality-proposal.md) — Hardening plan for remote execution grounding and provenance
- [Code Review Round 2 Remediation Plan](architecture/code-review-round2-remediation-plan.md) — Prioritized execution plan for the current security, correctness, and quality findings
- [Encrypted Storage + PKI Refactor Proposal](architecture/encrypted-storage-pki-refactor-proposal.md) — High-level scope for encrypted-by-default redb, distributed vault authority, and role separation
- [Encrypted Storage + PKI Architecture Plan](architecture/encrypted-storage-pki-architecture-plan.md) — Formal architecture plan for roles, key hierarchy, grants, replication, and rollout
- [PKI and mTLS](architecture/pki.md) — Certificate authority, mutual TLS, enrollment, x.509 extensions
- [Vault Replication](architecture/vault-replication.md) — PKI/credential sync over Noise protocol (draft)
- [Security Model](architecture/security.md) — Credential flow, capabilities, trust boundaries

## Reference

- [API Reference](api.md) — HTTP endpoints and WebSocket protocol
- [Feature Matrix](FEATURES.md) — Implementation status
- [Feature Evaluation Tracker](feature-evaluation.md) — Agent-suggested features and critical issues for review
- [Changelog](../CHANGELOG.md) — Version history

## Links

- [GitHub Repository](https://github.com/TrippingKelsea/rockbot)
- [Contributing Guide](../CONTRIBUTING.md)
