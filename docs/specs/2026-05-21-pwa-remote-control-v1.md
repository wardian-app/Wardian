# Wardian PWA Remote Control V1

Filename: `2026-05-21-pwa-remote-control-v1.md`

- **Status:** Implemented
- **Date:** 2026-05-21

## Context and Problem Statement

Wardian is currently a local-first desktop habitat. The Rust/Tauri backend owns
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
HTTPS client for that host, with Tailscale HTTPS as the v1 production path.

V1 will add a separate desktop-owned remote gateway for the PWA. The gateway must
not expose the existing named-pipe or Unix-socket control server directly. It
will instead map authenticated, audited web requests onto the same backend
capabilities used by the desktop UI and CLI.

The PWA will live in the existing repository as a web/mobile platform variant.
It should reuse shared TypeScript types, status utilities, theme tokens, queue
logic, and API DTOs where appropriate, but it should use a dedicated mobile shell
instead of trying to make the full desktop shell responsive.

## Host and Deployment Model

The host is the local Wardian desktop app. A phone client connects to that host
over a private network path.

V1 supports:

- Tailscale remote control.
- Tailscale HTTPS as the production path, using a canonical origin such as
  `https://<machine>.<tailnet>.ts.net`.
- Private-network access only through a trusted HTTPS origin.

V1 does not support:

- Public relay.
- Hosted Wardian agent execution.
- Direct browser access to the local CLI control pipe or socket.
- HTTP access to remote-control APIs.
- Self-signed certificate trust-on-first-use.
- Generic LAN binding without a trusted certificate.

Generic LAN binding can be revisited after v1 if Wardian can provide a trusted
HTTPS origin without weakening the remote-control boundary. HTTP-only local
diagnostics, if any, must be non-mutating and separate from the remote-control
gateway.

## Repository Placement

The PWA belongs in the existing repository as Wardian's web/mobile platform
variant. The implementation plan should choose the exact file layout, but the
expected direction is:

- Shared DTOs, state helpers, and theme tokens remain shared with the desktop
  frontend.
- Remote-specific frontend code lives under a dedicated mobile feature/view
  area, for example `src/features/remote` plus a mobile shell/view.
- Remote gateway Rust code lives outside the existing local control server and
  Tauri command modules, so browser requests cannot fall through to the local
  named-pipe or Unix-socket dispatcher.

## Certificate and Origin Policy

When remote access is enabled, Wardian records one canonical HTTPS origin for
the desktop remote gateway. The QR code, browser session, HTTP API, and
WebSocket gateway must all bind to that exact origin.

The gateway must enforce:

- Exact `Host` matching against the enabled canonical host.
- Exact `Origin` matching for browser requests and WebSocket upgrades.
- No wildcard CORS.
- No automatic acceptance of alternate hostnames, IP literals, or changed
  Tailscale machine names.

If the canonical Tailscale hostname or certificate identity changes, Wardian
must require an explicit repair or re-enable flow from the desktop before remote
access resumes.

Wardian may diagnose local remote-access readiness, including the loopback
gateway, Tailscale login state, and whether Tailscale Serve points the canonical
HTTPS origin at the Wardian gateway. Wardian should present those findings and
suggest concrete recovery commands, but it must not automatically mutate
Tailscale Serve, Funnel, certificate, firewall, or tailnet admin-console state.

## Security Requirements

Remote access is disabled by default and must be enabled from the desktop app.

V1 paired devices are full-control principals. A paired phone has the same
remote command authority as the desktop user for the exposed v1 mobile surface.
Read-only devices, scoped permissions, destructive-action confirmations, and
workflow-specific access scopes are post-v1 features. The pairing confirmation
UI must state that the incoming device receives full remote control.

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
- Server-side opaque access sessions with short lifetimes, represented in the
  browser by a host-only `HttpOnly`, `Secure`, `SameSite=Strict` cookie. Use the
  `__Host-` cookie prefix when the deployment path supports it.
- No authentication token in `localStorage`, URL query strings, URL fragments,
  or other persistent JavaScript-readable storage.
- Re-authentication through fresh challenge-response when a session expires.
- CSRF protection for HTTP APIs through a session-bound CSRF nonce and a custom
  header, plus exact `Origin` and Fetch Metadata checks.
- WebSocket upgrade protection with single-use WebSocket tickets.
- Immediate device and session revocation that closes active WebSockets.
- Rate limits and lockouts for pairing and authentication.
- Request size, payload, and connection limits.
- Audit logs for pairing, authentication, revocation, remote agent actions,
  workflow actions, transcript/output reads, and PTY input.

Initial rate-limit targets:

- Pairing offers: at most 3 created per 10 minutes per desktop user session.
- Pairing submissions: at most 10 attempts per offer.
- Pairing status checks: at most 120 per minute per pending pairing request.
- Authentication challenges: at most 5 issued per minute per device.
- Failed authentication signatures: lock the device for 10 minutes after 5
  failures in 10 minutes.
- WebSockets: at most 3 active remote status sockets per device session.
- Mutating HTTP requests: at most 120 per minute per device session, with lower
  endpoint-specific limits for kill/delete/clone style actions.

Initial payload limits:

- Prompt/action request bodies: 64 KiB unless a narrower endpoint limit is
  defined.
- Transcript/output responses: capped by line count and bytes.
- WebSocket events: bounded by message size and dropped or summarized when the
  stream falls behind.

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

Multiple paired devices are allowed. V1 should allow one active access session
per device and multiple status WebSockets under that active session.

Revocation invalidates the device, removes active access sessions, closes active
WebSockets, and rejects queued remote requests that have not reached the backend.
It cannot reliably roll back commands already dispatched to a PTY, agent, or
workflow, so revocation is a best-effort stop boundary for in-flight work.

## Pairing and Authentication Flow

Pairing:

1. The user enables remote access from the desktop settings UI.
2. Wardian creates a single-use pairing offer and displays a QR code.
3. The QR code encodes the canonical HTTPS gateway origin, pairing offer id,
   expiry, nonce, and desktop gateway identity public key or fingerprint.
4. The phone opens the PWA, verifies the HTTPS origin, and generates a
   non-extractable per-device asymmetric key pair.
5. The phone submits its public key, device label, and pairing offer response to
   the desktop gateway.
6. The desktop shows an explicit confirmation prompt with the incoming device
   label, device public-key fingerprint, gateway origin, and full-control
   warning. While a pairing offer is active, the desktop settings UI refreshes
   pending pairing approvals automatically so the user does not need a manual
   refresh after the phone submits its request.
7. On approval, Wardian stores the device public key, device label, created time,
   last-used time, and revocation state under Wardian settings.
8. The phone pins the accepted desktop gateway identity for future
   challenge-response checks.

The QR code must not contain a long-lived secret. If it includes a short-lived
pairing secret, that secret is single use, expires with the offer, and is not
sufficient without desktop approval.

Re-authentication:

1. The phone asks for an authentication challenge.
2. Wardian returns a single-use challenge bound to the device id, canonical
   origin, nonce, timestamp, intended audience, and current desktop gateway
   identity fingerprint.
3. The phone compares the returned desktop gateway identity fingerprint with
   the fingerprint accepted during pairing, then signs the challenge with its
   paired device private key.
4. Wardian verifies the signature, challenge freshness, nonce uniqueness, device
   status, origin, and host.
5. Wardian creates a server-side session, sets the opaque session cookie, and
   returns a session-bound CSRF nonce for mutating HTTP requests.

The recommended v1 device key is ECDSA P-256 generated through WebCrypto with
the private key marked non-extractable and stored in browser-managed key
storage, typically IndexedDB. If browser compatibility blocks that choice, the
implementation plan must document the replacement and its security tradeoffs.

WebSocket authentication:

1. The authenticated phone requests a single-use WebSocket ticket through an
   HTTPS `POST` protected by the session cookie and CSRF nonce.
2. Wardian creates a ticket bound to the device id, access session, canonical
   origin, requested stream, and a 15-60 second expiry.
3. The browser opens the WebSocket to the canonical WSS origin.
4. The gateway validates exact `Host` and `Origin` during upgrade.
5. The ticket is presented through a non-URL channel, such as a first message or
   a WebSocket subprotocol value, and the gateway does not emit stream data
   until the ticket is accepted.
6. The gateway closes the socket immediately when the session or device is
   revoked.

WebSocket tickets must not be placed in URL query strings.

QR codes are only required for first pairing, new devices, revoked devices,
cleared browser storage, or remote-access reset.

## Audit Logging

The remote gateway writes append-only JSONL audit records under Wardian state,
for example `<wardian-home>/remote-access/audit.jsonl`. The exact path belongs
in the implementation plan, but it must use the configured Wardian home rather
than a platform-specific absolute path.

Each audit record should include:

- `schema_version`
- `event_id`
- `timestamp`
- `request_id`
- `device_id`
- `session_id`
- `origin`
- `event_type`
- `action`
- `target_type`
- `target_id`
- `outcome`
- `error_code`, when applicable

Audited event types include pairing creation, pairing approval/rejection,
authentication success/failure, session expiration, revocation, roster reads,
transcript/output reads, agent actions, PTY input, workflow actions, and gateway
policy rejection.

The initial retention policy should rotate logs at 50 MiB and retain at least 90
days of remote-access audit history, subject to the user's normal Wardian state
cleanup settings.

## API and Gateway Shape

The remote gateway is a separate module from the existing Tauri command handler
and local CLI control server.

The gateway should expose a small, explicit API surface for the mobile PWA:

- Authentication and pairing.
- Device list and revocation.
- Agent roster and status.
- Selected-agent chat transcript reads.
- Agent action requests.
- Queue/completion state.
- Workflow list and run/stop requests.
- Live status stream over WebSocket.

Gateway requests should be translated into backend operations through typed
internal calls. The implementation should avoid routing arbitrary
`ControlRequest` JSON from browsers into the local control dispatcher.

All state-changing requests and sensitive reads must be audited with the remote
device id and remote session id.

## V1 Product Surface

The v1 PWA should be a simplified mobile command surface, not a responsive copy
of the full desktop shell.

The primary screen is a single-column command grid that opens a selected-agent
conversation view:

- Stacked agent cards.
- Agent name, status, provider, class, and workspace/project.
- Latest thought, transcript summary, or completion summary when available.
- Tap an agent card to open a mobile conversation view.
- The conversation view reads the same normalized chat transcript model used by
  desktop grid chat mode and sends prompts through the same backend action path.
- Multi-agent broadcast is intentionally not exposed from the mobile roster;
  use the desktop app for broadcast workflows.
- Pause/resume, clear, kill/delete, and clone where backend support is already
  straightforward.
- Queue/completion triage as a tab or filter.
- Lightweight workflow run/stop list.

V1 excludes:

- Full xterm PTY rendering.
- Visual workflow builder.
- Desktop three-column shell.
- Public relay.
- Offline action queueing.

If terminal context is needed, v1 should expose it through an explicit desktop
setting or endpoint rather than returning transcript tails in the default remote
roster. The v1 default is to omit transcript/output text from paired-device
roster responses.

Any future sanitized transcript/output view is not a secret-scrubbing
guarantee. It should prefer existing transcript or completion summaries, strip
ANSI and OSC control sequences, cap returned lines and bytes, and avoid raw PTY
streaming.

The PWA service worker should cache only the app shell and static assets needed
to load the mobile UI. It must not queue agent, workflow, PTY, or revocation
actions while offline. When the desktop is unreachable, the UI should enter an
unreachable state and require live reconnection before sending commands.

## Data Flow

The phone loads the PWA over HTTPS from the Wardian desktop remote gateway.
After pairing and re-authentication, the phone receives a short-lived session.

HTTP APIs handle discrete actions such as agent control and workflow run
requests. Remote workflow run requests do not accept arbitrary JSON payloads in
v1; workflow-specific payload schemas are required before phone-submitted
workflow inputs are accepted. WebSockets provide live roster, status, queue, and
workflow updates.
WebSocket connections require authenticated upgrade protection and must close
immediately when the device or session is revoked.
Polling may be used only as a temporary development fallback while the mobile
shell is incomplete; v1 must consume the WebSocket status stream before the
remote route is considered shippable.

Browser credentials are only the opaque session cookie and the in-memory
session-bound CSRF nonce. Persistent device identity is the non-extractable
private key, not a reusable bearer token.

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
- Missing or incorrect CSRF nonce rejection.
- Unauthenticated WebSocket rejection.
- Reused WebSocket ticket rejection.
- URL-carried WebSocket token rejection.
- WebSocket closure after revocation.
- Payload and connection limits.
- Session cookie attributes.
- Canonical origin repair or re-enable when the host identity changes.
- Successful remote agent action through the gateway.
- Successful remote workflow run/stop through the gateway.
- Audit log creation for remote actions.
- Audit log creation for sensitive reads.
- Mobile PWA status updates through a single-use WebSocket ticket rather than
  polling-only refresh.
- Offline PWA behavior that does not queue mutating actions.

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
- **Negative:** Generic LAN access without a trusted HTTPS origin stays out of
  v1.
- **Negative:** Paired devices are full-control principals in v1; finer scopes
  are deferred rather than partially implemented.
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
