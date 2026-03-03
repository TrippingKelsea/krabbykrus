# 🦀 Krabbykrus

A Rust-native AI agent framework with secure credential management.

## Features

- **Secure Credential Vault** - Encrypted storage with multiple unlock methods (password, keyfile, Age, SSH)
- **Terminal UI** - Responsive async TUI built with ratatui
- **Web UI** - Embedded dashboard for browser-based management
- **Multi-Provider LLM Support** - Anthropic, OpenAI, Ollama, AWS Bedrock
- **Plugin System** - Extensible tool architecture

## Crates

| Crate | Description |
|-------|-------------|
| `krabbykrus` | Main binary |
| `krabbykrus-cli` | CLI commands and TUI |
| `krabbykrus-core` | Gateway and web UI |
| `krabbykrus-credentials` | Secure credential vault |
| `krabbykrus-llm` | LLM provider abstraction |
| `krabbykrus-memory` | Memory and search system |
| `krabbykrus-security` | Sandboxing and permissions |
| `krabbykrus-tools` | Built-in agent tools |
| `krabbykrus-channels` | Communication channels |
| `krabbykrus-plugins` | Plugin system |

## Building

```bash
cargo build --release
```

## License

MIT
