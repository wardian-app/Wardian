# Optional external controller bridge

The controller mode gives an external assistant its own named identity and durable mailbox. It uses the ordinary Wardian task admission, provider delivery claims, replies and native completion outbox. It never borrows a managed agent UUID. The existing managed-agent adapter remains available separately.

Requirements: a Wardian build containing controller support and a running Wardian app using the configured home. Local stdio requires the Codex CLI; Node 22+ is required only for the optional gateway and its tests. The Node gateway uses stateless MCP Streamable HTTP POST with JSON responses. There is no SSE stream: use `receive_messages` or `wait_agent` to poll durable replies/events. Reading does not acknowledge messages; explicitly supply the returned cursor as `ack_cursor` to acknowledge them.

## Owner setup

Creating credentials and starting network access are explicit owner actions. Importing the extension, building Wardian, and discovering MCP tools issue no credentials and start no listener. Run the following from an owner terminal outside a managed Wardian session, replacing the example UUID and resource with the exact selected agent and endpoint:

```sh
wardian controller create --name assistant --resource https://bridge.example/mcp \
  --agent 20000000-0000-4000-8000-000000000000 \
  --scope agents:read --scope messages:send --scope tasks:submit --scope agents:wake \
  --scope controller:receive --scope controller:ack --scope controller:wait \
  --expires-at '<future-UTC-expiry>'
```

PowerShell (the same grant, expressed on one line):

```powershell
wardian controller create --name assistant --resource https://bridge.example/mcp --agent 20000000-0000-4000-8000-000000000000 --scope agents:read --scope messages:send --scope tasks:submit --scope agents:wake --scope controller:receive --scope controller:ack --scope controller:wait --expires-at '<future-UTC-expiry>'
```

Creation prints `{controller,token}` once. Keep the token in a secret store; only its hash is persisted in the Wardian home. List public grants with `wardian controller list`; revoke with `wardian controller revoke <controller-uuid>`. Revocation denies subsequent operations and quiet waits are rechecked every 250 ms. Already admitted work is durable and is not retracted by revocation, disconnect, timeout or MCP cancellation notifications.

### Local Codex stdio

Add this server to the shared Codex CLI configuration at `<codex-home>/config.toml`. Replace the command with the absolute path to the Wardian CLI executable (`wardian-mcp.exe` on Windows). In Windows TOML, use forward slashes in the path or escape each backslash. `env_vars` lists names Codex may forward from its own launch environment; it contains no credential values.

```toml
[mcp_servers.wardian-controller]
command = "<absolute-path-to-wardian-cli>"
args = ["mcp", "serve", "--controller"]
env_vars = ["WARDIAN_HOME", "WARDIAN_CONTROLLER_TOKEN", "WARDIAN_CONTROLLER_RESOURCE"]
tool_timeout_sec = 70
```

Set `WARDIAN_HOME` to the home used by the running Wardian app and set `WARDIAN_CONTROLLER_RESOURCE` to the exact resource used when creating the grant. Then start Codex from a terminal where `WARDIAN_CONTROLLER_TOKEN` is set. Codex launches the local Wardian stdio server; no HTTP listener, proxy, managed-session ID, or managed identity bootstrap is needed. The grant should name only the exact worker UUID the controller may access.

POSIX shell:

```sh
export WARDIAN_HOME='<absolute-wardian-home>'
export WARDIAN_CONTROLLER_RESOURCE='https://bridge.example/mcp'
wardian controller create --name assistant --resource "$WARDIAN_CONTROLLER_RESOURCE" \
  --agent '<selected-agent-uuid>' \
  --scope agents:read --scope messages:send --scope tasks:submit --scope agents:wake \
  --scope controller:receive --scope controller:ack --scope controller:wait \
  --expires-at '<future-UTC-expiry>'
read -r -s -p 'Paste the printed controller token: ' WARDIAN_CONTROLLER_TOKEN
printf '\n'
export WARDIAN_CONTROLLER_TOKEN
codex mcp list
codex
unset WARDIAN_CONTROLLER_TOKEN
```

PowerShell:

```powershell
$env:WARDIAN_HOME = '<absolute-wardian-home>'
$env:WARDIAN_CONTROLLER_RESOURCE = 'https://bridge.example/mcp'
wardian controller create --name assistant --resource $env:WARDIAN_CONTROLLER_RESOURCE --agent '<selected-agent-uuid>' --scope agents:read --scope messages:send --scope tasks:submit --scope agents:wake --scope controller:receive --scope controller:ack --scope controller:wait --expires-at '<future-UTC-expiry>'
$secureToken = Read-Host 'Paste the printed controller token' -AsSecureString
$tokenPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
try {
  $env:WARDIAN_CONTROLLER_TOKEN = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($tokenPointer)
} finally {
  [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($tokenPointer)
}
try {
  codex mcp list
  codex
} finally {
  Remove-Item Env:WARDIAN_CONTROLLER_TOKEN -ErrorAction SilentlyContinue
  Remove-Variable secureToken, tokenPointer -ErrorAction SilentlyContinue
}
```

The token is required in the launched process environment, so it exists in memory while Codex runs. Do not put its value in `config.toml`, command arguments, scripts, logs, or source control. Do not configure `WARDIAN_SESSION_ID` or `WARDIAN_CONTROLLER_REQUEST_KEY`; the server creates a fresh per-connection admission namespace.

For a remote client, copy `controller.example.json`, set absolute paths, the exact external HTTPS MCP resource, and any browser origins explicitly allowed. The audit file's parent directory must already exist. Start deliberately:

```sh
node server.mjs --config /absolute/path/controller.json --listen
```

The process binds only `127.0.0.1` and the configured port. An operator-managed HTTPS reverse proxy must route that resource to this loopback port and preserve its original Host header. The extension does not provision TLS, a tunnel, a public bind, background service, or firewall rules. A remote/cloud client cannot reach localhost by itself. Configure its endpoint and `Authorization: Bearer <token>` only after the owner approves the chosen network boundary. Never put the token in a URL. A custom MCP client supporting manually configured bearer headers can use this transport. This release does not implement OAuth login, dynamic registration, or automatic ChatGPT/Codex connector installation; clients requiring OAuth need a separate authorization integration.

For the optional owner-private ChatGPT Secure MCP Tunnel route, use the dedicated [stdio relay and setup guide](SECURE-TUNNEL.md). The manual bearer HTTP route above does not supply ChatGPT OAuth login.

## Tools and boundaries

| Tool | Required capabilities | Behavior |
| --- | --- | --- |
| `list_agents` | `agents:read` | Selected UUIDs only, minimal status/provenance |
| `send_message` | `messages:send` | Queue informational delivery; wakes an idle supported conversation when available, without creating a task or reply obligation |
| `followup_task` | `tasks:submit`, `agents:wake` | Durable task receipt; can start an off worker |
| `receive_messages` | `controller:receive`; `controller:ack` to acknowledge | This controller's opaque cursor-bound mailbox |
| `wait_agent` | `controller:wait` | Wait for this controller's mailbox activity |
| `interrupt_agent` | `agents:interrupt` | Interrupt current selected agent turn |

Selected managed workers can send information to the exact `controller:<uuid>` sender mailbox for unsolicited events. The receiver grant must remain active and include `controller:receive`; unselected workers and tasks addressed to controllers are rejected. Controllers cannot issue worker replies. Agents reply to the canonical request ID; native task completion is delivered through the existing durable outbox. An admission receipt is not evidence of provider visibility or completion. Interrupt is agent-level turn control, not request-specific cancellation or process termination.

Target arguments must be exact selected UUIDs; names, caller-supplied sender identity and unknown fields are rejected. Bodies are bounded to 1 MiB, literal messages to 64 KiB, receive counts to 100 and waits to 60 seconds. The gateway admits at most eight concurrent HTTP requests before starting authentication subprocesses. Each HTTP call authenticates its token against the authoritative grant before dispatch and before returning results; native dispatch checks the grant independently. Resource audience is checked at both boundaries. Host and Origin checks prevent forwarding or browser misuse. Audits record identity, operation and receipt outcome, never token or message content; unavailable auditing prevents dispatch. The operator is responsible for audit retention.

JSON-RPC IDs correlate responses and can restart when a client reconnects. Each new call gets a fresh native admission key by default. For explicit durable send/task replay, set `params._meta["wardian/request_key"]` to the same logical request string (1..200 UTF-8 bytes); the bridge hashes it into a controller-bound key. Reuse it only within the same operation and payload; canonical storage rejects changed payloads. Use a new request key for a new task even when its text is identical. Native stdio sessions have a separate connection namespace. No operation is automatically retried. Lost receipts are uncertain: inspect the mailbox rather than resubmit under a new request key. The gateway kills only its own timed-out CLI child; it cannot undo admitted tasks.

This capability system limits remote callers. It is not isolation against arbitrary processes with the same local OS account and write access to the Wardian home. Local grant-management environment guards prevent accidental agent use, not hostile same-user credential access.

## Validation

Run `npm test` in this directory for HTTP validation, revocation, selected-agent routing, bounded subprocess framing and request/receipt tests. Native controller tests exercise real canonical admission, delivery claims, worker replies, acknowledgement, durable completion and revocation in isolated test homes. Tests use fake credentials and transient loopback endpoints; installation does not activate access.

For the optional real CLI acceptance scenario, first build `wardian-cli`, set `WARDIAN_BRIDGE_E2E_CLI` to its absolute executable path and `WARDIAN_BRIDGE_E2E_DRIVER` to this directory's `native-e2e-driver.mjs`, then run:

```sh
cargo test -p Wardian --lib controller_task_worker_reply_and_ack_use_canonical_mailbox_without_agent -- --test-threads=1 --nocapture
```

The command works in PowerShell after setting the two variables with `$env:WARDIAN_BRIDGE_E2E_CLI` and `$env:WARDIAN_BRIDGE_E2E_DRIVER`. With the driver configured, the test must print `native_controller_http_roundtrip_passed` and `native_controller_stdio_roundtrip_passed`; without it the baseline test covers native mailbox behavior only. The bridge CI job configures the driver automatically. This scenario uses the actual built CLI and production handler, an isolated control endpoint, a transient loopback HTTP server, a direct MCP stdio client subprocess, and synthetic workers. It exercises HTTP task replay, stdio task submission and mailbox reply retrieval, acknowledgement, selected tool schemas, and denial of an unselected target. It also exercises the private-tunnel stdio relay in a real subprocess, repeated initialization and reused IDs, explicit durable task replay, out-of-scope denial, mailbox continuity and owner revocation on reconnect. The deterministic stdio client validates the Codex-compatible MCP protocol without invoking a model. It does not verify a real provider, public HTTPS routing, or a cloud client's connector installation.
