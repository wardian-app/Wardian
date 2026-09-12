# Agent messaging tools

`wardian mcp serve` exposes Wardian peer messaging through model-facing tools.
It runs as a local subprocess and reads and writes newline-delimited JSON-RPC
through standard input and output. Protocol discovery works without launching
or modifying a Wardian home. Operations require the running app for the
server's `WARDIAN_HOME` and a managed caller identified by `WARDIAN_SESSION_ID`.

## Tool contract

The tool surface follows Codex v2's distinction between information and work:

| Tool | Meaning |
| --- | --- |
| `send_message` | Send information without starting or interrupting a turn. |
| `followup_task` | Assign work and return a request receipt without waiting for its reply. |
| `receive_messages` | Read a bounded batch of information, tasks, and replies addressed to the caller. |
| `reply` | Complete the specified request as its authorized recipient. |
| `interrupt_agent` | Request interruption of the current turn while retaining the session, when supported. |
| `list_agents` | Discover Wardian peer names, UUIDs, providers, and status. |

Send and follow-up arguments are `target` and `message`. Targets are exact
agent names or UUIDs; ambiguous names and broadcasts fail. Message text retains
Unicode, line breaks, and punctuation. Receipts carry Wardian interaction or
request IDs and the actual delivery state. Admission, provider visibility,
task completion, and reply are different facts.

Receive returns typed records and recipient-bound cursors. Pass `cursor` to
continue reading and `ack_cursor` to acknowledge a previously returned batch.
Replaying a cursor preserves record identities and does not execute work.
`limit` is bounded to 100 records and `timeout_ms` to 60 seconds. An empty wait
does not cancel tasks; later messages and replies remain available.

If native context arrives during a wait, receive can return promptly with
`wake_reason: "provider_context_available"`. That context is already on the
provider delivery path, so the tool does not return a duplicate body or
acknowledge its consumption. The timeout flag remains false.

Reply takes `request_id`, `status` (`done`, `blocked`, or `failed`), and
`message`. The request determines the destination. A generic message or an
agent's final prose does not, by itself, complete a correlated request.

Only an assigned task with an explicit `request_id` requires MCP `reply`.
An ordinary chat message or `wardian send` completes through the agent's
assistant response. Its delivery interaction ID is not a task request ID.

An MCP tool error is returned with `isError: true`. A failed or lost send
response can leave delivery uncertain; neither the server nor the caller
should automatically replay it. Calls use bounded control exchanges. Timing
out at the client does not cancel an admitted message or task.

The app owns sender attribution, target identity, queue policy, and delivery
records. The tool does not accept an arbitrary sender argument. Informational
records never enter the runnable prompt queue. Follow-up work uses the
receiver's supported delivery boundary. Interruption is separate from sending
a correction, pausing the agent, or destroying its provider process.

## Codex configuration

Managed Codex startup registers the bundled native CLI in the agent's private
provider home. Registration preserves user-owned entries, disabled state, and
approval settings. An occupied server name or missing CLI is reported instead
of silently replacing configuration. The app's global provider home is not
modified.

For a separately configured managed Codex host, the registration command is:

```bash
codex mcp add wardian -- wardian mcp serve
```

The same command works in PowerShell. Configure the intended host's
`CODEX_HOME`. Supply its matching `WARDIAN_HOME` and managed
`WARDIAN_SESSION_ID` in the MCP server environment; registration does not
create an agent identity.

Codex also accepts the equivalent configuration:

```toml
[mcp_servers.wardian]
command = "wardian"
args = ["mcp", "serve"]
tool_timeout_sec = 70
```

Codex can require approval for this tool because it sends a message. A
background session with `approval_policy = "never"` cannot display that
approval and may reject the call before Wardian receives it. When the operator
has authorized messaging for that server, Codex supports an explicit per-tool
setting:

```toml
[mcp_servers.wardian.tools.send_message]
approval_mode = "approve"
```

Scope approval to the intended tools and agent or test host. The example
authorizes only `send_message`; other tools retain their own approval policy.
Registration does not grant approval or mark messaging as read-only.

This interface is available to MCP-capable interactive and background clients.
It does not replace Codex's built-in handlers or spawn peer identities.
Receiver tools and provider push share the same interaction records; a tool
being present does not establish unsolicited delivery support for every
provider version or terminal. See
[native orchestration delivery](./native-orchestration-delivery.md) for the
provider transport boundary.

The Codex v2 runtime uses one Wardian-owned app-server for a provider session.
The local-daemon integration targets stable Codex CLI `0.154.0` or later,
with an explicit compatibility exception for the tested `0.154.0-alpha.6`.
A newer version must still pass connection, input and attachment checks;
version eligibility alone is not evidence of successful delivery.
The original Codex TUI uses ordinary local-daemon discovery under the same
managed home. Wardian observes that TUI's selected thread in the initially
unloaded daemon before subscribing and enabling delivery. A silent embedded
fallback cannot satisfy this check. Wardian delivers information, work and
interruption through a private socket and `app-server proxy` WebSocket tunnel.
MCP provides the operations the model calls; composer pastes do
not deliver v2 peer messages. An existing embedded terminal session must be
explicitly restarted to adopt this runtime. Restart does not authorize
replaying an uncertain message.

During resume, the starting agent temporarily shows its terminal even when its
saved card mode is chat. This keeps native trust and login prompts accessible
while attachment is pending. It does not approve those prompts or enable peer
delivery early. Successful attachment restores the saved card mode; failed
startup restores the paused presentation.

Interactive startup temporarily projects the agent's launch settings into its
private config so the TUI and daemon agree. After attachment checks, Wardian
restores unchanged projected values while preserving user edits. An interrupted
projection is recovered on the next managed preparation; removed settings must
not remain enabled merely because an earlier launch used them. Background
launches use their CLI settings without creating this projection.

An accepted task for an inactive Codex agent may wait while Codex indexes its
existing history. This background initialization has no fixed response deadline;
individual connection attempts and protocol writes remain bounded. Cancelling
owner preparation, losing its conversation lease, or disposing the owner cancels startup and joins its
child before releasing ownership. Interactive startup keeps its existing deadline.
This behavior does not establish a startup performance target.

A definite background failure before task submission produces a correlated
`failed` reply explicitly attributed to Wardian. The task and failure notice
commit together. An accepted or uncertain submission cannot take this path.
An empty receive wait neither cancels the task nor justifies interrupting its
recipient; continue waiting within the caller's deadline.

## Behavioral experiment

The initial experiment used a v1-shaped `send_input` adapter and a fresh Astra
session. Its plain-English task contained no example message, tool-name
instruction, or compression request. Astra discovered the tool and delivered
a real message, using ordinary prose. That historical result does not prove
the new v2 sender/receiver contract.

The private report records exact input, exposed schema, visible output, model,
artifact hashes, and correlated delivery evidence. Authentication is never
included in the report. Character and word counts are labeled explicitly;
they are not token counts. Writing style and semantic completeness are assessed
separately from successful delivery. One trial is feasibility evidence, not a
general claim about model training or preferred language.

The initial experiment disabled built-in subagents, so it did not compare
preference between competing delegation systems. V2 acceptance instead checks
non-waking information, task admission, receiving, correlated replies, and
interruption through the native harness. Compressed spelling is not a success
criterion.

## Native acceptance

The real suite uses two fresh managed Codex agents and a plain-English request.
The sender must actually assign work, the receiver must reply to that request,
and the sender must consume the correlated reply before answering. A matching
final string alone is insufficient. The suite binds the canonical interaction
records to each agent's exact native rollout, model, turn IDs and tool receipts.
An existing empty Codex history database does not supersede the native rollout.
When Codex omits reasoning effort from a turn, the report records it as
unverified rather than inferring it from the requested configuration.

Run against a frozen packaged application and its matching CLI:

```bash
WARDIAN_E2E_REAL_MESSAGING_V2=1 WARDIAN_NATIVE_SKIP_BUILD=1 \
WARDIAN_NATIVE_APP='<absolute-packaged-app-path>' \
WARDIAN_E2E_MESSAGING_CLI='<absolute-matching-cli-path>' \
WARDIAN_E2E_CODEX_AUTH_HOME='<absolute-authorized-codex-home>' \
WARDIAN_E2E_CODEX_EXPECTED_VERSION=0.154.0-alpha.6 \
WARDIAN_E2E_CODEX_EXECUTABLE='<absolute-stock-codex-executable-path>' \
WARDIAN_E2E_MESSAGING_V2_MODE=attached_tui \
WARDIAN_E2E_MESSAGING_V2_LIFECYCLE=1 \
node scripts/run-native-e2e.mjs e2e-native/tests/agent-messaging-v2-real-native.test.mjs
```

In PowerShell, set the same environment variables with `$env:NAME = 'value'`
before running the same `node scripts/run-native-e2e.mjs` command. The runner
owns the isolated home lock and the Windows child-process Job Object; direct
`node --test` invocation is only for the inert contract tests, with real-provider
opt-in unset. Put the selected stock installation first on the test process's
`PATH`, and include its matching official helper executables. For a loose
Windows alpha6 installation with code mode enabled, the separately published
`codex-code-mode-host.exe` must sit beside `codex.exe`; the main release ZIP
alone does not contain it. These are unmodified upstream release components.
If the fixture requires explicit
tool grants, `WARDIAN_E2E_MESSAGING_V2_APPROVE_TOOLS=1` grants only the six named
tools in the two private, normally registered agent homes.

Each fixture workspace has its own empty Git repository so Codex cannot inherit
the checkout's repository root. Normal acceptance enables workspace trust only
in the isolated test home's runtime settings; the two generated launch policies
target those fixture roots. Trust projection is temporary and is restored after
attachment. Set `WARDIAN_E2E_MESSAGING_V2_TRUST_FIXTURE_WORKSPACES=0` only for an
untrusted-startup diagnostic; the harness does not answer native trust prompts.

Attached mode also requires both original terminals to remain on the same
owned provider sessions and display exchange activity. Lifecycle checks prove
that idle information does not start a turn and that one interrupt affects the
exact observed active turn. Background mode, selected with
`WARDIAN_E2E_MESSAGING_V2_MODE=background` and the lifecycle flag unset, checks a
later task after owner exit: a new generation must resume the same conversation
and recall the prior result without being given that result again.
