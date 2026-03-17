# Execution Locality Hardening Proposal

## Purpose

This proposal captures the current execution-locality issues observed while
testing remote tool execution and outlines the hardening work needed to make
RockBot security-first and locality-correct.

The core requirement is simple:

- If a tool is routed to the active client, the answer must reflect the active
  client.
- If a tool is routed to another client, the answer must reflect that client.
- If a tool is routed to the gateway, the answer must reflect the gateway.
- The model must not be allowed to blur those boundaries.

## Observed Problems

### 1. Environment claims can still be misattributed

Even when remote execution is working, the final answer can still report facts
as though they came from the gateway or from model inference rather than from
the selected executor.

Observed failure mode:

- remote `exec` calls clearly ran on the client
- the final answer still did not reliably ground itself in that locality

### 2. Remote tool output is too opaque during long runs

Recent logs showed the agent reaching 10 iterations and 27 tool calls while
continuing to query the same remote executor. At this stage, the more urgent
problem is poor operator visibility: long-running remote tools do not stream
their output, which makes it hard to tell whether the model is progressing,
exploring legitimately, or thrashing.

### 3. Remote path semantics are still weak

The agent issued repeated remote `read` calls that failed immediately. That
means the dispatch path is working, but the model still lacks enough grounding
about which filesystem view it is operating in and which paths are valid on the
remote executor.

### 4. Gateway WebSocket forwarding panics on UTF-8 boundaries

The gateway currently truncates large tool results by slicing strings on byte
offsets. That is unsafe for UTF-8 output and caused a panic during a recent
remote-exec run.

### 5. Final answers do not expose execution provenance

A user currently has to infer locality from logs and card state. The final
agent response does not clearly say whether its claims were executed on the
gateway, the active client, or another executor.

## Proposal

### A. Treat execution locality as first-class state

The system should carry execution-locality metadata all the way through:

- requested target
- resolved target
- execution workdir
- tool-result provenance

That metadata should be available to:

- the agent prompt
- the tool loop
- the TUI
- the final response formatter

### B. Require tool-backed verification for environment-sensitive claims

Claims about live machine state must be backed by tool output from the current
execution target. This includes:

- hostname
- current user
- current working directory
- OS / kernel details
- filesystem contents
- installed software
- running processes
- network state

Prompting helps, but prompt text alone is not sufficient. The agent loop should
be able to reject or nudge unverified answers when the task clearly depends on
live executor state.

### C. Stream remote tool output through the user-visible progress path

Remote `exec` should stream stdout/stderr as it runs so users can see what is
happening on the selected executor in real time.

Recommended implementation:

- add a streamed remote-tool output message to the WS protocol
- forward remote output through the existing agent progress channel
- preserve locality metadata on streamed chunks
- surface streamed tool output directly in the TUI session view

### D. Improve remote filesystem grounding

The prompt and tool results should make remote path semantics clearer:

- execution target
- execution workdir hint
- gateway workspace hint
- whether a path exists on the selected executor

This should reduce spurious remote `read` attempts against invalid paths.

### E. Expose provenance in the user-visible answer

Final responses should carry visible execution context, for example:

- `executed on: active client`
- `executed on: gateway`
- `executed on: remote client <label>`

That keeps locality transparent without requiring the user to inspect logs.

### F. Make core filesystem tools first-class remote passthroughs

When the selected execution target is a client, tools like `read`, `write`,
`edit`, `glob`, `grep`, and `patch` should feel native there. The model should
not need to fall back to `exec cat`, `exec sed`, or other shell workarounds for
basic file operations.

### G. Fix unsafe truncation immediately

All response truncation in the gateway and TUI should be UTF-8 safe. No string
sent over the wire should be sliced at arbitrary byte offsets.

## Recommended Implementation Order

1. UTF-8-safe truncation in gateway WebSocket forwarding.
2. Stream remote `exec` output through the existing progress path.
3. Surface `executed on` provenance in user-visible tool output and summaries.
4. Harden core filesystem tools so remote clients accept the common parameter
   shapes models naturally use.
5. Revisit tighter convergence rules after streamed output makes tool behavior
   easier to observe.

## Related Tracking

See [Feature Evaluation Tracker](../feature-evaluation.md) for the additional
agent-driven product suggestions that came out of this testing session.
