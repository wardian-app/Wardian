# Spec 029: Agent List Ordering

- **Status:** Implemented
- **Date:** 2026-05-04
- **Decider:** Wardian maintainers

## Context and Problem Statement

Wardian currently appends newly registered agents to `agent_order`. This makes freshly spawned agents appear at the bottom of the roster and grid, even though the newest agent is usually the one the operator wants to inspect first.

Cloned agents have a different mental model: the clone belongs near its source because the operator is comparing or continuing from that source agent. Appending clones separates related sessions and makes the roster less tactile.

## Proposed Decision

Keep the Rust backend as the single source of truth for persistent agent ordering. Extend the internal registration path so it can place a new session ID using one of these strategies:

- Fresh spawn: insert the new session ID at the top of `agent_order`.
- Clone: insert the new session ID immediately after the source session ID.
- Fallback: if the requested clone source is missing from `agent_order`, insert the clone at the top rather than appending.

The frontend remains unchanged. `list_agents` already returns agents in backend order, and the grid/watchlist already render that order.

Workflow agent nodes are out of scope. Current workflow execution can run ephemeral/headless work and temporarily operate on mapped existing agents, but it does not create permanent roster agents.

## Consequences

- **Positive**: Freshly spawned agents are immediately visible at the top of the default list.
- **Positive**: Clones stay adjacent to their source agent for easier comparison.
- **Positive**: Existing drag reorder behavior and `list_agents` consumers continue to use one backend order.
- **Negative**: Operators who expected chronological oldest-first ordering will see new agents above older sessions.
- **Negative**: Clone ordering depends on the source still being present in `agent_order`; missing order metadata falls back to top insertion.
