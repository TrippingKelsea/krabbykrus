<p align="center">
  <img src="docs/assets/rockbot-logo.png" alt="RockBot" width="256" />
</p>

<h1 align="center">RockBot</h1>

<p align="center">
  A self-hosted AI gateway and agent framework written in Rust.
</p>

<p align="center">
  <a href="https://github.com/TrippingKelsea/rockbot/actions"><img src="https://github.com/TrippingKelsea/rockbot/workflows/CI/badge.svg" alt="Build Status" /></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT" /></a>
</p>

---

RockBot routes messages from Telegram and Signal through a central gateway to
AI agents backed by multiple LLM providers. Credentials are stored in an
encrypted vault and injected into tool execution at runtime — never exposed to
the agent.

## Highlights

- **Multi-provider LLM** — Anthropic, OpenAI, AWS Bedrock, Ollama
- **Multi-channel** — Telegram, Signal
- **Encrypted credential vault** — AES-256-GCM with Argon2id key derivation
- **Human-in-the-loop approval** — sensitive operations require consent
- **mTLS by default** — built-in PKI with CA, client certs, and enrollment
- **Terminal UI + Web dashboard** — manage everything from either interface
- **Multi-agent orchestration** — handoffs, swarm blackboards, graph workflows
- **Remote tool execution** — Noise-backed client registration and remote dispatch
- **Modular crate architecture** — compile only what you need

## Quick Start

```bash
# Build from source (Rust 1.75+)
git clone https://github.com/TrippingKelsea/rockbot.git
cd rockbot
make release

# Initialize config and TLS certificate
rockbot config init gateway

# Start the gateway
rockbot gateway run

# Connect with the TUI (from any machine)
rockbot tui -g 192.168.1.10:18182
```

## Documentation

| | |
|---|---|
| [Getting Started](docs/user-guide/getting-started.md) | Installation, first run, adding credentials |
| [Configuration](docs/user-guide/configuration.md) | All config options and feature flags |
| [TUI Guide](docs/user-guide/tui-guide.md) | Navigating the terminal interface |
| [Architecture](docs/architecture/overview.md) | System design and data flow |
| [Crate Structure](docs/architecture/crates.md) | Workspace layout and dependency graph |
| [PKI and mTLS](docs/architecture/pki.md) | Certificate authority, mutual TLS, enrollment |
| [Security Model](docs/architecture/security.md) | Credential flow, capabilities, trust boundaries |
| [API Reference](docs/api.md) | HTTP/WebSocket endpoints |
| [Feature Matrix](docs/FEATURES.md) | Full implementation status |

## Building

```bash
make dev                                     # debug build, enhanced profile
make release                                 # release build, enhanced profile
make test                                    # test suite, enhanced profile

# Raw cargo equivalents
cargo build --release --no-default-features --features enhanced
cargo build --release --no-default-features --features experimental
cargo build --release --no-default-features -F anthropic
```

See [docs/architecture/crates.md](docs/architecture/crates.md) for the full
feature flag reference.

## Contributing

Contributions welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) before
submitting PRs.

## License

MIT — see [LICENSE](LICENSE) for details.
