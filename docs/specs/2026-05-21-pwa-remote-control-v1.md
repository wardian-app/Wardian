# Wardian PWA Remote Control V1

Filename: `2026-05-21-pwa-remote-control-v1.md`

- **Status:** Proposed
- **Date:** 2026-05-21
- **Decider:** Product/Engineering

## Context and Problem Statement

Wardian is currently a local desktop command center. The Rust/Tauri backend owns
agent lifecycle, PTY state, telemetry, workflows, filesystem operations, and the
local CLI control endpoint. The existing live control endpoint is intentionally
local: it is exposed through a Windows named pipe or Unix socket derived from
`WARDIAN_HOME`, and it does not provide a browser-safe authentication boundary.

The progressive web app goal for v1 is narrower than a hosted Wardian service.
The user wants to control their own local Wardian desktop runtime from a phone,
including over Tailscale. Public relay and cloud-hosted agent execution do not
belong in v1.

This is a high-risk feature. A paired phone that can control Wardian can inject
input into agents, run workflows, pause or kill sessions, and direct agents that
may have broad access to the user's computer. The design therefore treats remote
access security as the core feature, not as an implementation detail.

## Proposed Decision

Wardian desktop remains the host and source of authority. The PWA is a paired
LAN/Tailscale client for that host.

V1 will add a separate desktop-owned remote gateway for the PWA. The gateway must
not expose the existing named-pipe or Unix-socket control server directly. It
will instead map authenticated, audited web requests onto the same backend
capabilities used by the desktop UI and CLI.

The PWA will live in the existing repository as a web/mobile platform variant.
It should reuse shared TypeScript types, status utilities, theme tokens, queue
logic, and API DTOs where appropriate, but it should use a dedicated mobile shell
instead of trying to make the full desktop command center responsive.

## Host and Deployment Model

The host is the local Wardian desktop app. A phone client connects to that host
over a private network path.

V1 supports:

- Tailscale/private-network remote control.
- Tailscale HTTPS as the recommended production path, using a canonical origin
  such as `https://<machine>.<tailnet>.ts.net`.
- Explicit LAN binding only when the user enables it.

V1 does not support:

- Public relay.
- Hosted Wardian agent execution.
- Direct browser access to the local CLI control pipe or socket.
- HTTP production access to unrestricted remote-control APIs.

## Security Requirements

Remote access is disabled by default and must be enabled from the desktop app.

Before unrestricted remote agent and workflow control is allowed, v1 must
implement all of the following controls:

- HTTPS and WSS only for phone traffic.
- Exact canonical `Host` and `Origin` allowlists.
- No wildcard CORS.
- Pairing that requires desktop presence and explicit desktop confirmation.
- Single-use pairing QR codes with a 60-120 second lifetime.
- Per-device asymmetric credentials rather than shared persistent bearer
  secrets.
- Single-use authentication challenges with a 30-60 second lifetime.
- Server-side opaque access sessions with short lifetimes.
- No persistent JavaScript-readable refresh token.
- Re-authentication through fresh challenge-response when a session expires.
- CSRF protection for HTTP APIs.
- WebSocket upgrade protection, preferably with single-use WebSocket tickets.
- Immediate device and session revocation that closes active WebSockets.
- Rate limits and lockouts for pairing and authentication.
- Request size, payload, and connection limits.
- Audit logs for pairing, authentication, revocation, remote agent actions,
  workflow actions, and PTY input.

The persistent device credential is not a login session. It is only a paired
device identity that can sign fresh challenges. If the phone loses browser
storage, the device is revoked, or the desktop resets remote access, the user
must pair again.

Recommended session timing:

- Pairing QR: 60-120 seconds, single use.
- Authentication challenge: 30-60 seconds, single use.
- Access session: 5-15 minutes.
- Idle timeout: 15 minutes.
- Absolute session lifetime: 8-12 hours.
- WebSocket ticket: 15-60 seconds, single use.
- Device credential: persistent until revoked.

An access session is the short-lived authenticated server-side session that lets
the phone call APIs and open WebSockets. The absolute session lifetime is the
hard cap after which the phone must prove possession of its paired device key
again, even if it has remained active.

## Pairing and Authentication Flow

Pairing:

1. The user enables remote access from the desktop settings UI.
2. Wardian creates a single-use pairing offer and displays a QR code.
3. The phone opens the PWA and generates a per-device asymmetric key pair.
4. The phone submits its public key, device label, and pairing offer response to
   the desktop gateway.
5. The desktop shows an explicit confirmation prompt with the incoming device
   identity.
6. On approval, Wardian stores the device public key, device label, created time,
   last-used time, and revocation state under Wardian settings.

Re-authentication:

1. The phone asks for an authentication challenge.
2. Wardian returns a single-use challenge bound to the device id, canonical
   origin, nonce, timestamp, and intended audience.
3. The phone signs the challenge with its paired device private key.
4. Wardian verifies the signature, challenge freshness, nonce uniqueness, device
   status, origin, and host.
5. Wardian creates a server-side session and returns only the browser credential
   needed to use that session.

QR codes are only required for first pairing, new devices, revoked devices,
cleared browser storage, or remote-access reset.

## API and Gateway Shape

The remote gateway is a separate module from the existing Tauri command handler
and local CLI control server.

The gateway should expose a small, explicit API surface for the mobile PWA:

- Authentication and pairing.
- Device list and revocation.
- Agent roster and status.
- Agent action requests.
- Queue/completion state.
- Workflow list and run/stop requests.
- Live status stream over WebSocket.

Gateway requests should be translated into backend operations through typed
internal calls. The implementation should avoid routing arbitrary
`ControlRequest` JSON from browsers into the local control dispatcher.

All state-changing requests must be audited with the remote device id and remote
session id.

## V1 Product Surface

The v1 PWA should be a simplified mobile command surface, not a responsive copy
of the full desktop shell.

The primary screen is a single-column command grid:

- Stacked agent cards.
- Agent name, status, provider, class, and workspace/project.
- Latest thought, transcript summary, or completion summary when available.
- Card expansion for details.
- Send prompt to one agent.
- Broadcast or send to selected agents.
- Pause/resume, clear, kill/delete, and clone where backend support is already
  straightforward.
- Queue/completion triage as a tab or filter.
- Lightweight workflow run/stop list.

V1 excludes:

- Full xterm PTY rendering.
- Visual workflow builder.
- Desktop three-column shell.
- Public relay.

If terminal context is needed, v1 should expose a sanitized transcript or output
tail rather than raw PTY streaming by default.

## Data Flow

The phone loads the PWA over HTTPS from the Wardian desktop remote gateway.
After pairing and re-authentication, the phone receives a short-lived session.

HTTP APIs handle discrete actions such as agent control and workflow run
requests. WebSockets provide live roster, status, queue, and workflow updates.
WebSocket connections require authenticated upgrade protection and must close
immediately when the device or session is revoked.

The desktop backend remains authoritative for all runtime decisions. The PWA
does not own agent state, workflow state, or remote execution state.

## Error Handling

The PWA should show explicit user-facing states for:

- Remote access disabled.
- Pairing expired.
- Device pending desktop approval.
- Device revoked.
- Session expired and re-authentication required.
- Desktop unreachable.
- Tailscale/private-network disconnected.
- Certificate or origin mismatch.
- WebSocket disconnected.
- Command rejected by gateway policy.

Errors should distinguish authentication failure, connectivity failure, and
backend command failure.

## Testing and Verification

The implementation plan must include automated tests for:

- Expired pairing QR rejection.
- Pairing QR reuse rejection.
- Expired authentication challenge rejection.
- Replayed signature rejection.
- Revoked device rejection.
- Expired session rejection.
- Absolute session lifetime enforcement.
- Bad `Host` rejection.
- Bad `Origin` rejection.
- Cross-origin mutation rejection.
- Unauthenticated WebSocket rejection.
- WebSocket closure after revocation.
- Payload and connection limits.
- Successful remote agent action through the gateway.
- Successful remote workflow run/stop through the gateway.
- Audit log creation for remote actions.

Browser E2E tests can prove mobile UI behavior and mock remote gateway flows.
Native runtime E2E is required for claims that the remote gateway drives real
Wardian backend actions.

## Consequences

- **Positive:** Wardian keeps its local-first posture while enabling phone
  control over Tailscale/private networks.
- **Positive:** The desktop remains the host, preserving local filesystem,
  provider, PTY, workflow, and telemetry authority.
- **Positive:** A separate gateway creates a real browser security boundary
  instead of weakening the local CLI control endpoint.
- **Positive:** A mobile-specific command grid targets the actual phone
  workflow without forcing the desktop shell onto a narrow screen.
- **Negative:** Tailscale HTTPS and certificate/origin handling become setup
  constraints for v1.
- **Negative:** The security requirements make this more than a simple PWA
  manifest/service-worker change.
- **Negative:** The PWA cannot safely expose raw PTY streaming or broad generic
  control JSON until the remote gateway boundary is tested.
- **Negative:** Users who clear browser storage or revoke a device must pair
  again.

## References

Relevant existing Wardian surfaces:

- `src-tauri/src/lib.rs`: starts the local control server and registers broad
  Tauri command handlers.
- `src-tauri/src/control.rs`: accepts and dispatches local control requests.
- `crates/wardian-core/src/control.rs`: defines current local control request
  and response types.
- `crates/wardian-cli/src/live.rs`: connects to the current local named
  pipe/Unix socket endpoint.
- `src-tauri/src/commands/terminal.rs`: writes input into agent PTYs.
- `src-tauri/src/commands/workflow.rs`: runs workflows.
- `docs/specs/2026-05-07-cli-agent-control.md`: documents mutable local
  control through the local endpoint.
- `docs/guide/cli.md`: documents current CLI live-control behavior.
