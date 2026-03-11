# Contributing to RockBot

Thank you for your interest in contributing to RockBot! This document provides guidelines for contributing.

## Code of Conduct

Be respectful and inclusive. We're all here to build cool things together.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/rockbot.git`
3. Create a branch: `git checkout -b feature/your-feature`
4. Make your changes
5. Run tests: `cargo test`
6. Commit with a descriptive message
7. Push and open a PR

## Development Setup

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs/))
- SQLite 3
- OpenSSL development headers

### Building

```bash
cargo build          # Debug build
cargo build --release # Release build
cargo test           # Run all tests
cargo clippy         # Lint check
cargo fmt            # Format code
```

### Running

```bash
# Start gateway
cargo run -- gateway

# Launch TUI
cargo run -- tui

# Run with debug logging
RUST_LOG=debug cargo run -- gateway
```

## Project Structure

```
crates/
├── rockbot/          # Binary entry point
├── rockbot-cli/      # CLI and TUI
├── rockbot-core/     # Gateway, agents, sessions
├── rockbot-credentials/ # Credential vault
├── rockbot-llm/      # LLM providers
├── rockbot-memory/   # Memory system
├── rockbot-security/ # Capabilities
├── rockbot-tools/    # Built-in tools
├── rockbot-channels/ # Communication
└── rockbot-plugins/  # Plugin system
```

## Coding Guidelines

### Rust Style

- Follow standard Rust conventions
- Use `cargo fmt` before committing
- Address `cargo clippy` warnings
- Document public APIs with `///` doc comments

### Documentation

Every public item should have documentation:

```rust
/// Short description of what this does.
///
/// Longer description with more details if needed.
///
/// # Arguments
///
/// * `arg1` - Description of argument
///
/// # Returns
///
/// Description of return value.
///
/// # Errors
///
/// When and why this returns an error.
///
/// # Examples
///
/// ```
/// let result = my_function(42);
/// assert!(result.is_ok());
/// ```
pub fn my_function(arg1: i32) -> Result<String> {
    // ...
}
```

### Error Handling

- Use `thiserror` for error types
- Prefer `Result<T, E>` over panics
- Provide context in error messages
- Chain errors with `?` operator

```rust
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::IoError { path: path.to_owned(), source: e })?;
    
    toml::from_str(&content)
        .map_err(|e| ConfigError::ParseError { path: path.to_owned(), source: e })
}
```

### Testing

- Write unit tests for all public functions
- Use `#[cfg(test)]` modules in the same file
- Integration tests go in `tests/` directory
- Mock external dependencies

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_function() {
        let result = my_function(42);
        assert_eq!(result.unwrap(), "expected");
    }

    #[tokio::test]
    async fn test_async_function() {
        let result = async_function().await;
        assert!(result.is_ok());
    }
}
```

## Pull Request Process

1. **Title**: Use conventional commit format
   - `feat: Add new feature`
   - `fix: Fix bug in X`
   - `docs: Update documentation`
   - `refactor: Refactor Y`
   - `test: Add tests for Z`

2. **Description**: Explain what and why
   - What does this change do?
   - Why is it needed?
   - How was it tested?

3. **Checklist**:
   - [ ] Tests pass (`cargo test`)
   - [ ] No clippy warnings (`cargo clippy`)
   - [ ] Code formatted (`cargo fmt`)
   - [ ] Documentation updated
   - [ ] CHANGELOG updated (if applicable)

4. **Review**: Address feedback promptly

## Security

If you discover a security vulnerability, please email security@example.com instead of opening a public issue.

## Feature Requests

Open an issue with the `enhancement` label. Include:
- Use case description
- Proposed solution
- Alternatives considered

## Bug Reports

Open an issue with the `bug` label. Include:
- RockBot version
- Operating system
- Steps to reproduce
- Expected vs actual behavior
- Relevant logs

## Questions?

- Open a discussion on GitHub
- Check existing issues and docs first

Thank you for contributing! 🦀
