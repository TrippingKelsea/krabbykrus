# Browser Plugin Architecture Plan

## Goal

Define a RockBot browser extension that can:

- connect a browser session to a RockBot gateway
- surface agent actions in-page
- capture page context for agents and tools
- optionally hand off trusted browser actions to the main RockBot app

## Why a Plugin

The Web UI gives users a full browser app, but an extension enables:

- page-aware context capture
- per-tab agent assistance
- controlled DOM extraction
- browser-native workflow triggers

## Target Capabilities

- connect extension to RockBot cluster using imported identity
- send current page URL/title/selection/DOM snippets to RockBot
- expose “send page to agent” and “ask agent about this page”
- opt-in site permissions
- future automation hooks for safe browser actions

## Security Model

The extension must not create a weaker trust path than the main app.

Requirements:

- imported identity per extension profile
- stored key material handled like the browser Web UI, not raw plaintext
- explicit host permission prompts
- no unrestricted arbitrary page scraping
- no silent execution of privileged browser actions

## Architecture

### Extension Pieces

- background/service worker
  - WS session management
  - auth bootstrap
  - message routing

- content scripts
  - page context capture
  - selection extraction
  - DOM fragment serialization

- popup UI
  - quick actions
  - connection status
  - active agent/session chooser

- options page
  - gateway target
  - key import/remove
  - permission management

## Communication Model

- background worker owns the authenticated WS connection
- popup and content scripts message the background worker
- background worker forwards requests into RockBot over WS RPC

## Initial Feature Scope

### Phase 1

- options page
- key import/persistence
- gateway connect/disconnect
- popup status UI

### Phase 2

- send current URL/title/selection to agent
- create session from current tab
- show response snippets in popup

### Phase 3

- controlled DOM extraction tool
- per-site permission gating
- page annotations/highlights for results

### Phase 4

- browser action approval model
- safe form-fill/navigation primitives
- optional handoff to full Web UI

## Relationship to Web UI

The plugin should not duplicate the full Web UI.

Recommended model:

- extension for in-page context and quick actions
- full Web UI for deep session/agent management
- both use the same WS auth/control plane

## Key Technical Decisions

### Identity Storage

Start with:

- IndexedDB-backed key storage in extension context
- same logical identity import flow as Web UI

Later:

- passphrase wrapping
- profile export/import

### Page Context Format

Represent captured context as:

- URL
- title
- selected text
- visible text excerpt
- optional structured DOM metadata

Avoid raw full-page dumps by default.

### Browser Action Safety

Every action beyond read-only capture should be:

- explicit
- site-scoped
- auditable
- optionally approval-gated

## Risks

- extension storage security expectations
- browser permission sprawl
- page-specific DOM brittleness
- feature duplication with the Web UI if scope is not kept narrow

## Recommended Sequence

1. auth/connect foundation
2. quick “send page to agent” path
3. popup/session UX
4. safe DOM extraction
5. action approval model

## Success Criteria

- extension can authenticate to RockBot without a separate backend auth model
- page context can be sent to an agent in a single interaction
- host permissions are explicit and narrow
- privileged browser actions remain opt-in and auditable
