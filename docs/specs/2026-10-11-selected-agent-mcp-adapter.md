# Optional selected-agent MCP adapter

- Status: Local prototype; no runtime activation or cloud connectivity
- Date: 2026-10-11
- Related: #57 (MCP cognitive interface); #29 (extension architecture)

## Decision

Keep the public application unchanged. Provide a dependency-free optional Node
adapter under `extensions/selected-agent-bridge/` using `wardian mcp serve` as
the sole integration seam. Unlike the historical cross-provider bridge proposal,
current MCP already exposes canonical information, tasks, receive, wait, reply
and interruption. The prototype requires no provider internals, terminal input,
database writes, counterfeit identities or new control protocol.

The fixed operator policy names the existing managed host's own sender, matching
home/native CLI, selected exact recipient UUIDs and allowed tools. The backend
remains the owner of canonical delivery records and identity validation. Selection
constrains outgoing routes and roster discovery; receive/reply operate on the
sender's mailbox with its existing permissions. This is a local convenience
boundary, not new authentication or a sandbox against local users.

The adapter inherits the host's `WARDIAN_SESSION_ID` without overriding it.
Before any tool dispatch it requires that inherited identity to match the policy.
An external assistant with no managed identity is rejected; a policy UUID cannot
mint attribution. A sanctioned external-controller principal remains a backend
design prerequisite, not something this adapter creates by copying another agent.

Information remains non-waking; work returns a request receipt; reply/wait/receive
keep Wardian's correlation and cursor rules. Interrupt is separately permitted
and never inferred from MCP cancellation. One persistent serialized stdio session
preserves upstream admission semantics. The adapter rejects reused tool-call IDs
and never retries lost receipts. Cross-session exactly-once behavior is not claimed.

## Delivery boundary

Supported local transport: newline JSON-RPC stdio subprocesses, MCP 2025-11-25
and 2025-06-18, tools-only surface. This cannot connect a cloud assistant to a
local machine. No listener, tunnel, credentials, provider tool grants, persistent
configuration or live external identity is installed. The existing remote gateway
has a separate authenticated app contract; it is not assumed to be remote MCP.
Cloud reachability and least-privilege external principal issuance remain blocked
on an approved transport/authentication design and end-to-end runtime evidence.

## Evidence and publication scope

The extension suite tests routing/policy rejection, roster sanitization, literal
payloads, operation distinctions, opaque cursors, advisory cancellation, duplicate
IDs, lost receipts, bounded framing, subprocess identity projection and deadline
failure. Path-filtered Linux/Windows CI keeps these tests independently reachable.
Native CLI discovery smoke invokes no agent operation and proves only protocol
compatibility. No live delivery claim follows from mock or discovery evidence.

The extension README owns setup, limits and troubleshooting. No application
UI, CLI command, docs site navigation or provider defaults change; no screenshot
refresh is applicable. Keep personal policy files and test runtime identities out
of this public repository. This slice relates to #57 without claiming to complete
its wider spawn/workflow/storage scope or the general plugin architecture in #29.
