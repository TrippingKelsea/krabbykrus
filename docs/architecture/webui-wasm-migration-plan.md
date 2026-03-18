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

## Accessibility and ADA Usability

The browser app should target WCAG 2.2 AA as the operational baseline.
That is the practical bar for a modern accessible product and the right
proxy for ADA-aligned usability work.

### Accessibility Requirements

- full keyboard operation for every user-visible workflow
- visible focus indicators at all times
- semantic landmarks, headings, buttons, forms, tables, and dialogs
- screen-reader friendly labeling for all controls and status changes
- sufficient color contrast in all themes
- reduced-motion support for streaming and animated transitions
- no color-only state signaling
- mobile touch targets sized for accessibility, not just compactness
- error messaging that is specific, persistent, and associated with fields

### Accessibility-Specific Product Requirements

- chat streaming must announce incremental updates without overwhelming
  screen readers
- slash-command and command-palette interactions must expose listbox /
  option semantics and keyboard navigation
- model/provider pickers must be searchable and fully operable without
  pointer input
- imported identity and certificate-management flows must work with
  keyboard, screen readers, and touch
- mobile layouts must preserve a stable reading and focus order when panes
  collapse into sheets or stacked navigation

### Accessibility Engineering Requirements

- automated axe/lighthouse accessibility checks in CI
- integration tests for keyboard-only navigation on major flows
- a design-token layer for contrast-safe colors, spacing, focus rings, and
  minimum hit targets
- explicit accessibility review on:
  - chat transcript rendering
  - modal/sheet focus trapping
  - streaming tool output
  - model/provider overlays
  - certificate import and error flows

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

### Frontend Stack Options

The WebUI should be selected from a short list of realistic Rust-first
contenders rather than defaulting to the first WASM framework that compiles.

#### Option A: `leptos`

Strengths:

- strong Rust/WASM maturity with client-side rendering and optional SSR
- fine-grained reactivity keeps update costs low for chat-heavy views
- good fit for stateful app shells and long-lived UI state
- strong ecosystem momentum and documentation quality

Weaknesses:

- still requires deliberate component and styling discipline
- browser integration still drops to `web-sys` / `wasm-bindgen` for some APIs
- less natural fit if we want to preserve ratatui-style rendering semantics

Assessment:

- best default contender for the WebUI
- strongest balance of maturity, ergonomics, and long-term maintainability

#### Option B: `dioxus`

Strengths:

- broad platform story: web, desktop, and mobile
- good developer ergonomics and hot-reload workflow
- attractive if we want one UI stack spanning browser and possible future
  desktop shells

Weaknesses:

- more opinionated runtime model
- less aligned with TUI-style reusable rendering than a ratatui-based approach
- cross-platform story is attractive, but RockBot's immediate problem is a
  high-quality browser UI, not a second desktop shell

Assessment:

- credible second-place option
- strongest choice if multi-target GUI beyond the browser becomes a near-term
  goal

#### Option C: `yew`

Strengths:

- established and well-known Rust web framework
- component model familiar to teams with React-style mental models
- mature enough for conventional SPA work

Weaknesses:

- virtual-DOM model is less compelling than fine-grained reactivity for a
  streaming chat/control-plane app
- less attractive than Leptos for minimizing redraws and reactive drift

Assessment:

- viable, but no longer the leading fit for this project

#### Option D: `sycamore`

Strengths:

- fine-grained reactive approach similar in spirit to Leptos
- simple mental model and good Rust-first ergonomics

Weaknesses:

- smaller ecosystem and lower project momentum than Leptos
- fewer signs of broad adoption for a large app shell with many integration
  points

Assessment:

- technically viable, but behind Leptos on ecosystem confidence

#### Option E: `ratzilla`

Strengths:

- uniquely aligned with RockBot's existing ratatui/TUI investment
- makes TUI-inspired visual and layout reuse much more realistic
- could reduce design drift between terminal and browser surfaces

Weaknesses:

- terminal-themed rendering model is not the same thing as a robust accessible
  web application model
- accessibility semantics, responsive DOM structure, and standard web control
  behavior require extra scrutiny
- maturity is lower than the leading Rust web UI frameworks
- browser UX risks becoming a transplanted terminal rather than a good web app

Assessment:

- strong contender for selective reuse experiments
- weak choice as the primary WebUI foundation unless accessibility,
  responsiveness, and standard browser interaction semantics are proven first

### Recommended Stack Decision

Recommended baseline:

- `leptos` for the production WebUI shell and application surface
- `wasm-bindgen` / `web-sys` for browser APIs and crypto/storage integration
- a thin shared protocol crate for WS event/request types

Recommended exploration track:

- evaluate `ratzilla` as a focused reuse path for terminal-like sub-surfaces,
  prototypes, or embedded views
- do not make `ratzilla` the primary WebUI stack unless it proves:
  - WCAG/ADA-friendly semantics
  - responsive layout quality on mobile and desktop
  - clean integration with the app's browser auth and state model

### Why `leptos` First

- keeps more domain logic in Rust
- reduces drift with the native client protocol
- allows sharing typed DTOs with gateway/client crates
- better fit than a terminal-rendering abstraction for accessible,
  responsive, browser-native product behavior

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

## Shared TUI and WebUI Abstraction Strategy

We should push reuse down a layer, but not pretend the TUI renderer and the
browser renderer are the same problem.

### What Should Be Shared

- protocol DTOs and WS request/response types
- domain models:
  - sessions
  - agents
  - providers
  - cron jobs
  - vault metadata
- state machines for:
  - reconnect
  - auth/bootstrap
  - streaming message assembly
  - slash-command parsing and completion sources
- design tokens:
  - semantic colors
  - spacing scale
  - typography intent
  - status/severity states
- command metadata and keyboard-action vocabularies

### What Should Not Be Shared Directly

- ratatui widget implementations
- browser DOM/component implementations
- focus-management primitives
- layout containers
- accessibility semantics and ARIA wiring

### Recommended Reuse Shape

Create shared UI-adjacent crates/modules for:

- `rockbot-ui-model`
  - view models
  - derived presentation state
  - formatting helpers
- `rockbot-ui-protocol`
  - WS DTOs
  - command metadata
  - event mapping
- `rockbot-ui-theme`
  - semantic tokens and stateful style meanings

Then keep two renderer layers:

- TUI renderer on ratatui
- WebUI renderer on Leptos

This gives real reuse where it is valuable without forcing terminal-shaped
components into a browser environment.

## Ratzilla Evaluation

`ratzilla` is interesting because RockBot already has a meaningful ratatui
surface area. It is the clearest path if the goal is to preserve terminal
visual structure in the browser.

### Where Ratzilla Fits Well

- internal operator dashboards
- prototype parity experiments between TUI and browser
- browser-hosted terminal-themed views
- temporary migration bridge for specific screens

### Where Ratzilla Fits Poorly

- highly accessible forms and dialogs
- mobile-first responsive navigation
- polished browser-native interaction patterns
- public-facing UI that should feel like a web app instead of a terminal

### Recommendation on Ratzilla

- keep it as a contender, not the default
- run one bounded spike:
  - chat transcript
  - agent/session switcher
  - command palette
- evaluate it specifically on:
  - keyboard navigation
  - screen-reader semantics
  - mobile responsiveness
  - bundle/runtime cost
  - development velocity
- if it underperforms on accessibility or layout flexibility, keep it for
  experiments only and proceed with Leptos

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
- accessibility regressions if terminal-oriented rendering is forced into the
  browser without browser-native semantics
- over-sharing UI abstractions at the renderer layer, creating two mediocre
  frontends instead of one good TUI and one good WebUI

## Recommended Sequence

1. Shared protocol types and shared UI-model layer
2. Framework spike: Leptos baseline + bounded Ratzilla comparison
3. WASM shell + auth import
4. responsive read-only views
5. chat/session flows
6. management/editor flows
7. credentials polish, accessibility audit, and mobile refinement

## Success Criteria

- Web UI no longer depends on public CRUD APIs
- desktop and mobile both work with the same WASM app
- imported identity persists across browser restarts
- chat/session/provider management works over authenticated WS only
- major workflows satisfy WCAG 2.2 AA expectations
- shared state/protocol abstractions reduce TUI/Web duplication without forcing
  shared renderer components
