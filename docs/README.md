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
- [Deep Code Review Remediation Plan](architecture/deep-code-review-remediation-plan.md) — Validated current-state triage of the latest full-codebase review and phased remediation order
- [Encrypted Storage + PKI Refactor Proposal](architecture/encrypted-storage-pki-refactor-proposal.md) — High-level scope for encrypted-by-default redb, distributed vault authority, and role separation
- [Encrypted Storage + PKI Architecture Plan](architecture/encrypted-storage-pki-architecture-plan.md) — Formal architecture plan for roles, key hierarchy, grants, replication, and rollout
- [Virtual Disk Architecture Plan](architecture/virtual-disk-architecture-plan.md) — Formal plan for the `rockbot.data` virtual disk container, named volumes, encrypted local persistence, and model storage
- [Responsive WASM WebUI Migration Plan](architecture/webui-wasm-migration-plan.md) — Phased plan for replacing the bootstrap shell with a responsive authenticated browser app
- [Browser Plugin Architecture Plan](architecture/browser-plugin-architecture-plan.md) — Extension architecture for page-aware workflows, page-context capture, and safe browser actions
- [Tooling Gap Analysis](architecture/tooling-gap-analysis.md) — Practical comparison of current RockBot workflow gaps against adjacent coding/agent tools
- [Post-Maintenance Audit (2026-03-18)](architecture/post-maintenance-audit-2026-03-18.md) — Validation record and residual follow-on items after the latest maintenance/refactor pass
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
