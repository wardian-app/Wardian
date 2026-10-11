For distinct external controller identity, see [CONTROLLERS.md](CONTROLLERS.md). For an optional private ChatGPT tunnel route, see [SECURE-TUNNEL.md](SECURE-TUNNEL.md). The managed-agent adapter described below is a separate mode.

# Selected-agent MCP bridge

For an external assistant without a managed Wardian identity, use the separate
[external controller mode](CONTROLLERS.md), with owner-issued scoped grants and
optional HTTP transport. The managed adapter documented below retains its existing identity.

This optional, dependency-free adapter launches the existing native `wardian mcp
serve` server and exposes only the operator's selected tools and exact recipient
UUIDs. It adds no app settings, provider configuration, daemon, listener, tunnel,
plugin installation or automatic startup. Node.js 22 or newer is required.

Wardian remains the canonical owner of agent identity, permission checks,
interaction records, task admission, cursor acknowledgements and completion.
The bridge policy is a fixed, inspectable lens over those records. It is read once
at launch. Installing this directory does nothing until a local MCP host starts it.

## Configure deliberately

Copy `policy.example.json` outside the public repository. Replace all placeholders
and example UUIDs with the approved native CLI executable, Wardian home, existing
managed sender and selected recipients. Paths must be absolute; no shell command,
command wrapper, extra CLI arguments, environment expansion or name-based routes
are accepted. On Windows use `wardian-cli.exe` or `wardian-mcp.exe`, not
`wardian.cmd`. On POSIX use the installed native Wardian CLI binary.

Use Wardian's official identity inspection to resolve recipient UUIDs; do not copy
a session ID from another agent to impersonate it. The sender must be the existing
managed host's own identity, inherited through `WARDIAN_SESSION_ID`, and paired
with the same running app/home. The policy's sender must match that inherited
identity. A missing or different identity rejects every tool call before dispatch,
including mailbox reads. The adapter never writes or overrides `WARDIAN_SESSION_ID`,
creates an external controller, or registers an identity. Do not set that variable
manually to make an external assistant appear managed. Keep its policy private and protect it
like local CLI access. The example is intentionally invalid until paths and
identities have been replaced.

Launch explicitly:

```bash
node extensions/selected-agent-bridge/bridge.mjs '<absolute-policy-path>'
```

The same command works in PowerShell. For a local MCP-capable host use its normal
stdio server configuration with `command = "node"` and arguments containing the
absolute `bridge.mjs` and policy paths. Configure approvals in that host for only
the intended tools. This prototype ships no installer or grants and changes no
Codex, Wardian or agent configuration. A host may require a new session to discover
new tools; registration alone does not prove connectivity.

## Operations and scope

- `list_agents` retains only selected UUIDs and projects name, provider, status
  and status source. Both structured and text results are replaced; private
  workspace/process fields and unselected roster entries are excluded.
- `send_message` passes literal information without waking an agent.
- `followup_task` admits work and returns Wardian's canonical receipt. Admission
  does not prove provider visibility or completion. No names or broadcasts route.
- `wait_agent` wakes on the sender's mailbox activity without reading it.
- `receive_messages` reads the configured sender's mailbox, including its tasks,
  information and replies. It passes opaque cursor/acknowledgement tokens unchanged;
  selection limits **outgoing routing**, not mailbox contents. Use a dedicated,
  authorized sender if sharing an existing mailbox would disclose unrelated work.
- `reply` and `interrupt_agent` require explicit inclusion in the policy. Reply
  destination is determined by the request and checked by Wardian, not by a target
  parameter. A reply completes work as the sender's identity. Interruption requests
  affect the selected agent's current turn and are not request-specific cancellation.

All mailbox scope and provider capability limits of
[Wardian's messaging contract](../../docs/developer/agent-messaging-tools.md)
still apply. There is no unsolicited push into the assistant; call wait, receive,
then correlate replies using canonical IDs. A timeout does not cancel work. MCP
cancellation notifications are advisory and never interrupt an agent. Stopping
the bridge closes its own child server, not agents or already admitted tasks.

## Failure and authentication boundaries

Calls are serialized through one persistent upstream session. Frames are bounded
to one MiB, literal messages to 64 KiB, waits to 60 seconds and receive pages to
100 records. Unknown fields and malformed parameters are rejected before dispatch.
Typed tool-call IDs cannot be reused within the bridge session, which closes its
admission window at 10,000 calls. The upstream owns durable admission keys. No
response cache, retry, restart recovery or cross-session idempotency is claimed.

An upstream request has a 75-second deadline. Lost/malformed receipts close that
transport; delivery may already have occurred. Reconcile through Wardian before
restarting or resubmitting. Client disconnects and cancellation do not retract
admitted work. Tool receipts/errors otherwise remain canonical; child stderr is
discarded to avoid exposing paths, prompts or credentials through this adapter.

Local process access and Wardian's existing managed-origin checks are the only
authentication boundary here. A policy UUID is **not a new credential** or a
capability token; this wrapper is not isolation against a local caller who can
invoke Wardian directly or edit the policy. No remote authentication is implemented.

## Transport limits

Supported: local stdio MCP clients launched within an existing authorized Wardian
managed host on the Wardian machine, with that host's inherited identity and the
running matching app.
Mock tests prove the adapter exchange; discovery smoke checks prove only protocol
compatibility, not live agent delivery.

Unsupported: cloud assistants reaching a local stdio server, Streamable HTTP,
remote event delivery, a Codex plugin bundle, and external identities/grants.
Wardian's existing remote gateway is an authenticated app API, not this MCP
endpoint. This change neither enables nor uses it. A future cloud bridge requires
an approved transport, explicit scoped identity/authentication and tested event
delivery. Do not expose this stdio process through an unauthenticated tunnel.
See [MCP transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

## Validation

```bash
npm --prefix extensions/selected-agent-bridge test
node extensions/selected-agent-bridge/smoke-discovery.mjs '<absolute-native-cli-path>'
```

The same commands work in PowerShell. The dependency-free suite uses mock
backends and real stdio subprocesses, with temporary fixtures and no network
listener. The smoke check sends initialize and tool discovery only: it does not
read a Wardian home or control an agent. The extension's own path-filtered CI
workflow runs its suite on Linux and Windows. Live delivery needs a separately
authorized sender/recipient fixture and matching running application; it is not
part of these tests.
