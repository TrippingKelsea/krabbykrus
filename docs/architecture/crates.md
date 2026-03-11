# Crate Structure

RockBot is organized as a Cargo workspace with multiple crates, each responsible for a specific domain.

## Workspace Layout

```
rockbot/
├── Cargo.toml              # Workspace manifest
├── crates/
│   ├── rockbot/         # Binary crate (entry point)
│   ├── rockbot-cli/     # CLI and TUI
│   ├── rockbot-core/    # Gateway, agents, sessions
│   ├── rockbot-credentials/ # Secure credential vault
│   ├── rockbot-llm/     # LLM provider abstraction
│   ├── rockbot-memory/  # Memory and search
│   ├── rockbot-security/ # Capabilities and sandboxing
│   ├── rockbot-tools/   # Built-in tools
│   ├── rockbot-channels/ # Communication channels
│   └── rockbot-plugins/ # Plugin system
```

## Dependency Graph

```
rockbot (binary)
    │
    └─► rockbot-cli
            │
            ├─► rockbot-core
            │       │
            │       ├─► rockbot-credentials
            │       │       │
            │       │       └─► rockbot-security
            │       │
            │       ├─► rockbot-llm
            │       │
            │       ├─► rockbot-tools
            │       │       │
            │       │       └─► rockbot-security
            │       │
            │       ├─► rockbot-memory
            │       │
            │       ├─► rockbot-channels
            │       │
            │       └─► rockbot-plugins
            │
            └─► rockbot-credentials (direct CLI access)
```

## Crate Details

### `rockbot` (Binary)

**Purpose**: Main entry point that ties everything together.

**Dependencies**: `rockbot-cli`

**Exports**: None (binary only)

### `rockbot-cli`

**Purpose**: Command-line interface and terminal UI.

**Key modules**:
- `commands/` - CLI subcommands
- `tui/` - Terminal user interface
  - `app.rs` - Main event loop
  - `state.rs` - Centralized state
  - `components/` - UI components

**Public API**:
```rust
// Run the CLI
pub fn run() -> Result<()>;

// TUI entry point
pub mod tui {
    pub fn run_app() -> Result<()>;
}
```

### `rockbot-core`

**Purpose**: Core framework - gateway, agents, sessions.

**Key modules**:
- `gateway.rs` - HTTP server and API routing
- `agent.rs` - Agent execution engine
- `session.rs` - Session persistence
- `config.rs` - Configuration system
- `message.rs` - Message types
- `web_ui.rs` - Embedded web dashboard

**Public API**:
```rust
// Gateway
pub struct Gateway { ... }
impl Gateway {
    pub async fn new(config: Config) -> Result<Self>;
    pub async fn run(self) -> Result<()>;
}

// Agent
pub struct AgentEngine { ... }
impl AgentEngine {
    pub async fn process_message(&mut self, msg: Message) -> Result<Message>;
}

// Session
pub struct SessionManager { ... }
impl SessionManager {
    pub fn create_session(&self, agent_id: &str) -> Result<Session>;
    pub fn get_session(&self, key: &str) -> Result<Session>;
}

// Config
pub struct Config { ... }
impl Config {
    pub fn load() -> Result<Self>;
    pub fn from_file(path: &Path) -> Result<Self>;
}
```

### `rockbot-credentials`

**Purpose**: Secure credential storage with HIL support.

**Key modules**:
- `types.rs` - Core data types
- `crypto.rs` - Encryption utilities
- `storage.rs` - Vault operations
- `manager.rs` - High-level manager
- `permissions.rs` - Permission evaluation
- `audit.rs` - Audit logging

**Public API**:
```rust
// High-level manager (recommended)
pub struct CredentialManager { ... }
impl CredentialManager {
    pub async fn new(vault_path: &Path) -> Result<Self>;
    pub async fn unlock(&self, master_key: MasterKey) -> Result<()>;
    pub async fn check_permission(&self, path: &str) -> PathPermissionResult;
    pub async fn request_credential(&self, ...) -> CredentialRequestResult;
}

// Low-level vault
pub struct CredentialVault { ... }
impl CredentialVault {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn unlock(&mut self, key: MasterKey);
    pub fn create_endpoint(...) -> Result<Endpoint>;
    pub fn store_credential(...) -> Result<()>;
    pub fn decrypt_credential_for_endpoint(&self, id: Uuid) -> Result<Vec<u8>>;
}

// Crypto
pub struct MasterKey { ... }
impl MasterKey {
    pub fn derive_from_password(password: &str, salt: &[u8]) -> Result<Self>;
}
```

### `rockbot-llm`

**Purpose**: LLM provider abstraction.

**Key modules**:
- `provider.rs` - Provider trait
- `registry.rs` - Provider registry
- `types.rs` - Request/response types

**Public API**:
```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;
    fn models(&self) -> Vec<String>;
}

pub struct ProviderRegistry { ... }
impl ProviderRegistry {
    pub fn register(&mut self, name: &str, provider: Box<dyn LlmProvider>);
    pub fn get(&self, model: &str) -> Option<&dyn LlmProvider>;
}
```

### `rockbot-tools`

**Purpose**: Built-in tools for agent capabilities.

**Key modules**:
- `registry.rs` - Tool registry
- `execution.rs` - Tool execution
- `builtin/` - Built-in tool implementations

**Public API**:
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(&self, input: Value, ctx: &SecurityContext) -> Result<ToolResult>;
}

pub struct ToolRegistry { ... }
impl ToolRegistry {
    pub fn new(profile: ToolProfile) -> Self;
    pub fn register(&mut self, tool: Box<dyn Tool>);
    pub fn get(&self, name: &str) -> Option<&dyn Tool>;
}
```

### `rockbot-security`

**Purpose**: Capability system and sandboxing.

**Key modules**:
- `capabilities.rs` - Capability definitions
- `context.rs` - Security context

**Public API**:
```rust
pub enum Capability {
    FilesystemRead,
    FilesystemWrite,
    ProcessExecute,
    NetworkAccess,
    CredentialAccess,
    // ...
}

pub struct SecurityContext {
    pub capabilities: HashSet<Capability>,
    pub session_id: String,
}
```

### `rockbot-memory`

**Purpose**: Memory and search system.

**Key modules**:
- `manager.rs` - Memory manager
- `search.rs` - Search implementation

**Public API**:
```rust
pub struct MemoryManager { ... }
impl MemoryManager {
    pub fn load_documents(&mut self, path: &Path) -> Result<()>;
    pub fn search(&self, query: &str) -> Vec<SearchResult>;
}
```

### `rockbot-channels`

**Purpose**: Communication channel integrations.

**Key modules**:
- `traits.rs` - Channel trait
- `discord/`, `telegram/`, etc. - Channel implementations

**Public API**:
```rust
#[async_trait]
pub trait Channel: Send + Sync {
    async fn send(&self, message: &Message) -> Result<()>;
    async fn receive(&mut self) -> Result<Message>;
}
```

### `rockbot-plugins`

**Purpose**: Plugin system for extensibility.

**Key modules**:
- `traits.rs` - Plugin trait
- `loader.rs` - Plugin loading

**Public API**:
```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn on_load(&self) -> Result<()>;
    fn on_unload(&self) -> Result<()>;
}
```

## Type Conversions

Some types exist in multiple crates for layering purposes. Conversions are provided via `From` implementations:

```rust
// rockbot-core::config::ToolConfig -> rockbot-tools::ToolConfig
impl From<core::config::ToolConfig> for tools::ToolConfig { ... }
```

## Building

```bash
# Build all crates
cargo build

# Build specific crate
cargo build -p rockbot-credentials

# Build with release optimizations
cargo build --release

# Run tests for all crates
cargo test

# Run tests for specific crate
cargo test -p rockbot-credentials
```

## Documentation

Generate API documentation:

```bash
# Generate and open docs
cargo doc --open --no-deps

# Generate docs for all dependencies too
cargo doc --open

# Generate docs for specific crate
cargo doc -p rockbot-credentials --open
```
