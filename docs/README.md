# RockBot Documentation

Welcome to the RockBot documentation. This documentation is organized into three main areas:

## 📖 User Guide

For end users and operators deploying RockBot.

- [Getting Started](user-guide/getting-started.md) - Installation and first run
- [Configuration](user-guide/configuration.md) - Configuration reference
- [CLI Reference](user-guide/cli-reference.md) - Command-line interface
- [TUI Guide](user-guide/tui-guide.md) - Terminal user interface
- [Web UI Guide](user-guide/web-ui-guide.md) - Browser-based dashboard
- [Credential Management](user-guide/credentials.md) - Secure credential vault

## 🏗️ Architecture

For developers and contributors understanding the system design.

- [Overview](architecture/overview.md) - High-level architecture
- [Crate Structure](architecture/crates.md) - Workspace organization
- [Security Model](architecture/security.md) - Capability and permission system
- [Credential Flow](architecture/credential-flow.md) - How credentials are handled

## 📚 API Reference

Auto-generated from source code via `cargo doc`.

```bash
# Generate and open API documentation
cargo doc --open --no-deps
```

Online: [API Documentation](api/index.html) (after running `cargo doc`)

## Feature Matrix

See [FEATURES.md](FEATURES.md) for a detailed breakdown of implemented vs planned features.

## Quick Links

- [GitHub Repository](https://github.com/TrippingKelsea/rockbot)
- [Issue Tracker](https://github.com/TrippingKelsea/rockbot/issues)
- [Contributing Guide](../CONTRIBUTING.md)
