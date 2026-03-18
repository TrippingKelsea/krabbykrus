# Responsive WASM WebUI Migration Plan

## Goal

Replace the current bootstrap-only browser shell with a responsive WebAssembly
application that speaks the same authenticated WebSocket control plane as the
native TUI.

The target outcome is:

- one stateful app protocol over authenticated WS
- a browser UI that is layout-responsive on desktop and mobile
- local browser key import and persistence
- no sensitive public HTTP CRUD surface

## Non-Goals

- server-side rendered application pages
- a second browser-only API surface
- replacing native TUI workflows
- browser-native mTLS without the existing key-import bootstrap flow

## Constraints

- public HTTPS remains bootstrap-only
- authenticated app traffic continues to move over WS
- browser auth must work with imported client identity material
- the browser app must remain usable without introducing platform-specific OS installers

## Current State

The existing Web UI is a thin shell served from `/` and `/static/*`, with:

- IndexedDB-backed key persistence
- browser-side certificate/key auth bootstrap
- authenticated WS access after challenge/response

What is missing is a real application frontend:

- no responsive page architecture
- no componentized app shell
- no state model comparable to the TUI
- no browser-native feature parity for sessions, agents, providers, cron, or credentials

## Target Architecture

### Delivery Model

- gateway serves:
  - `/`
  - `/static/app.js`
  - `/static/app.css`
  - `/health`
  - optional `/api/cert/ca`
  - optional `/api/cert/sign`
- app bundle is a WASM + JS bootstrap package
- app uses the existing public WS bootstrap only for browser auth
- all authenticated app traffic uses the WS control plane

### Frontend Stack

Recommended:

- Rust + `leptos` or `dioxus` for WASM UI
- `wasm-bindgen` / `web-sys` for browser APIs
- a thin shared protocol crate for WS event/request types

Why:

- keeps more domain logic in Rust
- reduces drift with the native client protocol
- allows sharing typed DTOs with gateway/client crates

## Application Layers

### 1. Shell Layer

- route handling
- responsive layout
- auth/import screens
- reconnect/offline banners

### 2. Session Layer

- WS lifecycle
- auth state
- request/response correlation
- optimistic local state where safe

### 3. Domain State

- agents
- sessions
- providers/models
- credentials/vault metadata
- cron/jobs
- settings

### 4. Presentation

- desktop three-pane layout
- mobile stacked navigation
- overlays/modals
- command palette

## Responsive Design Requirements

### Desktop

- persistent navigation rail
- session/agent side panels
- main chat/content pane
- utility drawer for providers, cron, credentials

### Mobile

- bottom navigation or compact drawer
- single-primary-pane layout
- sheets instead of wide sidebars
- message composer fixed to viewport bottom

### Shared

- keyboard friendly on desktop
- touch friendly on mobile
- no fixed-width assumptions
- incremental rendering for streaming/tool output

## Auth and Key Handling

### Browser Identity

- import PEM cert/key through UI
- store non-extractable `CryptoKey` material in IndexedDB
- retain certificate PEM for display and trust metadata
- support explicit “forget identity”

### Follow-up Hardening

- optional passphrase wrapping for browser-stored key material
- key rotation and re-import flows
- multiple saved identities per cluster

## Feature Migration Phases

### Phase 1: Shared Protocol and App Shell

- introduce shared browser-friendly WS DTO crate or module
- replace bootstrap JS with WASM shell
- implement auth/import/reconnect flow
- add responsive top-level layout

### Phase 2: Read-Only Surfaces

- provider/model inventory
- agents list/detail
- sessions list/detail
- health/gateway status

### Phase 3: Interactive Chat

- create session
- send message
- stream chunks
- render tool output and thinking status
- retry / archive / switch session

### Phase 4: Management Surfaces

- agent create/edit
- provider auth config
- cron management
- context files

### Phase 5: Credentials/Vault UX

- endpoint listing
- permission review
- provider credential state
- grant/approval flow where applicable

### Phase 6: Browser-Only Polish

- command palette
- slash command completion
- cached UI state
- responsive mobile refinements

## Required Backend Work

- keep WS RPC stable and typed
- finish any remaining public-to-WS control plane migrations
- expose browser-safe auth bootstrap messages only on public WS
- avoid browser-only special cases in gateway business logic

## Testing Strategy

- component tests for auth/session state
- browser integration tests for import/auth/reconnect
- responsive snapshot tests for key breakpoints
- protocol compatibility tests against gateway WS messages

## Risks

- browser key persistence complexity
- WASM bundle size and startup latency
- drift from native TUI if DTOs are not shared
- mobile UX complexity for chat + tool output

## Recommended Sequence

1. Shared protocol types
2. WASM shell + auth import
3. responsive read-only views
4. chat/session flows
5. management/editor flows
6. credentials polish and mobile refinement

## Success Criteria

- Web UI no longer depends on public CRUD APIs
- desktop and mobile both work with the same WASM app
- imported identity persists across browser restarts
- chat/session/provider management works over authenticated WS only
