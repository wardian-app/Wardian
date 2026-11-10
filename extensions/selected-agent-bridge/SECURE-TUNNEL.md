# Optional private Secure MCP Tunnel

This is a dormant, single-operator route from a private ChatGPT plugin to the
Wardian external controller. The optional `tunnel-stdio.mjs` relay runs under
Node 22+ and forwards approved operations to the native controller backend.
Installing this extension starts no tunnel, listener, service, or credentials.
Source and local tests do not establish connectivity from ChatGPT or a dot.

The supported route is ChatGPT -> OpenAI Secure MCP Tunnel -> one local
`tunnel-client` -> Node stdio relay -> Wardian native controller. It needs no
public Wardian endpoint. Tool arguments and results still pass through OpenAI.
The separate Streamable HTTP gateway is for clients that support its bearer
contract; it is not the entry point for this stdio route.

## Boundaries to approve

Use a dedicated tunnel, a dedicated named Wardian controller, exact selected
agent UUIDs, minimum scopes, and an expiry. Follow [controller setup](CONTROLLERS.md)
for owner-managed grants. The grant resource must match the relay configuration
exactly. Do not borrow an agent session ID.

This route uses **one fixed local controller principal**. It does not authenticate
each ChatGPT user separately: stdio has no forwarded HTTP authorization header,
and tunnel-client shares one child per channel. Everyone allowed to use this
tunnel/plugin can exercise that principal's capabilities and read its mailbox.
Keep OpenAI tunnel/workspace associations and plugin access limited to the
approved operator. Do not share this plugin as a multi-user controller. Per-user
authorization needs a different authenticated boundary.

The OpenAI runtime API key authorizes tunnel polling, not Wardian operations.
The Wardian token authorizes the local controller, not tunnel access. Keep the
runtime key in a private file referenced by `control_plane.api_key`; leave
`CONTROL_PLANE_API_KEY` unset so the stdio child does not inherit that secret.
Set `WARDIAN_CONTROLLER_TOKEN` only in the explicitly launched tunnel process's
environment. Never place either credential in arguments, profiles, URLs, source,
tool arguments, logs, or public evidence. The local OS account remains trusted.

## Prepare privately; do not activate by installation

The documented client is [OpenAI tunnel-client v0.0.16](https://github.com/openai/tunnel-client/releases/tag/v0.0.16),
which publishes Windows amd64 and arm64 bundles. Installation, binary integrity
verification, tunnel creation/association, runtime-key issuance, Wardian grant
creation, and starting the tunnel are separate approved operator actions.

Copy [tunnel-profile.example.yaml](tunnel-profile.example.yaml) outside the public
repository and replace its placeholders. Copy [controller.example.json](controller.example.json)
as the relay's private JSON configuration and set its absolute native CLI,
Wardian home, resource, and audit paths. It uses the same configuration shape as
the HTTP gateway; `port` is ignored in stdio mode and no MCP listener is opened.
The audit directory must exist. Protect both configuration and secret
files using the local OS's access controls. The profile starts exactly:

```sh
'<absolute-node-executable>' '<absolute-extension-path>/tunnel-stdio.mjs' --config '<absolute-private-relay-config>' --stdio
```

Windows uses the same command string in the YAML profile. The tunnel parser
splits it directly, without `cmd.exe` or PowerShell. Keep the inner single quotes
around all paths; they preserve backslashes and spaces. Do not replace them with
unescaped double-quoted Windows paths. No per-command environment map is supported.
The child inherits the tunnel process's environment. In particular, the Windows
profile command is `'<absolute-node.exe-path>' '<absolute-tunnel-stdio.mjs-path>'
--config '<absolute-private-relay-config>' --stdio`, on one logical line.

Before an approved launch, remove `WARDIAN_SESSION_ID` and
`WARDIAN_CONTROLLER_REQUEST_KEY` from that process environment. The relay
generates operation keys; a fixed inherited key must not bind unrelated tasks.
Supply the approved Wardian controller token through the environment/secret
manager. Do not export the Platform runtime API key.

After setup approval, the profile commands are:

```sh
tunnel-client doctor --profile <private-profile-name> --explain
tunnel-client run --profile <private-profile-name>
```

PowerShell uses the same commands with the installed `tunnel-client.exe`.
`doctor` can contact OpenAI; it is not an offline-only validation command.
`run` starts outbound HTTPS polling and the profile's loopback health/UI endpoint.
Run exactly one active client per tunnel ID; stop it before starting a replacement.
Do not enable remote UI, raw HTTP logging, payload capture, or cloudflared for
this route. A loopback health endpoint does not prove remote tool delivery.

## Connect the private ChatGPT plugin

An OpenAI administrator must associate the tunnel with the target ChatGPT
workspace and owning Platform organization. The runtime and plugin creator need
Tunnels Read + Use; tunnel management needs Read + Manage. Workspace custom-plugin
permissions apply separately. No account eligibility or permissions are inferred
from a local build.

After approval, open ChatGPT Plugins, choose Add custom MCP server, select
Connection: Tunnel, and choose the approved tunnel. For this fixed-principal
stdio route choose **No authentication**: OpenAI's tunnel/workspace/plugin access
is the remote access boundary, and the environment token is verified locally by
Wardian. This choice does not make the Wardian grant anonymous or create per-user
identity. Review the requested access before creating the private plugin.
Attach it to the intended chat or dot only through that product's supported UI;
plugin creation alone does not prove a dot can discover or invoke it.

## Protocol and acceptance evidence

The relay must handle current `server/discover` requests and legacy initialization
without depending on a new native stdio child for each remote session. Modern
requests carry protocol version and client capabilities in `_meta`; discovery
returns supported versions, capabilities, `resultType`, and private cache hints.
Repeated initialization and reused RPC IDs are correlation events, not durable
task identities. Use the documented `wardian/request_key` only for an explicit
logical replay, and reconcile uncertain receipts before resubmitting.

Before claiming this route works, record sanitized evidence for all of:

- Current discovery and tool listing from the intended ChatGPT chat or dot;
  only granted tools and selected recipients appear, with no worker `reply`.
- A selected test worker receives one task and returns its correlated canonical
  reply; the controller reads and explicitly acknowledges that reply.
- An unselected recipient and an ungranted operation are denied before admission.
- Revocation denies further discovery/operations and an active mailbox wait;
  already admitted work remains durable.
- Plugin refresh/reconnect works with repeated initialization and reused RPC IDs;
  lost receipts and cancellation cause no automatic task replay.
- Shutdown closes owned children, and no public Wardian listener was created.

Until those approved tests pass, cloud/dot delivery remains unverified. Missing
client installation, Platform/workspace association, permissions, runtime key,
Wardian grant, matching native build, or relay protocol support is a blocker;
do not replace any of them with a managed-agent identity or an unauthenticated
public tunnel.

References: [OpenAI Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels),
[client configuration](https://github.com/openai/tunnel-client/blob/v0.0.16/docs/configuration.md),
[transport/authentication boundaries](https://github.com/openai/tunnel-client/blob/v0.0.16/docs/architecture.md),
[MCP discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover).
