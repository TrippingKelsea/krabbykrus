# RockBot — Claude Code Instructions

## Commit Standards

- Use [Conventional Commits](https://www.conventionalcommits.org/) format
- Prefixes: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `perf:`
- Keep the first line under 72 characters
- Use the body for details — explain *why*, not just *what*
- Never skip pre-commit hooks (`--no-verify`)
- Never amend published commits without explicit permission
- Never force-push to `main`

## Documentation Standards

- All new features must update relevant docs before committing
- Every public API endpoint must be documented in `docs/api.md`
- New crates must be added to `docs/architecture/crates.md` (layout, dependency graph, key modules)
- New config fields must be documented in `docs/user-guide/configuration.md`
- CLI command changes must be reflected in `docs/FEATURES.md`
- Keep `docs/architecture/overview.md` current with architectural changes
- Update `CHANGELOG.md` for every user-facing change (features, fixes, breaking changes)
- Verify no broken links: all paths referenced in README.md and docs/README.md must exist

### Documentation file purposes

| File | Content |
|------|---------|
| `README.md` | Project overview, quick start, doc links |
| `CHANGELOG.md` | Running changelog by version |
| `CONTRIBUTING.md` | Contribution guidelines |
| `docs/README.md` | Documentation index |
| `docs/api.md` | HTTP/WS endpoint reference |
| `docs/FEATURES.md` | Feature implementation matrix |
| `docs/architecture/overview.md` | High-level architecture |
| `docs/architecture/crates.md` | Crate structure and features |
| `docs/architecture/pki.md` | PKI and mTLS design |
| `docs/architecture/security.md` | Security model |
| `docs/user-guide/getting-started.md` | Installation and first run |
| `docs/user-guide/configuration.md` | Full config reference |
| `docs/user-guide/tui-guide.md` | TUI navigation |

## Code Standards

- Run `cargo clippy` and `cargo fmt` before commits
- All test modules must have `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`
- Use `anyhow` for application errors, `thiserror` for library error types
- Feature flags propagate: `rockbot` -> `rockbot-cli` -> `rockbot-core` -> per-crate
- No dependency cycles between crates
- New crates must opt into workspace lints: `[lints] workspace = true`

## Project Layout

- Binary: `crates/rockbot/`
- 20 crates total — see `docs/architecture/crates.md`
- Config: `crates/rockbot-config/src/config.rs`
- Gateway: `crates/rockbot-gateway/src/gateway.rs`
- Agent: `crates/rockbot-agent/src/agent.rs`
- PKI: `crates/rockbot-pki/src/`
- CLI commands: `crates/rockbot-cli/src/commands/`
- TUI: `crates/rockbot-cli/src/tui/`
