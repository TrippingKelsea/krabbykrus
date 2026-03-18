# Tooling Gap Analysis

## Scope

Compare RockBot’s current operator/developer experience against adjacent tools in this space:

- Claude Code
- OpenAI Codex CLI
- Aider
- Goose
- OpenHands

This is a product and workflow gap analysis, not a model-quality benchmark.

## RockBot Strengths

- integrated gateway + remote-exec architecture
- PKI/mTLS identity model
- encrypted local storage direction with `rockbot.data`
- TUI-first operations experience
- explicit tool locality and client execution routing
- multi-provider LLM architecture

## Current Gaps

### 1. Shell/CLI ergonomics

Compared with Codex CLI and Aider:

- shell completion only just landed and still needs docs/install polish
- fewer “obvious default” workflows for common edit/test/review loops
- less discoverable command surface

### 2. Repository navigation and code intelligence

Compared with Claude Code, Codex CLI, OpenHands:

- weaker first-class code exploration affordances
- fewer built-in repo summarization / symbol navigation workflows
- less polished guided review/fix loops

### 3. Browser and web context

Compared with Goose/OpenHands:

- no real browser plugin yet
- Web UI is still in bootstrap form, not a full responsive app
- limited page-aware workflows

### 4. Tooling modularity

Compared with mature plugin systems:

- tool split is improving, but provider/tool packaging is still uneven
- some built-ins still feel framework-internal rather than operator-shaped

### 5. Onboarding and deployment polish

Compared with lighter-weight CLIs:

- secure deployment story is strong, but more complex
- first-run workflows still have more concepts than Aider/Codex CLI
- operator docs need more “copy/paste path” style coverage

## Category-by-Category

### Against Claude Code

RockBot is stronger at:

- gateway/client topology
- explicit remote execution
- PKI-based cluster identity

RockBot is weaker at:

- smooth local coding workflow polish
- slash/help discoverability
- user-facing browser workflow

### Against Codex CLI

RockBot is stronger at:

- multi-node architecture
- agent/gateway separation
- credential/vault direction

RockBot is weaker at:

- single-user CLI simplicity
- shell ergonomics
- fast “just code in this repo” loop

### Against Aider

RockBot is stronger at:

- multi-provider architecture
- remote execution and cluster-aware design

RockBot is weaker at:

- editing workflow clarity
- Git-centric coding loop polish
- minimal-setup usability

### Against Goose / OpenHands

RockBot is stronger at:

- trust model and local deployment control
- explicit storage/security architecture

RockBot is weaker at:

- browser-native interaction
- polished web UX
- autonomous browser/context workflows

## Highest-Value Near-Term Improvements

1. Finish the responsive WASM Web UI.
2. Build the browser plugin for context capture.
3. Continue tightening CLI/TUI ergonomics:
   - completion docs
   - clearer command discovery
   - smoother common edit/test/review flows
4. Keep modularizing providers/tools so packaging maps to real operator concerns.
5. Add stronger repo/code-intelligence affordances in agent workflows.

## Strategic Recommendation

RockBot should not try to out-copy simpler single-user coding CLIs feature-for-feature.

Its differentiator is:

- secure multi-node orchestration
- explicit execution locality
- local-first operator control

The right strategy is:

- close the workflow polish gap
- close the browser/Web UI gap
- keep leaning into security, deployment, and remote execution as the core identity
