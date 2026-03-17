# Terminal User Interface (TUI) Guide

RockBot includes a full-featured terminal UI for managing your AI agents, credentials, and sessions without leaving the command line.

## Launching the TUI

```bash
rockbot tui
```

## Chat-First Architecture

The TUI is **chat-first**: the chat interface is always visible in the main content area. Other views (vault, settings, models, cron) are overlays that appear on top of the chat when triggered.

### Layout

```
Row 0: SlottedCardBar (top)  -- chat target selector (Dashboard/Agents/Sessions/Cron)
Row 1: Status strip          -- global: gateway / agents / sessions / vault status
Row 2: Chat area (fill)      -- ALWAYS renders chat (butler, session, or agent)
Row 3: Status bar            -- error / success messages (blank otherwise)
```

Switching between modes changes **what you're chatting with**, not the content area.

### Global Keys

| Key | Action |
|-----|--------|
| `q` | Quit |
| `c` | Enter chat mode |
| `?` | Context menu |
| `Alt+Left/Right` | Navigate cards in top bar |
| `Alt+Up/Down` | Switch mode (Dashboard/Agents/Sessions/Cron) |
| `Alt+Enter` | Open card detail overlay |
| `1-4` | Jump to mode |
| `5-7` | Open overlay (Cron / Vault / Models) |
| `Enter` | Select / Confirm |
| `Esc` | Cancel / Close overlay |

### Overlay Shortcuts

| Key | Overlay |
|-----|---------|
| `Alt+V` | Vault / Credentials |
| `Alt+S` | Settings |
| `Alt+M` | Models / LLM Providers |
| `Alt+C` | Cron Jobs |

### Modes

The card bar mode selector provides four navigation targets:

| # | Mode | Description |
|---|------|-------------|
| 1 | Dashboard | Butler chat + status overview cards, including WS, Noise, and execution target |
| 2 | Agents | One card per agent; selecting changes chat target |
| 3 | Sessions | Sessions grouped by agent; selecting changes active chat |
| 4 | Cron Jobs | Cron overview card |

## Chat

Chat is always visible. The chat target depends on the current mode:

- **Dashboard**: Chat with Butler (local companion agent)
- **Agents**: Chat with the selected agent (ad-hoc)
- **Sessions**: Chat within the selected session

Press `c` to focus the chat input. In chat mode:

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | Insert newline (Ctrl+J fallback) |
| `PgUp/PgDn` | Scroll history |
| `Ctrl+R` | Retry last message |
| `Esc` | Exit chat mode |

## Overlays

### Vault (Alt+V)

Manage your secure credential vault. Has four tabs:

| Tab | Description |
|-----|-------------|
| Endpoints | Configured credential endpoints |
| Providers | Available credential providers (from gateway) |
| Permissions | Credential access rules |
| Audit | Audit log |

| Key | Action |
|-----|--------|
| `Tab` / `1-4` | Switch tab |
| `a` | Add credential |
| `d` | Delete selected |
| `i` | Initialize vault |
| `u` | Unlock vault |
| `l` | Lock vault |
| `Enter` | View details |
| `Esc` | Close overlay |

### Settings (Alt+S)

Application configuration with five tab sections: General, Paths, About, Theme, Fonts.

| Key | Action |
|-----|--------|
| `Left/Right` or `Tab` | Switch section |
| `s` | Start gateway |
| `S` | Stop gateway |
| `r` | Restart gateway |
| `Esc` | Close overlay |

**Theme section** — currently supports preset theme + animation controls, with
custom token editing and persisted font preferences available in config:

| Key | Action |
|-----|--------|
| `Up/Down` | Select field |
| `]` | Next value / increment |
| `[` | Previous value / decrement |
| Mouse | Select presets, tokens, drag the color wheel/sliders |

**Typography section** — stores font preferences for the Web UI and other richer clients:

| Key | Action |
|-----|--------|
| `Up/Down` | Select role / family / size field |
| `]` | Next role/family or larger size |
| `[` | Previous role/family or smaller size |
| Mouse | Select roles, font families, and size buttons |

### Models (Alt+M)

LLM provider configuration. Dynamic tab bar built from actual gateway providers (not hardcoded).

| Key | Action |
|-----|--------|
| `Left/Right` | Select provider |
| `Enter` | View model list |
| `e` | Configure provider |
| `Esc` | Close overlay |

### Cron Jobs (Alt+C)

Scheduled task management with inline filter toggle.

| Key | Action |
|-----|--------|
| `Tab` | Cycle filter (All/Active/Disabled) |
| `Up/Down` | Select job |
| `e` | Enable/disable |
| `d` | Delete |
| `t` | Trigger now |
| `r` | Refresh |
| `Esc` | Close overlay |

## Agents Mode

Agents are shown as cards in the top bar. Selecting an agent changes the chat target.

| Key | Action |
|-----|--------|
| `Alt+Left/Right` | Select agent (card bar) |
| `c` | Chat with selected agent |
| `a` | Add new agent |
| `e` | Edit selected agent |
| `d` | Disable agent |
| `f` | Browse context files |
| `Alt+Enter` | Agent detail overlay |

## Sessions Mode

Chat sessions grouped by agent in the card bar. Selecting changes the active chat.

| Key | Action |
|-----|--------|
| `Alt+Left/Right` | Select session |
| `c` | Enter chat mode |
| `n` | Create new session |
| `k` | Kill session |
| `Alt+Enter` | Session detail overlay |

## Vault Unlock Flow

When accessing credentials with a locked vault:

1. **Keyfile-based vaults** auto-unlock without prompting
2. **Password-based vaults** show an unlock modal:
   - Enter your master password
   - Press `Enter` to submit
   - Press `Esc` to cancel

## Color Themes

Color themes, animation styles, token-level custom colors, and stored font
preferences can be changed live in the **Settings overlay** (Alt+S). Changes
autosave to `[tui]`, `[tui.theme]`, and `[tui.fonts]` in `rockbot.toml`.

You can also configure defaults in `rockbot.toml`:

```toml
[tui]
color_theme = "Purple"        # Purple, Blue, Green, Rose, Amber, Mono
animation_style = "Coalesce"  # Coalesce, Fade, Slide, None

[tui.theme]
ai_text_color = { r = 235, g = 222, b = 255, a = 255 }
thinking_text_color = { r = 191, g = 169, b = 239, a = 255 }
tool_text_color = { r = 255, g = 214, b = 153, a = 255 }

[tui.fonts]
ai_font_family = "terminal-default"
ai_font_size = 14
```

The terminal TUI uses the text-color tokens immediately, including
`ai_text_color`, `thinking_text_color`, and `tool_text_color`. Font families and
sizes are persisted but not enforced by the terminal emulator.

## Remote Execution Controls

The Dashboard exposes two remote-exec cards:

- **Noise**: shows WS connectivity, Noise registration state, and how many
  executors the gateway currently knows about.
- **Exec**: shows where tool calls will run by default.

Open the Exec card detail overlay with `Alt+Enter` while that card is selected.
Inside the overlay:

- `t` toggles local active-client execution on or off
- `g` forces gateway-local execution
- `Up` / `Down` select a connected remote executor
- `Enter` assigns the highlighted executor as the remote tool target

This execution target only affects remote-capable tool calls such as shell and
filesystem work. When local execution is enabled, those tools use the active
client's current working directory instead of the gateway host's cwd.

## Tips

### Quick Navigation

- Press `1-4` to jump directly to modes
- Press `5-7` to open overlays (Cron/Vault/Models)
- Use `Alt+V/S/M/C` for overlay shortcuts from any mode

### Responsive Design

The TUI adapts to your terminal size. For best experience:
- Minimum: 80x24
- Recommended: 120x40+

## Troubleshooting

### TUI Won't Start

**"No such device or address"** - The TUI requires an interactive terminal. Don't pipe input or run in a non-TTY environment.

**"Terminal too small"** - Resize your terminal to at least 80x24.

### Input Not Working

Ensure your terminal emulator supports:
- Unicode (for icons)
- 256 colors (for styling)
- Mouse input (optional, for clicking)
