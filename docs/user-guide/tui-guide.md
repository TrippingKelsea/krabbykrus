# Terminal User Interface (TUI) Guide

RockBot includes a full-featured terminal UI for managing your AI agents, credentials, and sessions without leaving the command line.

## Launching the TUI

```bash
rockbot tui
```

## Navigation

### Global Keys

| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Help |
| `Tab` | Toggle sidebar focus |
| `↑` / `↓` | Navigate menu / lists |
| `←` / `→` | Sidebar ↔ Content |
| `Enter` | Select / Confirm |
| `Esc` | Cancel / Back |

### Sidebar Navigation

The sidebar shows six main sections:

| Icon | Section | Description |
|------|---------|-------------|
| 📊 | Dashboard | Overview and status |
| 🔐 | Credentials | Manage secure credential vault |
| 🤖 | Agents | View and control agents |
| 💬 | Sessions | Browse conversation history |
| 🧠 | Models | Configure LLM providers |
| ⚙️ | Settings | Application settings |

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

View configured agents and their status.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Enter` | View agent details |
| `r` | Reload agent list |
| `↑` / `↓` | Select agent |

### Sessions

Browse conversation history.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Enter` | View session messages |
| `d` | Delete session |
| `a` | Archive session |
| `↑` / `↓` | Select session |

### Models

Configure LLM providers.

**Key bindings:**
| Key | Action |
|-----|--------|
| `Enter` | View provider details |
| `e` | Edit provider config |
| `↑` / `↓` | Select provider |

### Settings

Application configuration.

*Settings screen is under development.*

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
