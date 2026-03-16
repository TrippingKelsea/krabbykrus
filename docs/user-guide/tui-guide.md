# Terminal User Interface (TUI) Guide

RockBot includes a full-featured terminal UI for managing your AI agents, credentials, and sessions without leaving the command line.

## Launching the TUI

```bash
rockbot tui
```

## Navigation

### Layout

The TUI uses a unified card bar layout:

```
Row 0: SlottedCardBar (top)  — mode selector + per-mode info cards
Row 1: Status strip          — contextual info line
Row 2: Main content area     — full-height page content
Row 3: Status bar            — help text / errors
```

All card navigation lives in the top card bar. There are no inner card strips — each page gets the full content area.

### Global Keys

| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Context menu |
| `Alt+←` / `Alt+→` | Navigate cards in top bar |
| `Alt+↑` / `Alt+↓` | Switch section (mode) |
| `Alt+Enter` | Open card detail overlay |
| `↑` / `↓` | Navigate lists |
| `←` / `→` | Navigate per-mode items |
| `1-7` | Jump to section |
| `Enter` | Select / Confirm |
| `Esc` | Cancel / Back |

### Sections

The card bar mode selector provides seven sections:

| # | Section | Description |
|---|---------|-------------|
| 1 | Dashboard | Butler chat + status overview |
| 2 | Credentials | Manage secure credential vault |
| 3 | Agents | View and control agents |
| 4 | Sessions | Chat sessions (grouped by agent) |
| 5 | Cron Jobs | Scheduled task management |
| 6 | Models | Configure LLM providers |
| 7 | Settings | Application settings |

## Screens

### Dashboard

Shows system status at a glance:
- Gateway connection status
- Active sessions count
- Vault status (locked/unlocked)
- Recent activity

### Credentials

Manage your secure credential vault.

**Key bindings:**
| Key | Action |
|-----|--------|
| `a` | Add new credential |
| `d` | Delete selected credential |
| `u` | Unlock vault |
| `l` | Lock vault |
| `↑` / `↓` | Select credential |

**Add Credential Modal:**

When adding a credential, the form fields change based on the selected endpoint type:

| Type | Fields |
|------|--------|
| Home Assistant | URL, Long-Lived Access Token |
| Generic REST API | Base URL, Bearer Token |
| OAuth2 Service | Base URL, Auth URL, Token URL, Client ID, Client Secret, Scopes, Redirect URI |
| API Key Service | Base URL, API Key, Header Name |
| Basic Auth | Base URL, Username, Password |
| Bearer Token | Base URL, Token |

**Modal navigation:**
| Key | Action |
|-----|--------|
| `Tab` / `↑` / `↓` | Move between fields |
| `←` / `→` | Change endpoint type (when on type selector) |
| `Enter` | Next field / Submit |
| `Esc` | Cancel |

### Agents

View configured agents and their status. Agents are shown as cards in the top bar; the full content area shows the selected agent's details.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Alt+←/→` | Select agent (card bar) |
| `Enter` | View agent details |
| `a` | Add new agent |
| `e` | Edit selected agent |
| `d` | Disable agent |
| `f` | Browse context files |
| `r` | Reload agent list |

### Sessions

Chat sessions grouped by agent in the card bar. The content area shows the chat interface for the selected session.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Alt+←/→` | Select session (card bar) |
| `n` | Create new session |
| `c` | Enter chat mode |
| `k` | Kill session |
| `Alt+Enter` | Session detail overlay |

### Models

LLM provider configuration. Providers shown in the card bar; details in the content area.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Alt+←/→` | Select provider |
| `Enter` | View model list |
| `e` | Configure provider |
| `Alt+Enter` | Provider detail overlay |

### Settings

Application configuration with sub-sections: General, Paths, About.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Alt+←/→` | Select section |
| `s` | Start gateway |
| `S` | Stop gateway |
| `r` | Restart gateway |

## Vault Unlock Flow

When accessing credentials with a locked vault:

1. **Keyfile-based vaults** auto-unlock without prompting
2. **Password-based vaults** show an unlock modal:
   - Enter your master password
   - Press `Enter` to submit
   - Press `Esc` to cancel

The vault status is shown in the sidebar:
- 🔓 Unlocked (green)
- 🔒 Locked (yellow)
- ❌ Not initialized (red)

## Tips

### Quick Navigation

- Press number keys `1-6` to jump directly to sections
- `g` then `d` for Dashboard, `g` then `c` for Credentials, etc.

### Responsive Design

The TUI adapts to your terminal size. For best experience:
- Minimum: 80x24
- Recommended: 120x40+

### Color Themes

The TUI respects your terminal's color scheme. For best contrast, use a dark terminal theme.

## Troubleshooting

### TUI Won't Start

**"No such device or address"** - The TUI requires an interactive terminal. Don't pipe input or run in a non-TTY environment.

**"Terminal too small"** - Resize your terminal to at least 80x24.

### Input Not Working

Ensure your terminal emulator supports:
- Unicode (for icons)
- 256 colors (for styling)
- Mouse input (optional, for clicking)

### Fields Not Rendering

If form fields appear blank, this was likely a layout bug (now fixed). Update to the latest version:

```bash
git pull
cargo build --release
```
