# Agent messaging tools

`wardian mcp serve` exposes Wardian peer messaging through model-facing tools.
It runs as a local subprocess and reads and writes newline-delimited JSON-RPC
through standard input and output. Protocol discovery works without launching
or modifying a Wardian home. Operations require the running app for the
server's `WARDIAN_HOME` and a managed caller identified by `WARDIAN_SESSION_ID`.

## Tool contract

The agent-facing surface follows the distinction between information and work:

| Tool | Meaning |
| --- | --- |
| `send_message` | Admit an informational message; automatically deliver it and wake an idle recipient when its original conversation is available. |
| `followup_task` | Assign work and return an admission receipt. Wardian resumes the original requester conversation with the terminal reply when a safe route is available. |
| `reply` | Complete the specified request as its authorized recipient. |
| `interrupt_agent` | Request interruption of the current turn while retaining the session, when supported. |
| `list_agents` | Discover Wardian peer names, UUIDs, providers, and status. |

Send and follow-up arguments are `target` and `message`. Targets are exact
agent names or UUIDs; ambiguous names and broadcasts fail. Message text retains
Unicode, line breaks, and punctuation. Receipts carry Wardian interaction or
request IDs and the actual delivery state. Admission, provider visibility,
task completion, and reply are different facts. Agents do not poll or receive
mailbox pages. The retired `message receive`, `message wait`, `receive_messages`,
and `wait_agent` interfaces fail before changing claims or acknowledgements;
the backend mailbox remains available for read-only diagnostics.

Reply takes `request_id`, `status` (`done`, `blocked`, or `failed`), and
`message`. The request determines the destination. A committed explicit reply
suppresses automatic final-result publication. Exact native task turns may
publish their terminal result automatically. Every canonical terminal reply
inherits the requester's durable provider conversation binding and is resumed
through that same conversation when a safe continuation route is available.

Only an assigned task with an explicit `request_id` may use MCP `reply`.
An ordinary human chat message completes through the agent's assistant
response. Its delivery interaction ID is not a task request ID. Informational
peer messages automatically wake an idle recipient through the same durable
delivery service, but create no task or reply obligation and do not trigger an
automatic reply or task result. Messages admitted before wake eligibility was
recorded remain context-only; migration never turns old stored FYIs into a
startup wake backlog. Off, pause, session replacement, and provider changes
remain authoritative fences.

An MCP tool error is returned with `isError: true`. A failed or lost send
response can leave delivery uncertain; neither the server nor the caller
should automatically replay it. Calls use bounded control exchanges. Timing
out at the client does not cancel an admitted message or task.

The app owns sender attribution, target identity, queue policy, and delivery
records. The tool does not accept an arbitrary sender argument. Informational
records never enter the task queue. Follow-up work uses the
receiver's supported delivery boundary. Interruption is separate from sending
a correction, pausing the agent, or destroying its provider process.

Native delivery must reach the same running provider conversation. An idle
surface route is available only when no native owner is selected and the
runtime can confirm a provider turn receipt. An uncertain native submission is
not retried through another transport.
An explicit user `off` setting, a busy conversation lease, or an unavailable
provider identity leaves a continuation pending for a later readiness
opportunity.

## Automatic task delivery

`followup_task` first persists the task and returns its request ID. A durable
claim selects one delivery owner. Startup and readiness observations recover
pending tasks and replies after the agent roster and provider lifecycle become
ready. They do not replay a task or continuation whose delivery is uncertain.

At follow-up admission, Wardian captures the managed sender's persisted
provider and `resume_session` identity together with the observed input
generation in the canonical task row. The reply transaction copies that origin
into the canonical reply row before any wake is scheduled. Conversation logging
and archive hydration are not required. A normal process restore may advance
runtime generation; recovery matches the exact provider conversation. A new
session, clear, or provider change is stale and blocked. A missing identity
waits for readiness; an old reply without a durable origin remains blocked as
`blocked_legacy_uncertain` rather than being guessed or replayed.

For an attached Codex session, Wardian validates its native owner and runtime
generation. An observed idle owner receives `turn/start` with a structured
`wardian_task_delivery` item on the existing thread. An active owner with an
exact turn ID receives `turn/steer`, fenced by `expectedTurnId`, with literal
peer text as input and separate host routing metadata in `additionalContext`.
Structured steering requires stable Codex 0.159.2 or newer; older supported
versions retain idle and information delivery. No terminal paste or Return key
is involved. Acceptance persists an exact request/claim/generation/thread/turn
binding before a completion observer can publish a result.

An exact completed turn returns its final assistant item as `done`. Commentary
is excluded. When a model omits message phases, the last completed assistant
item with an unknown phase supplies the compatibility result. An interrupted
turn returns an attributed Wardian `blocked` notice; a failed turn returns
`failed`. Missing final text or output exceeding the 64 KiB reply limit returns
an attributed `blocked` notice. Unknown outcomes and lost observations remain
uncertain and are never replayed. Several tasks accepted into the same active
turn each return its shared final result to their own requester.

A recognized stale-turn rejection or a writer-fence activity change confirms
that no task was admitted. Its claim is released before current activity is
read for another dispatch opportunity, so an idle callback consumed while the
claim was held cannot strand the task. Other provider errors or lost
acknowledgements retain uncertainty and cannot trigger that recovery.

Terminal evidence is persisted before reply publication. Startup recovers
already captured outcomes after a failed reply write without rerunning provider
work. Lost observation continuity remains uncertain; transcripts are never
used to infer or replay a task.

Replies resume through the existing provider boundary: the selected Codex
owner can start or steer the original thread; Pi and OpenCode use their
generation-bound bridges; other attached supported surfaces receive an idle
prompt only after a positive provider-turn receipt. Active, paused, deleted, or
identity-mismatched conversations remain fenced until a legitimate readiness
opportunity. The continuation keeps the canonical reply kind and parent task
correlation and creates no new task or reply obligation.

Older informational messages use Codex context-only injection or bounded
carry-forward into the next Wardian-managed task or reply turn. That carry path
does not wrap direct TUI user-started input. Newly admitted messages wake an
idle supported conversation when an exact provider receipt route is available;
active non-Codex turns retain the message for the next idle opportunity.

## Legacy delivery retirement

The agent-facing receive and mailbox-wait adapters are retired from the CLI and
MCP tool list. Old control requests are rejected before claim or acknowledgement.
Canonical mailbox records and read-only diagnostics remain in the backend.
Previously admitted replies without captured requester origin are observable
but never replayed into a guessed conversation.

Human terminal input remains available. Codex peer messaging prefers the
negotiated native owner. An idle surface fallback is allowed only when no native
owner is selected and the runtime can confirm a provider turn receipt; an
uncertain native submission never falls back to the composer. The updated
behavior requires the updated runtime; source changes do not alter an already
running older instance.

## Codex configuration

Managed Codex startup registers the bundled native CLI in the agent's private
provider home. Registration preserves user-owned entries, disabled state, and
approval settings. An occupied server name or missing CLI is reported instead
of silently replacing configuration. The app's global provider home is not
modified.

On Windows, managed startup registers `wardian-mcp.exe`, a GUI-subsystem
launcher that starts the console CLI with `CREATE_NO_WINDOW`. It passes the
MCP server's standard input, output, and error through unchanged; closing the
launcher also closes its server process. The ordinary `wardian-cli.exe`
remains console-backed for interactive CLI use.

For a separately configured managed Codex host, the registration command is:

```bash
codex mcp add wardian -- wardian mcp serve
```

The same command works in PowerShell. Configure the intended host's
`CODEX_HOME`. Supply its matching `WARDIAN_HOME` and managed
`WARDIAN_SESSION_ID` in the MCP server environment; registration does not
create an agent identity.

For a separately configured Windows host, use the silent launcher:

```powershell
codex mcp add wardian -- wardian-mcp mcp serve
```

Codex also accepts the equivalent configuration:

```toml
[mcp_servers.wardian]
command = "wardian"
args = ["mcp", "serve"]
tool_timeout_sec = 70
```

On Windows, set `command = "wardian-mcp"` and keep the same arguments.

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
An admitted task does not justify interrupting its recipient because the reply
has not appeared yet. Wardian resumes the original conversation when the
terminal reply is available.

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
informational delivery, task admission, correlated automatic reply continuation,
and interruption through the native harness. Compressed spelling is not a success
criterion.

## Native acceptance

The real suite uses two fresh managed Codex agents and a plain-English request.
The sender must assign work, the receiver must reply to that request, and
Wardian must resume the sender's original conversation with the correlated
reply. A matching final string alone is insufficient. The suite binds canonical
records to each agent's exact native rollout, model, turn IDs and provider receipts.
An existing empty Codex history database does not supersede the native rollout.
When Codex omits reasoning effort from a turn, the report records it as
unverified rather than inferring it from the requested configuration.

The canonical Wardian `ask_...` request identity remains distinct from the
provider-facing OpenCode `msg_...` message identity; both must remain bound in
delivery evidence.

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
tool grants, `WARDIAN_E2E_MESSAGING_V2_APPROVE_TOOLS=1` grants only the five named
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

## External controller ingress

`ControllerMessaging` is an authenticated capability port distinct from `MessageOrigin`. Its bearer grant names an exact HTTPS resource, selected agent UUIDs and operation scopes. The backend validates it independently of transport validation and preserves canonical admission, provider claims, replies and completion outbox behavior. Controller mailbox owners use `controller:<uuid>` and never become provider agents. Receive acknowledgement requires a separate capability; quiet waits recheck revocation. No schema migration or provider-session changes are required.

See the [external controller transport guide](https://github.com/wardian-app/Wardian/blob/main/extensions/selected-agent-bridge/CONTROLLERS.md) for activation, transport constraints and acceptance testing.
