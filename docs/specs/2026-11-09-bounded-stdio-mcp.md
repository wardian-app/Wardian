# Bounded concurrent stdio MCP

- **Status:** Implemented
- **Date:** 2026-11-09
- **Issue:** #1709

## Context and Problem Statement

The stdio adapter previously handled each request through completion before
reading the next request. A slow control exchange therefore delayed unrelated
calls and notifications on that connection. On Windows, the GUI launcher also
retained an adjacent CLI child for the lifetime of each connection.

The desktop application already owns the shared control backend. Replacing
that backend or adding a network endpoint is unnecessary for this change.

## Decision

The console CLI and the Windows GUI executable use one shared library server.
`wardian-mcp mcp serve` runs that server directly with inherited stdio handles;
it does not delegate execution to an adjacent CLI process. The GUI executable
preserves managed sender identity, home selection, and existing registrations.

A connection admits tool calls to four workers and one queued position.
Excess work is rejected before a backend invocation. Request framing remains
limited to one MiB, and the output queue contains at most five responses.
These are queue and framing bounds, not a total process-memory guarantee.

Responses are correlated by request ID and written as complete JSON lines.
Concurrent calls can finish out of order. A client must await a mutation's
response before submitting another mutation that depends on it. Initialization
and request-ID admission remain connection-owned. Reserved call identities
are not recycled to replay a submission.

Cancellation remains advisory. It does not interrupt an agent, retract an
admitted message, or authorize a second submission. A transport failure can
leave delivery uncertain; the adapter must retain the existing no-replay
behavior. Backend exchange budgets still apply, and queued time can add to
the time a caller waits. This change does not promise that every client
deadline is satisfiable under saturation.

Closing input stops intake and joins admitted work. Output frames are drained
with a separate five-second deadline; output failure must not leave the
executable waiting indefinitely for more input. Failure is reported through
the process result and stderr, without protocol-corrupting stdout logs.

## Consequences

- Windows connections retain one bridge process instead of a launcher-child
  pair. They remain persistent stdio connections, not one process per call.
- Independent calls can progress concurrently, with bounded admission and
  explicit overload behavior. Dependent mutations require client sequencing.
- The desktop remains the authority for durable message/task admission.
  Connection-local queues are not a second durable task system.
- Existing managed sessions need the updated executable at their next normal
  bridge launch. A source change does not replace a running bridge.
- Durable agent-spawn operations and shared HTTP MCP are separate decisions.
  This change does not attribute historical spawn timeouts or app exits.

## Verification

Maintained MCP unit and stdio integration tests cover gated slow/fast calls,
overload and duplicate IDs, advisory cancellation, awaited mutation ordering,
framing, EOF, and output backpressure. Windows integration tests exercise the
GUI subsystem and verify that the server starts no adjacent CLI child.
Controlled pipe fixtures establish adapter behavior; they do not establish
real-provider readiness, desktop responsiveness, or deployed process counts.

See the [agent messaging reference](../developer/agent-messaging-tools.md) and
[provider runtime guide](../developer/provider-runtimes.md) for operator-facing
configuration and lifecycle behavior.
