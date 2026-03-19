# Agent Topology and Per-Agent Virtual Disk Architecture Proposal

## Purpose

This proposal defines the next storage and orchestration architecture for
RockBot's multi-agent system.

It combines:

- policy-constrained mesh topology
- zone-scoped blackboards
- global control-plane storage
- one virtual disk per agent for agent-local state
- replicated markdown documents stored canonically in `redb`
- on-demand flat-file extraction for inspection, editing, backup, and export

The goal is to make agent creation, delegation, ownership transfer, memory
management, replication, and UI editing coherent under one model instead of
splitting those concerns across ad hoc files, local directories, and
subsystem-specific persistence rules.

## Problem Statement

RockBot is moving toward:

- agents that can create other agents
- bounded execution domains
- richer agent-to-agent collaboration
- storage-backed agent identity and memory
- replicated multi-node operation
- browser and terminal UIs for managing all of the above

The current model is not a good long-term fit because:

- agent relationships are not modeled explicitly as topology
- agent markdown files are treated as filesystem objects instead of canonical
  replicated records
- global and agent-local state are not clearly separated
- per-agent isolation is weak
- ownership, creator provenance, and communication policy are not first-class
- UI editing and replication are fighting a file-based storage layout

## Summary of the Proposed Architecture

RockBot should adopt two complementary storage layers:

1. a global control-plane disk:
   - `rockbot.data`
2. a per-agent virtual disk:
   - one vdisk per agent, managed under the storage runtime

The canonical storage model becomes:

- global `redb` tables for topology, zones, policies, replication metadata, and
  agent registry
- per-agent `redb` tables for markdown documents, agent-local memory, local
  artifacts, and replication status

Markdown remains the content format for human-facing agent documents, but
markdown files are no longer the canonical runtime representation. They become:

- editable content rendered from `redb`
- optionally exported as flat files on demand

## Design Goals

1. Preserve a single authoritative control-plane for the cluster.
2. Isolate agent-local state into separate per-agent virtual disks.
3. Store canonical markdown content inside `redb`, not as loose files.
4. Support replication of agent memory and documents between nodes.
5. Model agent topology explicitly with policies and bounded domains.
6. Support future ownership transfer without losing original creator
   provenance.
7. Provide first-class UI workflows for creating agents, editing markdown,
   viewing memories, and selecting which large objects replicate.
8. Allow operational extraction of one agent's state to flat files on demand.
9. Keep the storage model compatible with `rockbot-storage` and
   `rockbot-storage-runtime`.
10. Avoid introducing a separate graph database unless the actual query/workload
    requires it later.

## Non-Goals

This proposal does not introduce:

- a separate graph database
- raw filesystem markdown as the canonical agent state model
- whole-file vdisk replication
- unrestricted agent-to-agent mesh communication
- automatic replication of every large artifact by default

## High-Level Model

```text
                         +----------------------+
                         |     rockbot.data     |
                         | global control plane |
                         +----------------------+
                         | agent registry       |
                         | topology graph       |
                         | zones                |
                         | blackboard metadata  |
                         | ACL/policy           |
                         | replication metadata |
                         | routing/session meta |
                         +----------+-----------+
                                    |
                   +----------------+----------------+
                   |                                 |
         +---------v---------+             +---------v---------+
         | agent:alpha.data  |             | agent:bravo.data  |
         | per-agent vdisk   |             | per-agent vdisk   |
         +-------------------+             +-------------------+
         | documents         |             | documents         |
         | memory            |             | memory            |
         | artifacts         |             | artifacts         |
         | local indexes     |             | local indexes     |
         | replication state |             | replication state |
         +-------------------+             +-------------------+
```

## Topology Model

### Core Direction

RockBot should adopt:

- a policy-constrained mesh for communication
- zone-scoped blackboards for indirect coordination
- separate ownership and provenance tracking

This avoids the bottlenecks of a strict tree while still supporting bounded
execution domains.

### Node Fields

Each agent node should include:

- `agent_id`
- `creator_agent_id`
- `owner_agent_id`
- `zone_id`
- `role`
- `state`
- `created_at`
- `updated_at`
- `ownership_changed_at`
- `created_via`

`creator_agent_id` is immutable provenance.

`owner_agent_id` is the current steward/controller and may change over time.

### Edge Model

Each edge should include:

- `edge_id`
- `from_agent_id`
- `to_agent_id`
- `edge_kind`
- `policy_id`
- `created_by`
- `observed_count`
- `created_at`
- `updated_at`

Recommended `edge_kind` values:

- `spawn`
- `delegate`
- `tool_use`
- `handoff`
- `observe`
- `blackboard_access`

### Zones

Zones are bounded execution domains.

Each zone should include:

- `zone_id`
- `owner_agent_id`
- `root_agent_id`
- `max_agents`
- `max_depth`
- `max_cross_zone_calls`
- `allowed_models`
- `allowed_tool_classes`
- `allow_cross_zone_delegation`
- `allow_subagent_creation`

### Blackboard Spaces

Blackboards should be scoped by zone and described globally.

Each blackboard should include:

- `board_id`
- `zone_id`
- `name`
- `description`
- `read_policy`
- `write_policy`
- `replication_policy`

## Storage Split

### Global `rockbot.data`

Global storage remains the authoritative control-plane.

It should contain:

- agent registry
- topology nodes
- topology edges
- zone definitions
- blackboard metadata and ACLs
- ownership history
- replication state and generation metadata
- global routing/session/cron/vault state
- per-agent vdisk registry and mount metadata

Recommended logical volumes/tables:

- `agents_registry`
- `topology_nodes`
- `topology_edges`
- `topology_edges_from`
- `topology_edges_to`
- `zones`
- `zone_members`
- `blackboards`
- `blackboard_acl`
- `ownership_events`
- `agent_vdisks`
- `replication_meta`

### Per-Agent Virtual Disks

Each agent should have one dedicated vdisk. Suggested naming:

- `agents/<agent_id>.data`

The runtime should manage these, not the agent directly.

Each per-agent vdisk should contain:

- markdown documents
- agent-local memory
- agent-local artifacts
- local indexes
- per-agent replication metadata
- optional local-only scratch state

## Canonical Agent Documents

Markdown content should be stored in `redb` tables inside each agent vdisk.

Recommended table:

- `agent_documents`

Record fields:

- `document_name`
- `markdown_content`
- `content_hash`
- `version`
- `updated_at`
- `updated_by`
- `replication_class`

Initial canonical documents:

- `SOUL.md`
- `AGENTS.md`
- `SYSTEM-PROMPT.md`
- `MEMORY.md`

The runtime may still project these into flat files when explicitly requested,
but runtime reads/writes should operate on the `redb` records.

## Replication Model

### Principle

Replication should replicate logical records, not raw vdisk bytes.

That means:

- global topology/control-plane replication stays at the global table level
- per-agent replication happens at record-class level inside each agent disk

### Replication Classes

Each record or object should have a replication class:

- `replicated_required`
- `replicated_preferred`
- `local_only`
- `manual_promote`

Recommended defaults:

- agent markdown documents: `replicated_required`
- ownership and topology metadata: global replicated
- large artifacts: `manual_promote`
- caches and temporary indexes: `local_only`

### Large Object Replication

Large binary or semi-binary records should be policy-controlled.

Each artifact/object should store:

- `object_id`
- `agent_id`
- `content_type`
- `size_bytes`
- `hash`
- `replication_policy`
- `promoted_for_replication`
- `last_replicated_at`

This allows the UI to present a per-object replication toggle instead of
replicating everything automatically.

## Why Not a Graph Database

RockBot does not currently need a dedicated graph database.

`redb` does not provide native graph capabilities, but RockBot's required
queries are operational and bounded, for example:

- can agent A delegate to agent B
- which zone does agent X belong to
- which agents are in zone Y
- what blackboards can agent Z write to
- who created this agent
- who currently owns this agent

Those queries are well served by:

- node tables
- edge tables
- adjacency indexes
- zone membership tables

A graph database should only be reconsidered if RockBot later needs:

- large-scale graph analytics
- arbitrary ad hoc graph exploration
- community detection / pathfinding workloads
- cross-node graph-native query execution

## Runtime Overhead Estimates

These estimates are intended for architectural sizing, not exact benchmark
guarantees.

### Per-Agent Vdisk Metadata Overhead

For one mostly idle agent, expected fixed overhead:

- vdisk superblock and metadata: ~32-96 KiB
- `redb` structural overhead: ~128-512 KiB
- minimal document set and indexes: ~16-64 KiB

Expected baseline per-agent disk footprint:

- empty/new agent: ~256 KiB to ~768 KiB
- practical initialized agent: ~512 KiB to ~2 MiB

### Runtime Memory Overhead Per Open Agent

If an agent vdisk is open and active:

- file handles / runtime state: ~16-64 KiB
- `redb` page cache and allocator state: ~128 KiB to ~1 MiB
- topology/policy/runtime caches: ~32-256 KiB

Expected steady-state resident overhead per active agent:

- light agent: ~0.3 MiB to ~1.5 MiB
- medium agent with memory/artifact indexes: ~1 MiB to ~4 MiB

### Global Overhead

The global control-plane disk should remain relatively small compared to model
and artifact data.

Expected global topology/control-plane overhead:

- tens of MiB for thousands of agents and edges
- much smaller than model storage or large artifact storage

### Scaling Guidance

Rough planning guidance:

- 10 active agents: overhead is negligible on modern hardware
- 100 active agents: still practical with lazy-open and page-cache discipline
- 1000 active agents: requires strong lazy-open behavior, bounded caches, and
  likely zone-based scheduling

Key implementation requirement:

- per-agent vdisks must be lazily opened and aggressively closed when idle

## Storage Runtime Changes

`rockbot-storage-runtime` should become responsible for:

- resolving the global control-plane store
- resolving or creating per-agent vdisks
- opening canonical document tables
- applying replication policy defaults
- exporting/importing projected flat files
- maintaining vdisk registry state in the global store

New runtime responsibilities:

- `open_agent_vdisk(agent_id)`
- `create_agent_vdisk(agent_id)`
- `read_agent_document(agent_id, name)`
- `write_agent_document(agent_id, name, markdown)`
- `list_agent_objects(agent_id)`
- `set_object_replication_policy(agent_id, object_id, policy)`
- `extract_agent_vdisk(agent_id, out_dir)`

## Extraction Tooling

RockBot should include an explicit extraction tool.

Recommended commands:

- `rockbot agent extract <agent-id> [--out DIR]`
- `rockbot agent inspect <agent-id>`
- `rockbot agent object list <agent-id>`
- `rockbot agent object replicate <agent-id> <object-id> --enable`
- `rockbot agent object replicate <agent-id> <object-id> --disable`

`rockbot agent extract` should write:

```text
<out>/<agent-id>/
├── SOUL.md
├── AGENTS.md
├── SYSTEM-PROMPT.md
├── MEMORY.md
├── artifacts/
└── manifest.json
```

The extraction output is a projection of canonical records, not the source of
truth.

## UI Requirements

The WebUI and TUI should edit canonical markdown records stored in the agent
vdisk, not loose files.

The UI should support:

- creating agents
- viewing creator/owner/zone metadata
- editing markdown documents
- browsing memories and artifacts
- choosing which large objects replicate
- visualizing topology edges and zone membership

## Wireframes

### Create Agent Flow

```text
+----------------------------------------------------------------------------------+
| Create Agent                                                                     |
+----------------------------------------------------------------------------------+
| Name                [ research-worker-1                                  ]       |
| Model               [ bedrock/us.anthropic.claude-opus-4-6-v1           ]       |
| Zone                [ product-research-zone                             v]       |
| Role                [ worker                                            v]       |
| Owner Agent         [ hex                                               v]       |
| Creator Agent       [ auto: current agent/user                                  ]|
|                                                                                  |
| Parent/Topology                                                                |
| [x] Add spawn edge from owner                                                   |
| [x] Allow delegation from owner                                                  |
| [ ] Expose as callable tool immediately                                          |
|                                                                                  |
| Document Seeds                                                                   |
| SOUL.md            [ default template / custom ]                                 |
| SYSTEM-PROMPT.md   [ default template / custom ]                                 |
| MEMORY.md          [ default template / custom ]                                 |
|                                                                                  |
| Policy                                                                         |
| Max child agents     [ 3 ]   Max tool calls [ 16 ]  Replication profile [Std]   |
|                                                                                  |
|                                             [Cancel] [Create Agent]              |
+----------------------------------------------------------------------------------+
```

### Agent Overview / Management

```text
+----------------------------------------------------------------------------------+
| Agent: research-worker-1                                                         |
+----------------------------------------------------------------------------------+
| Status: Active     Zone: product-research-zone     Owner: hex     Creator: hex   |
| VDisk: healthy     Replication: in sync            Last update: 2m ago           |
+----------------------------------------------------------------------------------+
| Tabs: [Overview] [Documents] [Memory] [Artifacts] [Topology] [Replication]       |
+----------------------------------------------------------------------------------+
| Topology                                                                         |
| Incoming: spawn(hex), delegate(hex)                                              |
| Outgoing: blackboard_access(board:research), delegate(verifier-1)               |
|                                                                                  |
| Zone Policy                                                                      |
| - Can spawn subagents: yes                                                       |
| - Can cross zone: no                                                             |
| - Allowed tools: filesystem, shell, web_fetch, agent_create                     |
+----------------------------------------------------------------------------------+
```

### Markdown Editor

```text
+----------------------------------------------------------------------------------+
| Agent: research-worker-1  >  Documents  >  SYSTEM-PROMPT.md                      |
+----------------------------------------------------------------------------------+
| Document status: replicated_required   Version: 12   Updated by: hex             |
| [Preview] [Split] [Raw Markdown]                            [Save] [Revert]       |
+----------------------------------------------------------------------------------+
| # System Prompt                                                                   |
|                                                                                    |
| You are a research worker operating inside the product-research-zone.             |
| Focus on evidence gathering, source synthesis, and concise reporting.             |
|                                                                                    |
| ## Constraints                                                                     |
| - Do not create agents outside your zone.                                         |
| - Send all completed research notes to board:research.                            |
|                                                                                    |
| ...                                                                                |
+----------------------------------------------------------------------------------+
| Validation                                                                         |
| - Markdown syntax: OK                                                              |
| - Prompt lint: OK                                                                  |
| - Replication class: replicated_required                                           |
+----------------------------------------------------------------------------------+
```

### Memory / Artifact Browser

```text
+----------------------------------------------------------------------------------+
| Agent: research-worker-1  >  Memory                                               |
+----------------------------------------------------------------------------------+
| Filters: [Documents] [Episodes] [Artifacts] [Indexes] [Replicated] [Local Only]  |
+----------------------------------------------------------------------------------+
| Name                        Type         Size      Replication       Updated       |
| MEMORY.md                   document     8 KiB     required          2m ago        |
| episode-2026-03-18-01       episode      42 KiB    preferred         7m ago        |
| notes-market-scan.json      artifact     1.2 MiB   local_only        12m ago       |
| source-pack-01.parquet      artifact     48 MiB    manual_promote    13m ago       |
+----------------------------------------------------------------------------------+
| Selected: source-pack-01.parquet                                                  |
| Hash: sha256:...                                                                  |
| Replication policy: [ manual_promote v ]                                          |
| [Promote For Replication] [Keep Local Only] [Extract]                             |
+----------------------------------------------------------------------------------+
```

### Replication Policy UI

```text
+----------------------------------------------------------------------------------+
| Agent: research-worker-1  >  Replication                                          |
+----------------------------------------------------------------------------------+
| Default profile: Standard                                                         |
|                                                                                  |
| Always replicate                                                                  |
| [x] SOUL.md                                                                       |
| [x] AGENTS.md                                                                     |
| [x] SYSTEM-PROMPT.md                                                              |
| [x] MEMORY.md                                                                     |
| [x] ownership and topology metadata                                               |
|                                                                                  |
| Prefer replicate                                                                  |
| [x] episodic summaries                                                            |
| [ ] full session transcripts                                                      |
|                                                                                  |
| Manual promote only                                                               |
| [x] large artifacts > 8 MiB                                                       |
| [x] index segments                                                                |
|                                                                                  |
| Never replicate                                                                   |
| [x] temporary caches                                                              |
| [x] materialized model scratch                                                    |
|                                                                                  |
|                                             [Save Replication Policy]             |
+----------------------------------------------------------------------------------+
```

### Topology View

```text
+----------------------------------------------------------------------------------+
| Topology: product-research-zone                                                   |
+----------------------------------------------------------------------------------+
| Zone Budget: 6 / 12 agents     Cross-zone calls: 0 / 4                            |
+----------------------------------------------------------------------------------+
| hex ----------------------> research-worker-1 ---------> verifier-1               |
|  | spawn,delegate                 | delegate                  ^ observe            |
|  |                                | blackboard_access         |                    |
|  +----------------------> research-worker-2 -----------------+                    |
|                                                                                  |
| Boards: board:research, board:reviews                                             |
+----------------------------------------------------------------------------------+
```

## UI Editing Semantics

The markdown editor should operate against canonical `redb` records.

Editing flow:

1. UI loads current document content from the agent vdisk.
2. UI edits markdown in memory.
3. Save writes a new versioned record to `agent_documents`.
4. Runtime updates the content hash and replication generation.
5. Optional background export updates projected flat files only if an export
   destination is configured.

This avoids a filesystem-first editing model.

## Migration Plan

### Phase 1

- add global topology and vdisk registry tables
- add per-agent vdisk format
- keep current flat files as compatibility projection

### Phase 2

- migrate canonical `SOUL.md`, `AGENTS.md`, `SYSTEM-PROMPT.md`, `MEMORY.md`
  into `agent_documents`
- route runtime reads/writes through the per-agent disk
- keep extraction/projection support

### Phase 3

- add ownership history, zones, and edge policy tables
- gate agent creation/delegation on topology policy

### Phase 4

- add UI flows for:
  - topology
  - markdown editing
  - artifact replication controls
  - extraction

### Phase 5

- remove remaining filesystem-first agent document assumptions
- make extraction explicitly on-demand instead of implicit

## Risks

- too many eagerly-opened agent vdisks can increase memory and file descriptor
  pressure
- replication policy can become confusing if object classes are not clearly
  described in the UI
- mixing global and per-agent replicated state requires careful generation and
  conflict tracking
- migration must preserve current agent markdown and history without silent data
  loss

## Recommendations

1. Keep topology and control-plane state global.
2. Move agent-local markdown and memory into per-agent vdisks.
3. Use `redb` as the canonical storage for markdown content.
4. Add flat-file extraction as an explicit operator tool.
5. Treat per-object replication policy as first-class UI state.
6. Implement lazy-open and idle-close for agent vdisks before scaling agent
   counts aggressively.

## Proposed Follow-On Work

1. Add a topology architecture plan derived from this proposal.
2. Extend `rockbot-storage-runtime` with per-agent vdisk APIs.
3. Define `agent_documents` and object replication schemas.
4. Add `rockbot agent extract`.
5. Prototype the document editor and replication browser in the WebUI.
