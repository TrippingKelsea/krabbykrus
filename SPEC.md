# rockbot System Specification

**Version:** 1.0
**Generated:** 2026-03-08
**Purpose:** Complete specification enabling reconstruction of rockbot functionality in any programming language

---

## 1. Executive Summary

rockbot is a **self-hosted, multi-channel AI gateway** that connects messaging applications to AI agents for real-time conversation and task execution. It acts as a unified control plane routing messages from 30+ messaging platforms (WhatsApp, Telegram, Discord, Slack, Signal, iMessage, etc.) through AI agents that can execute tools, maintain conversational context, and deliver responses back through the originating channels.

### Core Design Principles

1. **Self-hosted first**: All data stays on user's devices; no cloud dependency
2. **Single-user focused**: Personal assistant with multi-agent isolation
3. **Multi-channel unified inbox**: Route messages from various platforms through one control plane
4. **Agent-native**: Built for AI coding agents with tool use, sessions, memory, and workspace management
5. **Plugin-extensible**: Core functionality extensible via plugins for channels, tools, providers

---

## 2. System Architecture

### 2.1 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Messaging Platforms                               │
│  WhatsApp │ Telegram │ Discord │ Slack │ Signal │ iMessage │ ...   │
└────────────────────────────────┬────────────────────────────────────┘
                                 │ Platform SDKs/APIs
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         Gateway Server                               │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐                 │
│  │   Channel    │ │   Routing    │ │   Session    │                 │
│  │   Manager    │ │   Engine     │ │   Store      │                 │
│  └──────────────┘ └──────────────┘ └──────────────┘                 │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐                 │
│  │   Agent      │ │   Plugin     │ │    Cron      │                 │
│  │   Runtime    │ │   Registry   │ │   Scheduler  │                 │
│  └──────────────┘ └──────────────┘ └──────────────┘                 │
│  ┌──────────────┐                                                    │
│  │  Provider    │                                                    │
│  │  Registry    │                                                    │
│  └──────────────┘                                                    │
│                         WebSocket RPC / HTTP API                     │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
         ┌───────────────────────┼───────────────────────┐
         │                       │                       │
         ▼                       ▼                       ▼
    ┌─────────┐            ┌─────────┐            ┌─────────┐
    │   CLI   │            │ Web UI  │            │ Mobile  │
    │ Client  │            │ Client  │            │  Apps   │
    └─────────┘            └─────────┘            └─────────┘
```

### 2.2 Component Responsibilities

| Component | Responsibility |
|-----------|----------------|
| **Gateway Server** | Central orchestration; WebSocket RPC server; HTTP API server; single source of truth for all state |
| **Channel Manager** | Maintains connections to messaging platforms; handles inbound/outbound |
| **Routing Engine** | Maps incoming messages to agents based on bindings and policies |
| **Session Store** | Persists conversation transcripts and session metadata |
| **Agent Runtime** | Executes AI agents with tool calling and streaming responses |
| **Plugin Registry** | Loads and manages extension plugins |
| **Cron Scheduler** | Executes scheduled jobs and heartbeats |
| **Provider Registry** | Manages LLM provider lifecycle; auto-detection, registration, status, and model routing |
| **CLI Client** | Command-line interface for gateway interaction |

### 2.3 Interface Principle

TUI, WebUI, and CLI are **purely presentation layers**. They do NOT directly instantiate LLM providers, make LLM API calls, or maintain provider state. All LLM operations flow through the gateway's HTTP/WebSocket API.

This makes the gateway the single manager of the bot — everything else is just an interface:

- Interfaces query the gateway for provider lists and status via `/api/providers`
- Interfaces send chat messages through the gateway via `/api/chat` or `chat.send` RPC
- Interfaces never hardcode provider lists or call LLM APIs directly
- Provider credentials, model availability, and auth state are determined by the gateway at runtime

---

## 3. Gateway Protocol

### 3.1 Transport

- **Protocol**: WebSocket (ws:// or wss://)
- **Default Port**: 18789
- **Frame Format**: JSON
- **Protocol Version**: 3

### 3.2 Frame Types

#### Request Frame (Client → Server)
```typescript
{
  type: "req",
  id: string,        // Unique request ID for correlation
  method: string,    // RPC method name
  params: object     // Method-specific parameters
}
```

#### Response Frame (Server → Client)
```typescript
{
  type: "res",
  id: string,           // Matches request ID
  ok: boolean,          // Success indicator
  payload?: object,     // Result on success
  error?: {             // Error details on failure
    code: string,
    message: string,
    details?: object,
    retryable?: boolean,
    retryAfterMs?: number
  }
}
```

#### Event Frame (Server → Client)
```typescript
{
  type: "event",
  event: string,        // Event name
  payload: object,      // Event-specific data
  seq: number,          // Sequence number for ordering
  stateVersion: {       // State version for sync
    presence: number,
    health: number
  }
}
```

### 3.3 Connection Handshake

1. Client connects to WebSocket endpoint
2. Client sends `connect` request with client info:
   ```typescript
   {
     clientId: string,
     clientVersion: string,
     platform: "cli" | "web" | "ios" | "android" | "macos" | "node",
     capabilities: string[],
     authToken?: string,
     authPassword?: string
   }
   ```
3. Server validates authentication and responds with `HelloOk`:
   ```typescript
   {
     protocolVersion: number,
     features: string[],
     snapshot: Snapshot,
     sessionToken?: string,    // For subsequent auth
     deviceToken?: string,     // For device pairing
     policy: {
       execApprovalRequired: boolean,
       pushNotifications: boolean
     }
   }
   ```

### 3.4 RPC Methods

#### Agent Methods
| Method | Description |
|--------|-------------|
| `agent.invoke` | Execute agent turn with message |
| `agent.wait` | Wait for running agent to complete |
| `agent.wake` | Wake agent with text input |
| `agent.identity` | Get/set agent identity |

#### Chat Methods
| Method | Description |
|--------|-------------|
| `chat.send` | Send message to agent |
| `chat.abort` | Abort running chat |
| `chat.inject` | Inject message into transcript |
| `chat.history` | Retrieve chat history |

#### Session Methods
| Method | Description |
|--------|-------------|
| `sessions.list` | List active sessions |
| `sessions.resolve` | Resolve session by key/ID |
| `sessions.patch` | Update session metadata |
| `sessions.reset` | Reset session (clear history) |
| `sessions.delete` | Delete session |
| `sessions.compact` | Compact transcript |
| `sessions.usage` | Get usage statistics |

#### Channel Methods
| Method | Description |
|--------|-------------|
| `channels.status` | Get channel health status |
| `channels.send` | Send message through channel |
| `channels.poll` | Create poll in channel |

#### Config Methods
| Method | Description |
|--------|-------------|
| `config.get` | Get current configuration |
| `config.set` | Set configuration (raw YAML/JSON) |
| `config.apply` | Apply configuration changes |
| `config.schema` | Get configuration schema |

#### Provider Methods
| Method | Description |
|--------|-------------|
| `providers.list` | List registered LLM providers and their status |
| `providers.status` | Get specific provider status including auth and models |
| `providers.test` | Test provider connectivity |

#### Cron Methods
| Method | Description |
|--------|-------------|
| `cron.list` | List scheduled jobs |
| `cron.add` | Add new cron job |
| `cron.edit` | Edit existing job |
| `cron.remove` | Remove job |
| `cron.run` | Trigger immediate execution |

### 3.5 Event Types

| Event | Description |
|-------|-------------|
| `presence` | Client connect/disconnect notifications |
| `health` | Channel health updates |
| `chat` | Chat message/stream events |
| `agent` | Agent execution events |
| `system` | System notifications |
| `config` | Configuration change notifications |

---

## 4. Channel System

### 4.1 Channel Plugin Interface

Each messaging channel implements the `ChannelPlugin` interface:

```typescript
interface ChannelPlugin<ResolvedAccount = any> {
  // Identity
  id: ChannelId;
  meta: {
    id: string;
    label: string;
    docsPath: string;
  };

  // Capabilities
  capabilities: {
    chatTypes: Array<"direct" | "group" | "channel" | "thread">;
    polls?: boolean;
    reactions?: boolean;
    edit?: boolean;
    unsend?: boolean;
    reply?: boolean;
    effects?: boolean;
    groupManagement?: boolean;
    threads?: boolean;
    media?: boolean;
    nativeCommands?: boolean;
    blockStreaming?: boolean;
  };

  // Lifecycle Adapters
  onboarding?: ChannelOnboardingAdapter;
  config: ChannelConfigAdapter<ResolvedAccount>;
  setup?: ChannelSetupAdapter;
  security?: ChannelSecurityAdapter<ResolvedAccount>;

  // Message Handling
  outbound?: ChannelOutboundAdapter;
  gateway?: ChannelGatewayAdapter<ResolvedAccount>;

  // Behaviors
  groups?: ChannelGroupAdapter;
  mentions?: ChannelMentionAdapter;
  threading?: ChannelThreadingAdapter;
  messaging?: ChannelMessagingAdapter;

  // Extensions
  commands?: ChannelCommandAdapter;
  actions?: ChannelMessageActionAdapter;
  agentTools?: ChannelAgentToolFactory | ChannelAgentTool[];
}
```

### 4.2 Supported Channels

#### Built-in Channels
| Channel | Library/SDK | Features |
|---------|-------------|----------|
| **Telegram** | grammY | Groups, threads, inline buttons, polls |
| **Discord** | discord.js | Guilds, roles, threads, components |
| **Slack** | Bolt | Workspaces, channels, threads, blocks |
| **Signal** | signal-cli | Groups, reactions, E.164 phone numbers |
| **iMessage** | BlueBubbles API | DMs, group chats (macOS required) |
| **WhatsApp** | Baileys | Groups, broadcast lists, polls |

#### Extension Channels
- Matrix, Microsoft Teams, Google Chat, IRC, Mattermost
- Feishu, LINE, Zalo, Nextcloud Talk, Twitch
- Voice Call (WebRTC)

### 4.3 Outbound Delivery

```typescript
interface ChannelOutboundAdapter {
  deliveryMode: "direct" | "gateway" | "hybrid";
  textChunkLimit?: number;  // Max chars per message
  chunker?: (text: string, limit: number) => string[];
  chunkerMode?: "text" | "markdown";

  sendText?: (ctx: ChannelOutboundContext) => Promise<OutboundDeliveryResult>;
  sendMedia?: (ctx: ChannelOutboundContext) => Promise<OutboundDeliveryResult>;
  sendPoll?: (ctx: ChannelPollContext) => Promise<ChannelPollResult>;
}

interface ChannelOutboundContext {
  cfg: rockbotConfig;
  to: string;              // Target ID/phone/email
  text: string;            // Message body
  mediaUrl?: string;       // Remote media URL
  replyToId?: string;      // Thread/reply parent
  threadId?: string | number;
  accountId?: string;
  identity?: OutboundIdentity;
  silent?: boolean;
}
```

### 4.4 Text Chunk Limits by Channel

| Channel | Limit (chars) |
|---------|---------------|
| Discord | 2000 |
| Telegram | 4000 |
| Slack | 4000 |
| Signal | 4000 |
| WhatsApp | 4000 |
| iMessage | 4000 |
| IRC | 350 |
| LINE | 5000 |

---

## 5. Routing System

### 5.1 Route Resolution

Messages are routed to agents through a hierarchical binding system:

```typescript
interface ResolvedAgentRoute {
  agentId: string;
  channel: string;
  accountId: string;
  sessionKey: string;
  mainSessionKey: string;
  lastRoutePolicy: "main" | "session";
  matchedBy: MatchedByType;
}

type MatchedByType =
  | "binding.peer"        // Thread/conversation-specific
  | "binding.peer.parent" // Inherited from parent thread
  | "binding.guild+roles" // Discord role-based
  | "binding.guild"       // Discord guild-wide
  | "binding.team"        // Teams team binding
  | "binding.account"     // Account-level default
  | "binding.channel"     // Channel-level default
  | "default";            // Global fallback
```

### 5.2 Binding Priority (Highest to Lowest)

1. **Peer Binding**: Specific thread/conversation → agent mapping
2. **Parent Peer**: Inherited from thread parent
3. **Guild + Roles**: Discord-specific role-based routing
4. **Guild**: Discord guild-wide binding
5. **Team**: Microsoft Teams team binding
6. **Account**: Account-level default agent
7. **Channel**: Channel-level default agent
8. **Default**: Global fallback agent

### 5.3 Session Key Format

Session keys uniquely identify conversations:

```
{scope}:{channel}:{identifier}

Examples:
  main:telegram:123456789
  direct:discord:user_987654321
  thread:slack:C123456_ts1234567890
```

---

## 6. Session Management

### 6.1 Session Entry Schema

```typescript
interface SessionEntry {
  // Identity
  sessionId: string;
  sessionFile?: string;
  label?: string;
  displayName?: string;

  // Execution Context
  spawnedBy?: string;
  spawnDepth?: number;

  // Message State
  abortedLastRun?: boolean;
  abortCutoffMessageSid?: string;
  abortCutoffTimestamp?: number;

  // Runtime Configuration
  thinkingLevel?: "off" | "minimal" | "low" | "medium" | "high";
  verboseLevel?: "on" | "off";
  reasoningLevel?: string;
  elevatedLevel?: string;

  // Model Selection
  model?: string;
  modelProvider?: string;
  providerOverride?: string;
  modelOverride?: string;
  authProfileOverride?: string;

  // Chat Configuration
  chatType?: "direct" | "group" | "channel";
  groupActivation?: "mention" | "always";
  sendPolicy?: "allow" | "deny";
  queueMode?: "steer" | "followup" | "collect" | "queue" | "interrupt";

  // Routing
  channel?: string;
  lastChannel?: string;
  lastTo?: string;
  lastAccountId?: string;
  lastThreadId?: string | number;
  origin?: SessionOrigin;

  // Token Tracking
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  contextTokens?: number;
  cacheRead?: number;
  cacheWrite?: number;

  // Maintenance
  compactionCount?: number;
  skillsSnapshot?: SessionSkillSnapshot;
  systemPromptReport?: SessionSystemPromptReport;
}
```

### 6.2 Transcript Storage

- **Format**: JSONL (JSON Lines)
- **Location**: `~/.rockbot/agents/{agentId}/sessions/{sessionId}.jsonl`
- **Entry Types**:
  - `user`: User messages
  - `assistant`: Agent responses
  - `tool_use`: Tool invocations
  - `tool_result`: Tool execution results
  - `system`: System messages

### 6.3 Session Scoping Modes

| Mode | Description |
|------|-------------|
| `per-sender` | Separate session per unique sender |
| `global` | Shared session across all senders |
| `per-peer` | Session per conversation/thread |
| `per-channel-peer` | Session per channel + conversation |
| `per-account-channel-peer` | Most specific: account + channel + conversation |

---

## 7. Agent System

### 7.1 Agent Execution Flow

```
1. Receive Message
   ├─ Route to Agent (binding resolution)
   └─ Load Session (transcript, metadata)

2. Build Context
   ├─ Load System Prompt (AGENTS.md, SOUL.md)
   ├─ Inject Skills (available tools)
   └─ Apply Thinking Level

3. Execute Agent
   ├─ Model Selection (primary/fallback)
   ├─ API Call (streaming response)
   ├─ Tool Execution Loop
   │   ├─ Parse Tool Calls
   │   ├─ Execute Tools
   │   └─ Return Results
   └─ Generate Final Response

4. Deliver Response
   ├─ Chunk by Channel Limits
   ├─ Send through Outbound Adapter
   └─ Update Session Transcript
```

### 7.2 Agent Configuration

```typescript
interface AgentConfig {
  workspace?: string;           // Working directory
  model?: string;               // Default model
  agentDir?: string;            // Agent-specific config dir
  bindings?: AgentBinding[];    // Channel/account bindings

  // Prompt Injection
  systemPrompt?: string;
  extraContext?: string[];

  // Tool Configuration
  tools?: {
    allow?: string[];
    disallow?: string[];
  };

  // Behavior
  blockStreaming?: boolean;
  maxToolCalls?: number;
  timeout?: number;
}
```

### 7.3 Model Failover

The agent runtime implements automatic failover:

1. **Primary Model**: Attempt with configured model
2. **Rate Limit Detection**: Detect 429/rate limit errors
3. **Auth Failure Tracking**: Track failed auth profiles
4. **Fallback Selection**: Try next model in priority order
5. **Cooldown Management**: Exponential backoff for failed providers

### 7.4 Thinking Levels

| Level | Description |
|-------|-------------|
| `off` | No extended thinking |
| `minimal` | Brief reasoning |
| `low` | Standard reasoning |
| `medium` | Detailed reasoning |
| `high` | Extensive step-by-step reasoning |

---

## 8. Tool System

### 8.1 Tool Definition

```typescript
interface AgentTool<TParams, TResult> {
  name: string;
  description: string;
  parameters: JSONSchema;       // JSON Schema for parameters
  execute: (params: TParams, context: ToolContext) => Promise<TResult>;

  // Optional flags
  ownerOnly?: boolean;          // Restrict to owner senders
  optional?: boolean;           // Requires explicit allowlist
}

interface ToolContext {
  config: rockbotConfig;
  workspaceDir: string;
  agentDir: string;
  agentId: string;
  sessionKey: string;
  sessionId: string;
  messageChannel?: string;
  requesterSenderId?: string;
  senderIsOwner?: boolean;
  sandboxed?: boolean;
}
```

### 8.2 Built-in Tools

| Tool | Description |
|------|-------------|
| `read` | Read file contents |
| `write` | Write file contents |
| `edit` | Edit file with diff |
| `exec` | Execute shell command |
| `patch` | Apply unified diff patch |
| `glob` | Find files by pattern |
| `grep` | Search file contents |
| `browser_navigate` | Navigate browser to URL |
| `browser_screenshot` | Capture browser screenshot |
| `memory_get` | Retrieve from memory store |
| `memory_search` | Search memory store |

### 8.3 Tool Execution Hooks

Plugins can intercept tool execution:

```typescript
// Before tool execution
type BeforeToolCallHook = (event: {
  tool: string;
  params: object;
}) => {
  params?: object;     // Modified params
  block?: boolean;     // Block execution
  blockReason?: string;
};

// After tool execution
type AfterToolCallHook = (event: {
  tool: string;
  params: object;
  result: object;
  error?: Error;
}) => void;
```

---

## 9. Plugin System

### 9.1 Plugin Registration API

```typescript
interface rockbotPluginApi {
  id: string;
  name: string;
  version?: string;
  config: rockbotConfig;
  runtime: PluginRuntime;
  logger: PluginLogger;

  // Registration Methods
  registerTool(tool: AgentTool, opts?: ToolOptions): void;
  registerHook(events: string[], handler: HookHandler): void;
  registerHttpRoute(params: HttpRouteParams): void;
  registerChannel(plugin: ChannelPlugin): void;
  registerGatewayMethod(method: string, handler: RequestHandler): void;
  registerCli(registrar: CliRegistrar): void;
  registerService(service: PluginService): void;
  registerProvider(provider: ProviderPlugin): void;
  registerCommand(command: CommandDefinition): void;
  registerContextEngine(id: string, factory: ContextEngineFactory): void;

  // Typed Event Handler
  on<K extends HookName>(hookName: K, handler: HookHandler<K>): void;
}
```

### 9.2 Hook Points

| Hook | Trigger |
|------|---------|
| `before_model_resolve` | Before model selection |
| `before_prompt_build` | Before system prompt assembly |
| `llm_input` | Before LLM API call |
| `llm_output` | After LLM response |
| `agent_end` | After agent completes |
| `message_received` | Inbound message received |
| `message_sending` | Before outbound send |
| `message_sent` | After send attempt |
| `before_tool_call` | Before tool execution |
| `after_tool_call` | After tool execution |
| `session_start` | New session created |
| `session_end` | Session ended |
| `gateway_start` | Gateway started |
| `gateway_stop` | Gateway stopped |

### 9.3 Plugin Loading Order

1. **Bundled Plugins**: Ship with core distribution
2. **Global Plugins**: Installed in global plugin directory
3. **Workspace Plugins**: Local to workspace
4. **Config Plugins**: Defined in configuration

### 9.4 Plugin Runtime Services

```typescript
interface PluginRuntime {
  // Configuration
  config: {
    loadConfig(): Promise<rockbotConfig>;
    writeConfigFile(content: string): Promise<void>;
  };

  // System Events
  system: {
    enqueueSystemEvent(event: SystemEvent): void;
    requestHeartbeatNow(): void;
    runCommandWithTimeout(cmd: string, timeout: number): Promise<string>;
  };

  // Media Processing
  media: {
    loadWebMedia(url: string): Promise<Buffer>;
    detectMime(buffer: Buffer): string;
    resizeToJpeg(buffer: Buffer, maxSize: number): Promise<Buffer>;
  };

  // Text-to-Speech / Speech-to-Text
  tts: {
    textToSpeechTelephony(text: string): Promise<Buffer>;
  };
  stt: {
    transcribeAudioFile(path: string): Promise<string>;
  };

  // Subagent Spawning
  subagent: {
    run(params: SubagentRunParams): Promise<SubagentRunResult>;
    waitForRun(params: SubagentWaitParams): Promise<SubagentWaitResult>;
    getSessionMessages(params: GetMessagesParams): Promise<Message[]>;
    deleteSession(params: DeleteSessionParams): Promise<void>;
  };
}
```

---

## 10. Configuration System

### 10.1 Configuration File

- **Location**: `~/.rockbot/config.json5`
- **Format**: JSON5 (supports comments, trailing commas)
- **Hot Reload**: Changes applied without restart (most settings)

### 10.2 Top-Level Configuration Sections

```typescript
interface rockbotConfig {
  // Authentication
  auth: {
    profiles: Record<string, AuthProfile>;
    order?: Record<string, string[]>;
  };

  // Model Configuration
  // NOTE: The `providers` map feeds the gateway's Provider Registry at startup.
  // Actual available providers are determined at runtime based on credentials
  // and feature flags. Interfaces must query /api/providers (or providers.list RPC)
  // to discover available providers — never hardcode or instantiate them directly.
  models: {
    primary?: string;
    providers?: Record<string, ProviderConfig>;
    definitions?: ModelDefinition[];
  };

  // Session Behavior
  session: {
    scope?: "per-sender" | "global";
    dmScope?: "main" | "per-peer" | "per-channel-peer";
    idleMinutes?: number;
    typingMode?: "never" | "instant" | "thinking" | "message";
    reset?: {
      mode: "daily" | "idle";
      atHour?: number;
      idleMinutes?: number;
    };
  };

  // Gateway Server
  gateway: {
    mode?: "local" | "remote" | "socket";
    bind?: string;
    port?: number;
    token?: string;
    password?: string;
    sslCert?: string;
    sslKey?: string;
  };

  // Logging
  logging: {
    level?: "silent" | "fatal" | "error" | "warn" | "info" | "debug" | "trace";
    file?: string;
    maxFileBytes?: number;
    consoleLevel?: string;
    redactSensitive?: "off" | "tools";
  };

  // Per-Channel Settings
  channels: {
    telegram?: TelegramConfig;
    discord?: DiscordConfig;
    slack?: SlackConfig;
    signal?: SignalConfig;
    // ... other channels
  };

  // Agent Definitions
  agents: Record<string, AgentConfig>;

  // Tool Configuration
  tools: {
    alsoAllow?: string[];
    disallow?: string[];
    sandbox?: { enabled?: boolean };
  };

  // Environment Variables
  env: {
    vars?: Record<string, string>;
    shellEnv?: boolean;
  };

  // Cron Jobs
  cron: CronJobConfig[];

  // Plugins
  plugins: PluginConfig[];
}
```

### 10.3 Model Aliases

Built-in shortcuts for common models:

| Alias | Model ID |
|-------|----------|
| `opus` | `anthropic/claude-opus-4-6` |
| `sonnet` | `anthropic/claude-sonnet-4-6` |
| `gpt` | `openai/gpt-5.4` |
| `gpt-mini` | `openai/gpt-5-mini` |
| `gemini` | `google/gemini-3.1-pro-preview` |
| `gemini-flash` | `google/gemini-3-flash-preview` |

### 10.4 Environment Variables

| Variable | Description |
|----------|-------------|
| `ROCKBOT_GATEWAY_TOKEN` | Gateway authentication token |
| `ROCKBOT_GATEWAY_PASSWORD` | Gateway password |
| `ROCKBOT_LOG_LEVEL` | Override log level |
| `ROCKBOT_PROFILE` | Load specific profile |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `GOOGLE_API_KEY` | Google AI API key |

---

## 11. CLI Interface

### 11.1 Command Structure

```
rockbot <command> [subcommand] [options]
```

### 11.2 Core Commands

| Command | Description |
|---------|-------------|
| `setup` | Initialize workspace and configuration |
| `onboard` | Interactive onboarding wizard |
| `configure` | Configure credentials and channels |
| `config` | Non-interactive config operations |
| `gateway run` | Start the gateway server |
| `gateway dev` | Start in development mode |
| `status` | Show channel health and sessions |
| `doctor` | Health checks and diagnostics |
| `agent` | Run agent turn via gateway |
| `agents` | Manage agent configurations |
| `message send` | Send message through channel |
| `sessions` | List and manage sessions |
| `channels` | Manage channel connections |
| `models` | Configure AI models |
| `cron` | Manage scheduled jobs |
| `plugins` | Manage plugins |

### 11.3 Key Command Options

#### `rockbot agent`
```
--message <text>      Message body (required)
--to <number>         Recipient (E.164 format)
--session-id <id>     Explicit session ID
--agent <id>          Agent ID override
--thinking <level>    Thinking level
--channel <channel>   Delivery channel
--local               Run embedded agent
--deliver             Deliver response to channel
--json                Output as JSON
--timeout <seconds>   Command timeout
```

#### `rockbot message send`
```
--message <text>      Message body
--media <path>        Attach media file
--buttons <json>      Inline keyboard (Telegram)
--components <json>   Components payload (Discord)
--reply-to <id>       Reply to message ID
--thread-id <id>      Thread ID
--silent              Send silently
```

---

## 12. Message Context

### 12.1 Inbound Message Context

```typescript
interface MsgContext {
  // Message Content
  Body: string;                    // Processed message body
  BodyForAgent: string;            // Agent-facing body
  RawBody: string;                 // Original raw body

  // Sender Information
  From: string;                    // Sender identifier
  SenderId: string;                // Canonical sender ID
  SenderName?: string;             // Display name
  SenderUsername?: string;         // Username if available
  SenderE164?: string;             // E.164 phone number

  // Message Metadata
  MessageSid: string;              // Message ID
  ReplyToId?: string;              // Parent message ID
  ReplyToBody?: string;            // Parent message body
  MessageThreadId?: string | number;

  // Forward Context
  ForwardedFrom?: string;
  ForwardedFromId?: string;
  ForwardedDate?: number;

  // Chat Context
  ChatType: "direct" | "group" | "channel";
  GroupSubject?: string;
  GroupChannel?: string;
  ConversationLabel?: string;

  // Media
  MediaPath?: string;
  MediaUrl?: string;
  MediaType?: string;
  MediaPaths?: string[];
  MediaUrls?: string[];

  // Session Routing
  SessionKey: string;
  AccountId?: string;
  OriginatingChannel: string;

  // History
  InboundHistory?: Array<{
    sender: string;
    body: string;
    timestamp?: number;
  }>;

  // Flags
  WasMentioned?: boolean;
  Timestamp: number;
}
```

### 12.2 Message Processing Pipeline

```
1. Platform Event
   ↓
2. Raw Message Extraction
   ↓
3. Normalization (MsgContext creation)
   ↓
4. Access Control Check
   ├─ DM Policy (open/closed/allowlist)
   ├─ Group Policy (mention required?)
   └─ Sender Authorization
   ↓
5. Route Resolution
   ├─ Find matching binding
   └─ Determine session key
   ↓
6. Context Finalization
   ├─ Load media (download if URL)
   ├─ Build thread context
   └─ Resolve session metadata
   ↓
7. Dispatch to Agent
```

---

## 13. Cron System

### 13.1 Cron Job Schema

```typescript
interface CronJob {
  id: string;
  name: string;
  description?: string;
  enabled: boolean;

  // Targeting
  agentId?: string;
  sessionKey?: string;

  // Schedule
  schedule: CronSchedule;

  // Execution
  payload: CronPayload;
  sessionTarget: "main" | "isolated";
  wakeMode: "next-heartbeat" | "now";

  // Delivery
  delivery?: {
    type: "none" | "announce" | "webhook";
    channels?: string[];
    webhookUrl?: string;
  };

  // State
  state: {
    nextRunAtMs?: number;
    lastRunAtMs?: number;
    lastRunStatus?: "ok" | "error" | "skipped";
    lastError?: string;
    consecutiveErrors?: number;
  };
}
```

### 13.2 Schedule Types

```typescript
type CronSchedule =
  | { type: "at"; atMs: number }              // One-time at timestamp
  | { type: "every"; intervalMs: number }     // Repeating interval
  | { type: "cron"; expression: string };     // Cron expression
```

### 13.3 Payload Types

```typescript
type CronPayload =
  | { type: "systemEvent"; event: string; data?: object }
  | { type: "agentTurn"; message: string; extraSystemPrompt?: string };
```

---

## 14. Security Model

RockBot implements defense-in-depth security with a core principle: **credentials never cross the agent boundary**. Sensitive data is stored encrypted, injected at tool execution time, and sanitized from responses.

### 14.1 Threat Model

#### Adversaries

| Threat Actor | Capabilities | Mitigations |
|--------------|--------------|-------------|
| **Compromised Agent** | Prompt injection, tool abuse | Permission system, HIL approval, credential isolation |
| **Local Attacker** | File system access | Encryption at rest, memory zeroization |
| **Network Attacker** | Traffic interception | TLS, auth tokens, no plaintext secrets in transit |
| **Malicious Plugin** | Code execution in gateway | Capability restrictions, sandboxing |

#### Trust Boundaries

```
┌─────────────────────────────────────────────────────────────────┐
│                    UNTRUSTED ZONE                                │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
│  │   Agent     │  │   Plugins   │  │  External   │              │
│  │  (LLM)      │  │             │  │  Services   │              │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘              │
└─────────┼────────────────┼────────────────┼─────────────────────┘
          │                │                │
          ▼                ▼                ▼
┌─────────────────────────────────────────────────────────────────┐
│                     TRUST BOUNDARY                               │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              Credential Manager (Gateway)                 │   │
│  │  • Permission evaluation                                  │   │
│  │  • HIL approval queue                                     │   │
│  │  • Audit logging                                          │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────────┐
│                    TRUSTED ZONE                                  │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                  Credential Vault                        │    │
│  │  • Encrypted storage (AES-256-GCM)                       │    │
│  │  • Master key (Argon2id derived)                         │    │
│  │  • Hash-chained audit log                                │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### 14.2 Authentication Modes

| Mode | Description |
|------|-------------|
| `token` | Static token authentication |
| `password` | Password-based authentication |
| `loopback` | Localhost bypass (no auth required) |
| `tailscale` | Tailscale network identity |
| `device` | Device pairing (mobile apps) |

### 14.3 DM Policies

| Policy | Description |
|--------|-------------|
| `open` | Accept messages from anyone |
| `closed` | Reject all DMs |
| `allowlist` | Only accept from configured senders |
| `pairing` | Require pairing code for new senders |

### 14.4 Group Policies

| Setting | Description |
|---------|-------------|
| `requireMention` | Only respond when mentioned |
| `toolPolicy` | Allow/block specific tools in groups |
| `groupActivation` | `mention` or `always` |

### 14.5 Rate Limiting

- **Auth Failures**: Exponential backoff on failed attempts
- **Browser Origins**: Stricter limits for web clients
- **Loopback Exemption**: Configurable localhost bypass

### 14.6 Credential Vault Architecture

The credential vault provides secure storage for API keys, tokens, passwords, and certificates.

#### Storage Structure

```
~/.local/share/rockbot/credentials/
├── vault.json           # Encrypted vault metadata
├── endpoints/           # Endpoint configurations (encrypted)
│   └── {uuid}.json
├── credentials/         # Encrypted credential blobs
│   └── {uuid}.enc
├── permissions.json     # Permission rules
└── audit.log           # Hash-chained audit trail
```

#### Encryption Scheme

| Layer | Algorithm | Purpose |
|-------|-----------|---------|
| **Key Derivation** | Argon2id | Password → Master Key |
| **Symmetric Encryption** | AES-256-GCM | Credential encryption |
| **Hash Chain** | SHA-256 | Audit log integrity |

```typescript
interface EncryptedCredential {
  id: string;              // UUID
  endpointId: string;      // Parent endpoint UUID
  type: CredentialType;    // bearer_token, api_key, oauth2, etc.
  nonce: string;           // 12-byte nonce (hex)
  ciphertext: string;      // Encrypted secret (base64)
  createdAt: string;       // ISO timestamp
  expiresAt?: string;      // Optional expiration
}
```

#### Master Key Derivation

```
Password + Salt → Argon2id(m=65536, t=3, p=4) → 256-bit Master Key
```

- **Memory**: 64 MiB (hardened against GPU attacks)
- **Iterations**: 3 passes
- **Parallelism**: 4 lanes
- **Salt**: 16 bytes, randomly generated per vault

#### Credential Types

| Type | Description | Use Case |
|------|-------------|----------|
| `bearer_token` | Bearer authentication token | API access |
| `api_key` | Static API key | Service authentication |
| `oauth2` | OAuth2 tokens with refresh | Google, GitHub, etc. |
| `basic_auth` | Username + password pair | Legacy services |
| `certificate` | TLS client certificate | mTLS |
| `raw_secret` | Arbitrary secret data | Custom integrations |

### 14.7 Permission System

Permissions control which operations agents can perform without human intervention.

#### Permission Levels

| Level | Description | Use Case |
|-------|-------------|----------|
| `Allow` | Execute immediately | Read-only operations |
| `AllowHIL` | Require human approval | State-changing operations |
| `AllowHIL2FA` | Require approval + 2FA | Destructive operations |
| `Deny` | Block and log attempt | Forbidden operations |

#### Permission Rules

```typescript
interface Permission {
  id: string;              // UUID
  endpointId: string;      // Target endpoint
  pathPattern: string;     // Glob pattern (e.g., "/api/states/**")
  method?: HttpMethod;     // Optional method filter
  permissionLevel: PermissionLevel;
  createdAt: string;
}
```

#### Pattern Matching

- `*` matches any sequence except `/`
- `**` matches any sequence including `/`
- `?` matches any single character except `/`

```
/api/states          → Exact match only
/api/states/*        → /api/states/foo but NOT /api/states/foo/bar
/api/states/**       → Any path under /api/states/
/api/services/*/turn_on → /api/services/light/turn_on
```

#### Evaluation Priority

When multiple rules match, specificity determines precedence:

1. **Exact path** beats patterns
2. **Longer patterns** beat shorter ones
3. **Method-specific** beats wildcard method
4. **Most restrictive** wins ties (Deny > AllowHIL2FA > AllowHIL > Allow)

### 14.8 Human-in-the-Loop (HIL) System

The HIL system enables human oversight for sensitive operations.

#### Approval Flow

```
Agent requests credential
    │
    ▼
Permission check: AllowHIL or AllowHIL2FA
    │
    ▼
Create ApprovalRequest
    │
    ├─────────────────────────────────────┐
    │                                     │
    ▼                                     ▼
Push notification to            API endpoint for
human (TUI/Web/Mobile)          programmatic approval
    │                                     │
    └────────────────┬────────────────────┘
                     │
                     ▼
              Human decides
                     │
         ┌──────────┴──────────┐
         │                     │
         ▼                     ▼
      Approve               Deny
         │                     │
         ▼                     ▼
   Release credential    Return error
   to pending request    to agent
```

#### Approval Request Schema

```typescript
interface ApprovalRequest {
  id: string;              // UUID
  endpointId: string;      // Target endpoint
  path: string;            // Requested path
  method: HttpMethod;      // HTTP method
  requiredLevel: PermissionLevel;
  requestedAt: string;     // ISO timestamp
  expiresAt: string;       // Auto-deny after timeout
  status: "pending" | "approved" | "denied" | "expired";
  resolvedBy?: string;     // Human identifier
  resolvedAt?: string;
  denialReason?: string;
}
```

#### Notification Channels

| Channel | Delivery | Latency |
|---------|----------|---------|
| TUI | Direct terminal prompt | Immediate |
| Web UI | WebSocket push | < 1 second |
| Mobile | Push notification | < 5 seconds |
| Messaging | Telegram/Discord/etc. | < 10 seconds |

#### Timeout Behavior

- Default timeout: 5 minutes
- Configurable per-endpoint
- Expired requests automatically denied
- Consecutive timeouts may trigger cooldown

### 14.9 Audit Logging

All credential operations are logged in a tamper-evident hash chain.

#### Audit Entry Schema

```typescript
interface AuditEntry {
  sequence: number;        // Monotonic sequence number
  timestamp: string;       // ISO timestamp
  requestId: string;       // Correlation ID
  source: string;          // Requester (agent ID, user, etc.)
  endpointId: string;      // Target endpoint
  method: HttpMethod;      // Operation method
  path: string;            // Resource path
  parametersHash: string;  // SHA-256 of parameters
  permissionLevel: PermissionLevel;
  approvalId?: string;     // HIL approval reference
  resultStatus: ResultStatus;
  resultHash: string;      // SHA-256 of result
  errorMessage?: string;
  previousHash: string;    // Hash of previous entry
  entryHash: string;       // Hash of this entry
}

type ResultStatus = "success" | "denied" | "error" | "timeout";
```

#### Hash Chain Integrity

Each entry's hash includes:
- All entry fields (sequence, timestamp, source, etc.)
- Previous entry's hash

```
Entry N hash = SHA256(
  sequence || timestamp || requestId || ... || previousHash
)
```

#### Verification

```bash
# Verify audit log integrity
rockbot credentials audit --verify
```

Verification checks:
1. Sequential sequence numbers
2. Valid hash chain (no gaps or tampering)
3. Timestamp ordering
4. Hash computation correctness

### 14.10 Credential Injection Flow

```
1. Agent requests tool execution
   Tool: home_assistant_api
   Params: { path: "/api/services/light/turn_on", method: "POST" }
       │
       ▼
2. Gateway intercepts tool call
   Detects credential requirement: rockbot://homeassistant/api/**
       │
       ▼
3. Permission evaluation
   CredentialManager.checkPermission(endpointId, "POST", "/api/services/light/turn_on")
       │
       ├── Allow → Continue to step 4
       ├── AllowHIL → Create approval request, wait for human
       └── Deny → Return error to agent
       │
       ▼
4. Credential retrieval
   vault.decryptCredential(endpointId)
       │
       ▼
5. Credential injection
   headers["Authorization"] = "Bearer " + credential
       │
       ▼
6. Execute tool with injected credentials
       │
       ▼
7. Sanitize response
   Remove any credential echoes from tool output
       │
       ▼
8. Audit logging
   Log operation with result hash
       │
       ▼
9. Return sanitized result to agent
```

### 14.11 Memory Safety

#### Zeroization

All sensitive data is zeroized when no longer needed:

```rust
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    key: [u8; 32],
}
```

- Master keys zeroized on drop
- Decrypted credentials zeroized after use
- Password buffers cleared immediately after key derivation

#### Memory Protection

- Avoid swapping sensitive pages (mlock where available)
- No logging of credential values
- Redact credentials in error messages

### 14.12 Configuration

```toml
[credentials]
enabled = true
vault_path = "~/.local/share/rockbot/credentials"
unlock_method = "env"  # "env", "password", "keyring", "yubikey"
password_env_var = "ROCKBOT_VAULT_PASSWORD"
default_permission = "deny"
hil_timeout_seconds = 300
audit_retention_days = 90

# Per-endpoint permission defaults
[[credentials.endpoints]]
id = "homeassistant"
type = "home_assistant"
base_url = "http://homeassistant.local:8123"
default_permission = "allow_hil"

[[credentials.endpoints.permissions]]
path = "/api/states/**"
method = "GET"
level = "allow"

[[credentials.endpoints.permissions]]
path = "/api/services/**"
method = "POST"
level = "allow_hil"

[[credentials.endpoints.permissions]]
path = "/api/config/**"
level = "deny"
```

### 14.13 API Endpoints (Credentials)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/credentials/status` | Vault status (locked/unlocked, counts) |
| POST | `/api/credentials/unlock` | Unlock vault with password |
| POST | `/api/credentials/lock` | Lock vault |
| GET | `/api/credentials/endpoints` | List endpoints (no secrets) |
| POST | `/api/credentials/endpoints` | Create endpoint |
| DELETE | `/api/credentials/endpoints/:id` | Delete endpoint |
| POST | `/api/credentials/endpoints/:id/credential` | Store credential |
| GET | `/api/credentials/permissions` | List permission rules |
| POST | `/api/credentials/permissions` | Add permission rule |
| DELETE | `/api/credentials/permissions/:id` | Remove permission rule |
| GET | `/api/credentials/audit` | View audit log |
| GET | `/api/credentials/approvals` | List pending HIL approvals |
| POST | `/api/credentials/approvals/:id/approve` | Approve HIL request |
| POST | `/api/credentials/approvals/:id/deny` | Deny HIL request |

### 14.14 Provider Registry API Endpoints

The gateway exposes provider state through the following HTTP API. All interfaces (TUI, WebUI, CLI) must use these endpoints to discover and interact with LLM providers — they must never hardcode provider lists or call LLM APIs directly.

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/providers` | List all registered providers with status, auth type, and available models |
| GET | `/api/providers/:id` | Get details for a specific provider |
| POST | `/api/providers/:id/test` | Test provider connectivity and authentication |
| POST | `/api/chat` | Send a chat message through the gateway (replaces direct LLM calls from interfaces) |

LLM providers register with the gateway at startup based on the `models.providers` configuration and available credentials. The `/api/providers` response reflects the live runtime state — which providers are reachable, authenticated, and which models are available.

### 14.15 Security Checklist

#### Implementation Requirements

- [ ] Master key derived with Argon2id (OWASP parameters)
- [ ] AES-256-GCM for all credential encryption
- [ ] Unique nonce per encryption operation
- [ ] Hash-chained audit log with SHA-256
- [ ] Zeroization of all sensitive memory
- [ ] Permission evaluation before any credential access
- [ ] HIL timeout and expiration handling
- [ ] Response sanitization for credential leaks
- [ ] Rate limiting on unlock attempts
- [ ] Secure random number generation (OS CSPRNG)

#### Operational Security

- [ ] Vault password not stored in config files
- [ ] Audit log backed up regularly
- [ ] Permission rules reviewed periodically
- [ ] HIL notifications tested and working
- [ ] Credential rotation schedule established

---

## 15. Data Storage

### 15.1 Directory Structure

```
~/.rockbot/
├── config.json5              # Main configuration
├── credentials/              # Encrypted credentials
├── agents/
│   └── {agentId}/
│       └── sessions/
│           └── {sessionId}.jsonl   # Transcript files
├── sessions/                 # Session metadata (SQLite)
├── logs/                     # Log files
├── plugins/                  # Installed plugins
├── models.json               # Discovered models cache
└── state/                    # Runtime state
```

### 15.2 Transcript Format (JSONL)

Each line is a JSON object representing a transcript entry:

```jsonl
{"role":"user","content":"Hello","timestamp":1709913600000,"sid":"msg_001"}
{"role":"assistant","content":"Hi there!","timestamp":1709913601000,"sid":"msg_002"}
{"role":"tool_use","name":"read","params":{"path":"/file.txt"},"timestamp":1709913602000,"sid":"tool_001"}
{"role":"tool_result","name":"read","result":"file contents","timestamp":1709913603000,"sid":"tool_001"}
```

### 15.3 Session Compaction

When transcripts grow large:
1. Summarize older messages
2. Preserve recent N messages
3. Update compaction count
4. Maintain context continuity

---

## 16. Skills System

### 16.1 Skill Definition

```typescript
interface Skill {
  name: string;
  description: string;
  content: string;              // Skill prompt/instructions

  // Metadata
  metadata?: {
    always?: boolean;           // Always include
    skillKey?: string;
    emoji?: string;
    homepage?: string;
    os?: string[];              // Supported OS
    requires?: {
      bins?: string[];          // Required binaries
      env?: string[];           // Environment variables
      config?: string[];        // Config keys
    };
  };

  // Installation
  install?: InstallSpec[];
}

interface InstallSpec {
  kind: "brew" | "node" | "go" | "uv" | "download";
  label?: string;
  formula?: string;             // For brew
  package?: string;             // For node/go
  url?: string;                 // For download
  os?: string[];
}
```

### 16.2 Skill Discovery

1. **Bundled**: `{packageRoot}/skills/`
2. **Workspace**: Configured in `rockbot.config.ts`
3. **Agent-specific**: Per-agent skill filters

### 16.3 Skill Invocation

```typescript
interface SkillInvocationPolicy {
  userInvocable: boolean;       // User can invoke directly
  disableModelInvocation: boolean;  // Model cannot invoke
}
```

---

## 17. Media Pipeline

### 17.1 Supported Media Types

| Type | Extensions | Processing |
|------|------------|------------|
| Image | jpg, png, gif, webp | Resize, convert to JPEG |
| Audio | mp3, wav, ogg, m4a | Transcription (STT) |
| Video | mp4, webm, mov | Thumbnail extraction |
| Document | pdf, txt, md | Text extraction |

### 17.2 Media Processing Functions

```typescript
interface MediaPipeline {
  loadWebMedia(url: string): Promise<Buffer>;
  detectMime(buffer: Buffer): string;
  mediaKindFromMime(mime: string): "image" | "audio" | "video" | "document";

  // Image Processing
  getImageMetadata(buffer: Buffer): Promise<ImageMetadata>;
  resizeToJpeg(buffer: Buffer, maxSize: number): Promise<Buffer>;

  // Audio Processing
  isVoiceCompatibleAudio(path: string): boolean;
  transcribeAudioFile(path: string): Promise<string>;
  textToSpeechTelephony(text: string): Promise<Buffer>;
}
```

---

## 18. Error Handling

### 18.1 Error Categories

| Category | Description |
|----------|-------------|
| `AUTH_FAILED` | Authentication failure |
| `RATE_LIMITED` | Rate limit exceeded |
| `INVALID_PARAMS` | Invalid request parameters |
| `NOT_FOUND` | Resource not found |
| `CHANNEL_ERROR` | Channel delivery failure |
| `MODEL_ERROR` | Model API error |
| `TOOL_ERROR` | Tool execution failure |
| `TIMEOUT` | Operation timeout |

### 18.2 Error Response Format

```typescript
interface ErrorResponse {
  code: string;
  message: string;
  details?: object;
  retryable?: boolean;
  retryAfterMs?: number;
}
```

### 18.3 Retry Strategy

1. **Retryable Errors**: Rate limits, transient failures
2. **Exponential Backoff**: 1s, 2s, 4s, 8s, 16s, 32s max
3. **Circuit Breaker**: Disable failing providers temporarily
4. **Fallback**: Try alternative models/providers

---

## 19. Observability

### 19.1 Logging

- **Format**: JSON structured logs
- **Levels**: silent, fatal, error, warn, info, debug, trace
- **Rotation**: Configurable max file size
- **Redaction**: Sensitive data masking

### 19.2 Events

System events broadcast via WebSocket:

```typescript
interface SystemEvent {
  type: "info" | "warning" | "error";
  source: string;
  message: string;
  timestamp: number;
  metadata?: object;
}
```

### 19.3 Health Monitoring

```typescript
interface HealthSnapshot {
  channels: Record<string, ChannelHealth>;
  agents: Record<string, AgentHealth>;
  system: SystemHealth;
}

interface ChannelHealth {
  status: "online" | "offline" | "degraded";
  lastActivity?: number;
  error?: string;
}
```

---

## 20. User Interface Design

RockBot provides two unified interfaces: a **Terminal UI (TUI)** for power users and a **Web UI** for browser-based access. Both interfaces share the same navigation structure, design language, and state model to ensure consistency.

### 20.1 Design Philosophy

| Principle | Description |
|-----------|-------------|
| **Unified Navigation** | Same 6-section structure in both TUI and Web UI |
| **Elm-like Architecture** | Single state source, message-driven updates, pure render functions |
| **Dark-first Aesthetic** | Cyberpunk-inspired palette optimized for extended use |
| **Keyboard-first** | Full keyboard navigation; mouse/touch optional |
| **Real-time Updates** | WebSocket-driven state sync across clients |

### 20.2 Navigation Structure

Both interfaces share identical navigation sections:

| Section | Icon | Description | Sub-tabs |
|---------|------|-------------|----------|
| **Dashboard** | 📊 | System overview, health, quick stats | — |
| **Credentials** | 🔐 | Vault management, endpoints, permissions | Endpoints, Providers, Permissions, Audit |
| **Agents** | 🤖 | Agent configuration, bindings | — |
| **Sessions** | 💬 | Active sessions, chat interface | — |
| **Models** | 🧠 | LLM provider configuration | — |
| **Settings** | ⚙️ | Gateway config, paths, about | — |

### 20.3 Color Palette

```css
:root {
  --bg: #0f0f1a;           /* Deep background */
  --surface: #1a1a2e;      /* Card/panel background */
  --surface-2: #232342;    /* Hover/selected state */
  --primary: #e94560;      /* Accent (coral red) */
  --secondary: #0f3460;    /* Secondary accent */
  --accent: #7c3aed;       /* Purple highlights */
  --text: #f0f0f0;         /* Primary text */
  --text-dim: #8888aa;     /* Secondary text */
  --success: #10b981;      /* Green status */
  --warning: #f59e0b;      /* Yellow/orange warnings */
  --error: #ef4444;        /* Red errors */
  --border: #2a2a4a;       /* Subtle borders */
}
```

### 20.4 Terminal UI (TUI)

Built with **ratatui** (Rust TUI framework), the TUI provides full functionality in terminal environments.

#### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         App                                  │
│  ┌─────────────┐  ┌──────────────────────────────────────┐  │
│  │   State     │  │           Event Loop                  │  │
│  │  (AppState) │◄─┤  • Crossterm key events               │  │
│  └──────┬──────┘  │  • Async message channel              │  │
│         │         │  • Tick timer (animations)            │  │
│         ▼         └──────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                    Components                        │    │
│  │  sidebar │ dashboard │ credentials │ sessions │ ... │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

#### State Management

```rust
/// Central application state
pub struct AppState {
    // Navigation
    pub menu_item: MenuItem,
    pub sidebar_focus: bool,
    pub credentials_tab: usize,
    
    // Data
    pub gateway: GatewayStatus,
    pub vault: VaultStatus,
    pub agents: Vec<AgentInfo>,
    pub sessions: Vec<SessionInfo>,
    pub endpoints: Vec<EndpointInfo>,
    
    // UI State
    pub input_mode: InputMode,
    pub status_message: Option<(String, bool)>,
    pub should_exit: bool,
    
    // Async communication
    pub tx: mpsc::UnboundedSender<Message>,
}

/// Input modes for modal handling
pub enum InputMode {
    Normal,
    PasswordInput { prompt: String, masked: bool, action: PasswordAction },
    AddCredential(AddCredentialState),
    EditCredential(EditCredentialState),
    Confirm { message: String, action: ConfirmAction },
    ChatInput,
    ViewSession { session_id: String },
}
```

#### Message Types

```rust
pub enum Message {
    // Navigation
    Navigate(MenuItem),
    ToggleSidebar,
    
    // Data loading
    GatewayStatus(GatewayStatus),
    AgentsLoaded(Vec<AgentInfo>),
    SessionsLoaded(Vec<SessionInfo>),
    VaultStatus(VaultStatus),
    EndpointsLoaded(Vec<EndpointInfo>),
    
    // Chat
    ChatResponse(String),
    ChatStreamChunk(String),
    
    // UI feedback
    SetStatus(String, bool),
    Tick,
    Quit,
}
```

#### Key Bindings

| Key | Context | Action |
|-----|---------|--------|
| `Tab` | Global | Toggle sidebar/content focus |
| `j/k` or `↓/↑` | Sidebar | Navigate menu items |
| `Enter` | Sidebar | Select menu item |
| `[` / `]` | Content | Previous/next sub-tab |
| `a` | Credentials | Add endpoint |
| `d` | Credentials | Delete selected |
| `u` | Credentials | Unlock vault |
| `Enter` | Sessions | Start chat |
| `Esc` | Modal | Cancel/close |
| `Ctrl+C` | Global | Exit |

#### Component Structure

```
tui/
├── app.rs           # Main app loop, event handling
├── state.rs         # AppState, Message, data types
├── effects.rs       # Visual animations
├── components/
│   ├── mod.rs       # Component exports, helpers
│   ├── sidebar.rs   # Navigation sidebar
│   ├── dashboard.rs # Dashboard view
│   ├── credentials.rs # Credential vault UI
│   ├── agents.rs    # Agent list/config
│   ├── sessions.rs  # Session list + chat
│   ├── models.rs    # Provider configuration
│   ├── settings.rs  # Gateway settings
│   └── modals.rs    # Password, confirm, forms
```

### 20.5 Web UI

Embedded single-page application served by the gateway. No build step required—pure HTML/CSS/JS.

#### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Browser                                  │
│  ┌─────────────┐  ┌──────────────────────────────────────┐  │
│  │   State     │  │           Event Handlers              │  │
│  │ (JS globals)│◄─┤  • Navigation clicks                  │  │
│  └──────┬──────┘  │  • Keyboard shortcuts (1-6)           │  │
│         │         │  • Form submissions                   │  │
│         ▼         └──────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                    DOM Updates                       │    │
│  │  Sidebar │ Page content │ Modals                     │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                   │
│                          ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              REST API + WebSocket                    │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

#### Layout Structure

```html
<div class="app">
  <aside class="sidebar">
    <div class="logo">🦀 RockBot</div>
    <ul class="nav">
      <li class="nav-item" data-page="dashboard">📊 Dashboard</li>
      <li class="nav-item" data-page="credentials">🔐 Credentials</li>
      <!-- ... -->
    </ul>
    <div class="sidebar-footer">v0.1.0</div>
  </aside>
  
  <main class="main">
    <div id="page-dashboard" class="content page">...</div>
    <div id="page-credentials" class="content page hidden">...</div>
    <!-- ... -->
  </main>
</div>
```

#### API Endpoints (Web UI)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Gateway health + agent list |
| GET | `/api/credentials/status` | Vault status |
| POST | `/api/credentials/init` | Initialize vault |
| POST | `/api/credentials/unlock` | Unlock vault |
| POST | `/api/credentials/lock` | Lock vault |
| GET | `/api/credentials/endpoints` | List endpoints |
| POST | `/api/credentials/endpoints` | Add endpoint |
| DELETE | `/api/credentials/endpoints/:id` | Remove endpoint |
| GET | `/api/sessions` | List sessions |
| POST | `/api/chat` | Send chat message |
| GET | `/api/agents` | List agents |
| POST | `/api/gateway/reload` | Reload config |

#### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `1` | Dashboard |
| `2` | Credentials |
| `3` | Agents |
| `4` | Sessions |
| `5` | Models |
| `6` | Settings |

### 20.6 Component Design Patterns

#### Cards

Used for grouping related content:

```html
<div class="card">
  <div class="card-header">
    <h3>Title</h3>
    <button class="btn btn-primary btn-sm">Action</button>
  </div>
  <!-- Content -->
</div>
```

#### Tables

For data lists with consistent styling:

```html
<table>
  <thead><tr><th>Column</th><th>Status</th><th>Actions</th></tr></thead>
  <tbody>
    <tr>
      <td>Value</td>
      <td><span class="badge badge-success">Active</span></td>
      <td><button class="btn btn-danger btn-sm">Delete</button></td>
    </tr>
  </tbody>
</table>
```

#### Badges

Status indicators:

```html
<span class="badge badge-success">Online</span>
<span class="badge badge-warning">Pending</span>
<span class="badge badge-error">Offline</span>
<span class="badge badge-info">Info</span>
```

#### Modals

Overlay dialogs for forms:

```html
<div class="modal-overlay">
  <div class="modal">
    <h2>Title</h2>
    <div class="form-group">
      <label>Field</label>
      <input type="text" placeholder="...">
    </div>
    <div class="modal-actions">
      <button class="btn btn-secondary">Cancel</button>
      <button class="btn btn-primary">Save</button>
    </div>
  </div>
</div>
```

### 20.7 Extension Guidelines

When adding new features to the UI:

#### Adding a New Section

1. **State**: Add `MenuItem` variant and any data fields to `AppState`
2. **TUI Component**: Create `components/newview.rs` with `render_newview()`
3. **Web UI**: Add page div, nav item, and `loadNewviewPage()` function
4. **Navigation**: Update `MenuItem::all()` and keyboard shortcuts

#### Adding a Sub-tab

1. **State**: Add tab enum (e.g., `CredentialsTab`) and state field
2. **TUI**: Handle `[`/`]` navigation in `handle_normal_mode()`
3. **Web UI**: Add tab bar HTML and `showSubtab()` logic

#### Adding a Modal

1. **State**: Add `InputMode` variant with necessary fields
2. **TUI**: Add `render_*_modal()` and `handle_*()` functions
3. **Web UI**: Add modal HTML and show/close functions

#### Adding API Data

1. **State**: Add data type and field to `AppState`
2. **Message**: Add `*Loaded` and `*Error` variants
3. **Spawn**: Add `spawn_*_load()` async task
4. **API**: Add endpoint in `web_ui.rs`
5. **TUI/Web**: Update render/load functions

### 20.8 Responsive Design (Web UI)

The Web UI uses CSS Grid for responsive layouts:

```css
/* Default: 4-column grid */
.grid { 
  display: grid; 
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); 
  gap: 1.5rem; 
}

/* Split layouts */
.split { grid-template-columns: 1fr 1fr; }
.split-35-65 { grid-template-columns: 35% 65%; }

/* Mobile: stack vertically */
@media (max-width: 768px) {
  .sidebar { width: 60px; }
  .nav-item span:not(.icon) { display: none; }
  .split { grid-template-columns: 1fr; }
}
```

### 20.9 Accessibility

| Feature | Implementation |
|---------|----------------|
| **Keyboard Navigation** | Full keyboard support in both TUI and Web |
| **Color Contrast** | WCAG AA compliant text/background ratios |
| **Focus Indicators** | Visible focus rings on interactive elements |
| **Screen Reader** | Semantic HTML in Web UI |
| **Reduced Motion** | Respect `prefers-reduced-motion` |

---

## 21. Implementation Notes

### 21.1 Language Considerations

When implementing in a new language:

1. **WebSocket Library**: Need full duplex, binary frame support
2. **JSON Schema**: Validation library for protocol schemas
3. **Async Runtime**: For concurrent channel connections
4. **Process Management**: For tool execution and sandboxing
5. **File Watching**: For configuration hot reload
6. **HTTP Client**: For model API calls
7. **SQLite**: For session metadata storage

### 21.2 Critical Paths

1. **Message Routing**: Must be fast (<10ms) to not delay responses
2. **Tool Execution**: Sandboxing critical for security
3. **Streaming**: Real-time token delivery for responsiveness
4. **Session Loading**: Cache hot sessions in memory

### 21.3 Testing Requirements

1. **Unit Tests**: Core routing, session management
2. **Integration Tests**: Channel adapters, model providers
3. **E2E Tests**: Full message flow through gateway
4. **Live Tests**: With real API keys and channels

---

## Appendix A: Type Definitions Reference

### Core Types

```typescript
type ChannelId = "telegram" | "discord" | "slack" | "signal" | "imessage" |
                 "whatsapp" | "matrix" | "msteams" | "googlechat" | "irc" | string;

type ChatType = "direct" | "group" | "channel" | "thread";

type SessionScope = "per-sender" | "global" | "per-peer" |
                    "per-channel-peer" | "per-account-channel-peer";

type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high";

type QueueMode = "steer" | "followup" | "collect" | "queue" | "interrupt";

type LogLevel = "silent" | "fatal" | "error" | "warn" | "info" | "debug" | "trace";
```

### Protocol Constants

```typescript
const PROTOCOL_VERSION = 3;
const DEFAULT_PORT = 18789;
const MAX_MESSAGE_SIZE = 1024 * 1024;  // 1MB
const DEFAULT_TIMEOUT = 120000;         // 2 minutes
```

---

## Appendix B: Channel Configuration Examples

### Telegram

```json5
{
  telegram: {
    botToken: "${TELEGRAM_BOT_TOKEN}",
    allowFrom: ["+15555550123", "@username"],
    groupPolicy: "allowlist",
    dmPolicy: "pairing",
    typingMode: "thinking"
  }
}
```

### Discord

```json5
{
  discord: {
    botToken: "${DISCORD_BOT_TOKEN}",
    guildIds: ["123456789"],
    roleAllowlist: ["Admin", "Moderator"],
    prefix: "!",
    requireMention: true
  }
}
```

### Slack

```json5
{
  slack: {
    appToken: "${SLACK_APP_TOKEN}",
    botToken: "${SLACK_BOT_TOKEN}",
    channelAllowlist: ["#general", "#support"],
    socketMode: true
  }
}
```

---

## Appendix C: Cron Examples

```json5
{
  cron: [
    {
      id: "daily-summary",
      name: "Daily Summary",
      schedule: { type: "cron", expression: "0 9 * * *" },
      payload: {
        type: "agentTurn",
        message: "Generate a daily summary of activity"
      },
      delivery: {
        type: "announce",
        channels: ["telegram"]
      }
    },
    {
      id: "heartbeat",
      name: "System Heartbeat",
      schedule: { type: "every", intervalMs: 300000 },
      payload: {
        type: "systemEvent",
        event: "heartbeat"
      }
    }
  ]
}
```

---

**End of Specification**
