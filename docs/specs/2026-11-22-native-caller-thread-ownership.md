# Native caller threads and canonical task ownership

## Decision

An assigned Wardian agent's native root owns its canonical task. A native
helper fork can inherit the conversation and Wardian agent environment without
inheriting terminal authority. Output text cannot be checked semantically to
decide whether the parent's whole task is complete.

Reuse native thread identities. Extend the existing agent `MessageOrigin` with
one optional `provider_thread_id`; introduce no token or additional agent
identity. MCP reads `_meta.threadId` or
`_meta["x-codex-turn-metadata"].thread_id`, rejecting an inconsistent pair.
Ordinary CLI calls read `CODEX_THREAD_ID`. MCP must never borrow a shared server
process's ambient thread identity for a call without per-call metadata.

The isolated Codex 0.167.0 probe executed `fork_context: true` against owned
loopback model/MCP peers. Root and child had distinct native thread IDs while
sharing `sessionId`. Shell calls likewise retained the Wardian agent ID while
receiving different `CODEX_THREAD_ID` values. A provider session ID alone cannot
distinguish the callers.

## Settlement and progress

Automatic completion retains the exact input receipt, generation, thread and
output-turn rule from [task delivery receipts](./2026-11-22-task-delivery-receipts.md).
The native observation thread gate runs before receipt/final tracking, so a
child cannot borrow copied root input or turn IDs.

For explicit Codex replies, present provenance must match the assigned task
root and current runtime generation. Before submitting input, Wardian records
the owned root on the active scheduler claim using its existing recipient
origin fields. This lets a root reply before the provider acknowledges its
turn. Automatic completion still requires the acknowledged turn binding and
accepted-input receipt. Foreign or stale provenance is rejected
with an actionable error. Its body is retained as informational progress for
the owning parent, labelled with the caller thread, and never becomes the
parent's terminal reply to the requester. Existing worker parentage determines
native-child attribution. Child progress messages use the same parent route.
If the current root is still being discovered, a send with thread provenance
returns a retriable error. Progress whose original task root is stale or
unavailable remains in history outside the provider delivery queues.

Missing provenance preserves legacy explicit replies and records a
`thread_provenance_unavailable` diagnostic. Older clients omit the field;
manual CLI calls and scripts remain supported. Other providers retain their
existing behavior. Original task IDs and normal requester continuation routes
remain unchanged.

## Trust model

This prevents accidental mis-settlement by cooperating same-user processes.
It is not a security boundary against a process deliberately forging its
environment or local control requests. Native IDs are correlation evidence,
not credentials. No semantic judgement of helper output grants authority.
