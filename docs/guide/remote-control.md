# Remote Control

Wardian Remote lets a paired phone control the Wardian desktop app over a
trusted private-network HTTPS origin. The desktop remains the host for agents,
workflows, filesystem access, provider CLIs, PTYs, and telemetry.

## Requirements

- Wardian desktop running on the host computer.
- Tailscale enabled on the host and phone.
- A trusted HTTPS origin such as `https://<machine>.<tailnet>.ts.net`.
- Remote access configured to bind only to the Wardian loopback gateway.

For Tailscale setup, MagicDNS naming, Tailscale Serve, Tailscale SSH, and the
difference between private Serve and public Funnel access, see
[Remote Access With Tailscale](./remote-access-tailscale.md).

## Enable Remote Access

1. Open Settings.
2. Open Remote Access.
3. Confirm the canonical Tailscale HTTPS origin.
4. Start Tailscale Serve for the Wardian loopback gateway.
5. Create a pairing code.
6. Scan the code from the phone and approve the device on the desktop.
7. Open the remote URL from the phone while connected to the same tailnet.

The pairing code is short-lived and single use. The phone generates its own
device key during pairing, then waits for explicit desktop approval before it
can create a remote session.

## Security Model

The Wardian desktop is the host. The phone is only a remote control surface, and
paired devices have full control of the v1 mobile surface. Treat a paired phone
like an unlocked desktop session.

Wardian rejects remote-control HTTP access, wildcard origins, non-loopback
gateway binding, reused pairing offers, reused WebSocket tickets, missing CSRF
nonces, and revoked devices.

The default remote roster does not include transcript or output text. Remote
workflow runs also reject arbitrary phone-provided payloads until Wardian has
workflow-specific input schemas.

Revoke a lost or untrusted phone immediately from Settings. Revocation ends the
device's active remote sessions and prevents future gateway calls from that
device.

## Mobile Surface

The v1 mobile shell is a simplified single-column control grid for small
screens. It is focused on:

- Viewing active agent status.
- Selecting one or more agents.
- Sending prompts to selected agents.
- Running basic lifecycle actions such as pause, resume, clear, and kill.
- Launching saved workflows from the mobile workflow list.

The service worker caches only the remote app shell and static assets. It does
not queue agent, workflow, PTY, or revocation actions while offline. If the
desktop is unreachable, reconnect before sending commands.

## Boundaries

Public relay access is not part of v1. Device scopes are not part of v1. Raw PTY
streaming is not part of v1 by default; remote views should use sanitized status
or transcript summaries unless a later design explicitly expands that surface.
