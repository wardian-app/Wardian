# Optional selected-agent and external-controller MCP bridge

- Status: Implemented; runtime activation and public HTTPS routing are explicit owner actions
- Date: 2026-10-11
- Related: #57 (MCP cognitive interface)

## Decision

Keep the application surface small: optional dependency-free Node transport plus an explicit native controller capability port. Existing managed-agent stdio clients retain their identity and behavior. External clients use a separate named controller UUID and `controller:<uuid>` mailbox, never a copied agent identity. Controllers have no provider process, agent record, workspace assignment or lifecycle generation.

Owner-issued grants select exact agent UUIDs, explicit operation scopes and one canonical HTTPS resource. Only credential hashes persist. Creation, listing and revocation are local owner CLI actions; verification emits secret-free policy. Native canonical dispatch independently checks the grant, target, capability and resource, including acknowledgement and quiet wait revocation. A redacted credential DTO is confined to the new ControllerMessaging control request; existing MessageOrigin privileges are not widened.

The existing canonical interaction database already supports namespaced sender and cursor owners without agent foreign keys. Reuse ordinary admission, provider delivery claims, worker replies and native completion outbox. Information does not enqueue work. Tasks require both submit and wake capability. Controllers read only their own durable mailbox and cannot reply as workers. Controller replies never trigger provider spawn for the controller owner.

## Transport and activation

Local controller clients use native `mcp serve --controller` with environment-only token and exact resource. The optional HTTP gateway verifies the native grant before dispatch and disclosure, validates Host/Origin, bounds frames/messages/waits and records metadata-only audit events. It uses stateless MCP 2025-11-25 POST JSON responses; receive/wait provide polling instead of SSE. An explicit optional `wardian/request_key` metadata string yields a principal-bound canonical admission key; ordinary JSON-RPC IDs correlate responses and new calls receive fresh keys. Payload collisions reject; no automatic retries or request-specific cancellation are implied.

The gateway binds loopback only when deliberately started with configuration and `--listen`. Remote integration requires an operator-managed HTTPS route and a client supporting manually provisioned bearer headers. OAuth login, dynamic registration, automatic connector installation, tunnel provisioning, persistent startup and public exposure are outside this release. Cloud clients cannot reach a local stdio process or loopback socket alone. No runtime grant or route is created by installation/build/tests.

The system constrains remote callers, not arbitrary processes with the same local OS account. Revocation stops subsequent operations and result disclosure; it cannot undo work already admitted before the change. MCP cancellation notifications and disconnected clients do not retract durable tasks. Interrupt is a separately scoped current-turn operation, not process termination or cancellation of a particular request.

## Verification and documentation

Extension tests cover selected routing, mixed identity rejection, scoped tool discovery, authentication/revocation/audience, audit failure, literal text, typed request IDs, bounded frames, child termination and real subprocess protocol exchanges. Native tests use isolated homes to exercise canonical task admission, provider claims, worker replies, acknowledgement, completion outbox and revocation. An opt-in native acceptance driver connects transient HTTP through the actual compiled CLI to the real canonical handler and a mock worker; it verifies non-waking information, one durable task under replay and reply acknowledgement.

The extension CONTROLLERS guide documents owner setup and exact integration limits; CLI and messaging developer guides link to it. No application UI or provider defaults change, so no screenshot refresh applies. This feature addresses direct selected-agent control in #57, without claiming its wider workflow, spawn and storage vision.
