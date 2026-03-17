# Feature Evaluation Tracker

This document tracks product ideas surfaced during live dogfooding of RockBot
from the perspective of the agent running inside the harness.

Statuses:

- `proposed` — captured for evaluation
- `investigating` — active design or feasibility review
- `planned` — approved for implementation
- `deferred` — valid idea, not currently prioritized
- `rejected` — not aligned with product direction

## Suggested Features

| Feature | Status | Notes |
|---------|--------|-------|
| Persistent agent memory across sessions | proposed | Extend beyond episodic memory with a writable identity/preferences layer tied to `SOUL.md` or equivalent durable state. |
| File watch and event-driven triggers | proposed | Wake agents on filesystem or system events instead of cron-only scheduling. |
| Conversation branching / session forking | proposed | Branch a session into parallel approaches and compare outcomes. |
| Tool result caching | proposed | Per-turn or short-horizon caching for idempotent tools like `read`, `glob`, and `grep`. |
| Streaming tool output | investigating | Promoted into active execution-locality hardening so long-running remote tools become observable. |
| Self-diagnostic / introspection tool | proposed | Expose current token budget, turn age, context pressure, and tool-call counts. |
| Screenshot / image capture tool for remote clients | proposed | Purpose-built remote screenshot capture instead of ad hoc shell usage. |
| Ambient context injection | proposed | Inject current time, weather, calendar, system load, or other ephemeral context automatically. |
| Inter-agent pub/sub | proposed | Lightweight publish/subscribe beyond handoffs, swarm blackboards, and DAG workflows. |
| Undo / rollback for file operations | proposed | Track `write` / `edit` / `patch` history and expose a session-scoped undo stack. |

## Critical Issues Surfaced During Evaluation

These are not product features, but they were surfaced during the same review
and should be tracked alongside the wishlist because they affect trust and
operator confidence.

| Issue | Severity | Status | Notes |
|------|----------|--------|-------|
| Execution locality can be misreported in final answers | high | investigating | Answers must clearly reflect whether a fact came from the gateway, the active client, or another executor. |
| Remote tool loops can run for too many iterations | high | investigating | Recent runs showed excessive repeated `exec` calls before convergence. |
| Remote path semantics are unclear to the model | medium | investigating | Repeated failed `read` calls suggest poor grounding on executor-local filesystem state. |
| UTF-8-unsafe truncation in gateway WS forwarding | high | investigating | Recently caused a panic while forwarding large tool output. |
| `rockbot-core` remains a thin re-export facade | medium | proposed | Evaluate whether the compatibility layer still earns its maintenance cost. |
| Sessions split across SQLite while agents/credentials use `redb` | medium | proposed | Consider whether storage consolidation is worth the migration cost. |
| `rockbot-overseer` string truncation bug risk | high | proposed | The model flagged another likely UTF-8 slicing issue that should be audited. |
| File-based memory plus vector store split | low | proposed | Acceptable for now, but warrants an architecture review for durability and simplicity. |
| Plugin system still scaffold-level | low | proposed | Consider whether WASM/plugin ABI work is worth prioritizing. |

## Notes From The Session

The most useful product ideas from this session were the ones that improve
agent self-awareness and operator control:

- stronger execution provenance
- better long-running tool UX
- better session management primitives
- better local/remote environment introspection

Those features should be evaluated through the lens of security and locality,
not just convenience.
