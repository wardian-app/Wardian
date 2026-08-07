# Prime Agent Provider

- **Status:** Accepted, partially implemented
- **Date:** 2026-08-05
- **Verified against:** prime-agent 0.7.0, Windows 11, Node 24.13.1, npm 11.10.1

Findings marked *(verified)* were observed directly against a running
prime-agent on this platform: the phase 0 spike, live RPC round trips, and
reads of the installed package's own source. Everything else is from the
upstream docs. Several sections marked *(corrected)* record where the original
design was wrong and what the evidence showed instead; they are kept rather
than quietly rewritten because each one cost a real bug.

See *What is not built* under Implementation phases for the current state.

## Problem

Wardian supports five agent CLIs (Claude, Codex, Gemini, Antigravity, OpenCode).
All five share two properties that shape the current provider layer:

1. They are driven interactively by writing keystrokes into a PTY, so every
   provider needs a hand-tuned `DeliveryProfile` (submit key, bracketed-paste
   threshold, settle delay, input-ready and busy markers).
2. The agent process dies with its PTY, so PTY liveness is a valid proxy for
   agent liveness.

[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent) violates both.
It exposes a bidirectional JSONL command channel (`--mode rpc`) that replaces
keystroke emulation entirely, and it runs each root session tree in a detached
daemon worker that survives client disconnect. It is also the first candidate
provider with first-class recursive subagents, which Wardian currently has no
way to display.

Adding it therefore is not only a new `AgentProvider` implementation. It
requires a lifecycle shape the provider layer has not needed before, and it
opens a nested-agent surface the UI has never had a provider able to populate.

This spec defines the provider contract, the lifecycle changes, and the depth
of feature combination, in landable phases.

## Background: what Prime Agent is

An MIT-licensed fork of `earendil-works/pi` (pi-mono). A TypeScript host drives
a persistent IPython kernel, which is the **only** model-facing tool. File
operations, shell commands, skills, MCP integrations, and subagent delegation
all happen as Python inside that kernel.

Relevant properties, from `packages/coding-agent/docs/` in the upstream repo:

| Property | Detail |
|---|---|
| Meta-provider | Selects its own backend: anthropic, openai, google, bedrock, vertex, azure, mistral, cloudflare, copilot |
| Modes | interactive TUI, `-p/--print`, `--mode json`, `--mode rpc`, plus `acp` and `daemon` *(verified)* |
| Distribution | npm global tarball; the installer downloads a checksum-verified `.tgz` and runs `npm install -g` *(verified)* |
| Sessions | Flat append-only JSONL under `~/.prime/agent/sessions/`, overridable with `--session-dir` |
| Context files | Native `AGENTS.md`; also reads `CLAUDE.md`; global at `~/.prime/agent/AGENTS.md` |
| Subagents | `await rlm(...)` inside the kernel; each child gets its own model, kernel, and session tree |
| Daemon | Detached supervisor, one worker process per root session tree; workers outlive clients |
| A2A | Sessions message each other, restricted to parent/sibling/child within one root tree |
| Config dir | `~/.prime/agent`, overridable with `PRIME_AGENT_CODING_AGENT_DIR` |
| Windows | Requires bash; probes `~/.prime/agent/settings.json` → `C:\Program Files\Git\bin\bash.exe` → PATH |
| Runtime dep | Bootstraps an IPython kernel venv at `~/.prime/agent/kernel-venv`, or uses `PRIME_AGENT_KERNEL_PYTHON` |

Prime Agent has no permission-prompt or sandbox layer; it executes
model-generated Python with the user's permissions. This matches the existing
OpenCode precedent, where `OpenCodeProviderConfig` also carries no safety
knobs. It is a property of the provider the user selects, not a gap Wardian
needs to close.

## Decision

### Provider identity

Provider id `prime`, display name `Prime Agent`, instruction file `AGENTS.md`.

`prime` is a meta-provider: `AgentConfig.model` carries a composite
`provider/model[:thinking]` value (for example `anthropic/claude-opus-5:high`)
rather than a bare model id. The model catalog must preserve that composite in
`ProviderModelOption.id`.

### Data schema

Add to `crates/wardian-core/src/models/agent_config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PrimeProviderConfig {
    /// off | minimal | low | medium | high | xhigh | max
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Allowlist for `--tools`; `ipython` is the only built-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_builtin_tools: Option<bool>,
    /// Repeatable `-e/--extension` sources (path, npm, or git).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// Repeatable `--skill` paths, beyond habitat discovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// Persistent objective for a new root session (`--goal`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autonomous: Option<bool>,
    /// Repeatable `--autonomous-gate` shell commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub autonomous_gates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autonomous_max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autonomous_max_tokens: Option<u64>,
    /// Prime's short daemon id for the worker, distinct from the session UUID.
    /// Informational, not required to stop the agent; see Lifecycle below.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_agent_id: Option<String>,
}
```

Register `ProviderConfig::Prime(PrimeProviderConfig)`, extend `type_name()` to
return `"prime"`, and add a `prime_config()` accessor matching the existing
per-provider accessors.

### Event mapping

Prime's `--mode json` stream maps onto `AgentEvent` without marker scraping.
This is the cleanest mapping of any provider Wardian supports:

The first stream line is *(verified)*:

```json
{"type":"session","version":3,"id":"019fd48e-…","timestamp":"2026-08-06T00:51:47.789Z","cwd":"…","rlmDepth":0}
```

`rlmDepth` is the root-versus-subagent discriminator: `0` is a root session,
deeper values are RLM descendants. Phase 4 depends on it.

| Prime JSON line | `AgentEvent` |
|---|---|
| `{"type":"session","id":"<uuid>","rlmDepth":0,…}` (first line) | `Init { session_id, timestamp }` |
| `turn_start` | `UserQuery` |
| `message_start`, `message_update`, `message_end` | `Generating` |
| `tool_execution_start`, `tool_execution_update` | `Generating` |
| `tool_execution_end` | `Generating` |
| `turn_end` | `Generating` (further turns may follow) |
| `agent_end` | `TurnCompleted` |
| `compaction_start` / `compaction_end` | `Generating` / `Generating` |
| `auto_retry_start` | `Generating` |
| everything else | `Unknown` |

`ActionRequired` has one source: an extension UI request. Prime has no
permission prompts, so nothing else in the stream represents the agent waiting
on a person. `pty_status_event_policy_for_provider` keeps
`ProviderStatusEventPolicy::Normal` for `prime`.

The method decides, not the event type. `select`, `confirm`, and `input` block
on a reply, while `notify`, `setStatus`, and the widget methods are
fire-and-forget in `rpc-extension-ui-context.js` and arrive as the same
`extension_ui_request` type. Mapping all of them to `ActionRequired` would
leave a working agent showing amber with nothing for the user to answer, so
`parse_output` maps only the blocking three and passes the dialog's own title
or message through as the event text.

Two shape details confirmed in the spike that the mapping must tolerate:

- `message_start` / `message_end` fire for `user` and `toolResult` roles as
  well as `assistant`, so role must be inspected rather than assumed.
- Every assistant message carries
  `usage: { input, output, cacheRead, cacheWrite, totalTokens, cost }`.
  Telemetry can read token and cost accounting straight off the stream instead
  of estimating it, which no other provider allows.

`Init` arrives on the first stream line, so `prime` needs no session-id
bootstrap handshake. `provider_needs_bootstrap_session` and the
`matches!(provider, "codex" | "opencode" | "antigravity")` pre-bound-identity
guard in `manager/spawn.rs:334` must both exclude `prime`.

### Session identity and workspace placement

Wardian pins each agent's session storage into its own workspace:

```
--session-dir  ~/.wardian/agents/<UUID>/sessions
```

This makes transcripts inspectable on disk without a live provider process,
satisfies the Markdown-as-Truth principle as far as a JSONL format allows, and
removes the need to discover a provider-owned session directory. Resume uses
`-r <session-uuid>`; `--fork <id>` backs Wardian's clone action.

Because the directory is Wardian-owned and per-agent, session discovery can
fall back to "newest JSONL in the agent's session dir" if the `Init` line is
ever missed. That fallback is not merely defensive: **the session header `id`
is not the session file name** *(verified)*. A run whose header id was
`019fd4aa-dfbc-…` wrote `019fd4aa-d94b-….jsonl`. Never derive one from the
other.

`--resume` does accept the header id and resolves it to the right file, and the
header id is stable across resume *(verified)*, so it is the correct value to
persist in `resume_session`.

**Resume can transiently fail while the previous worker's lease is still held**
*(verified)*. Immediately re-running against a just-completed one-shot session
produced:

```text
Error: Session is already active in 3a87eadc7fe1: …\019fd4aa-d94b-….jsonl
```

At that moment `prime-agent stop 3a87eadc7fe1` reported
`Unknown active session`, so the lease outlived the worker that owned it. The
same resume succeeded on a later attempt. Wardian's resume path must therefore
treat `session_already_active` as retryable with backoff rather than a hard
failure, and must not assume the agent id named in that error is still
stoppable.

`core/session-lease.js` shows what that retry is actually waiting for, and it
is not a timer. `isLeaseOwnerAlive` compares the recorded pid's liveness and
its process start id, and `reclaimStaleLease` takes the lease over as soon as
that check fails. There is no timed grace period to sit out: the window is only
as long as the previous worker takes to exit. An earlier revision of this spec
attributed the recovery to a grace period, which would have argued for a much
longer backoff than the evidence supports.

`PrimeProvider::SESSION_LEASE_RETRY_BACKOFF` is therefore short and strictly
increasing (250ms, 750ms, 2s), applied in
`delivery/headless_process.rs::run_with_session_lease_retries`. Only Prime gets
a schedule, only a recognized lease conflict is retried, and the retries share
a single delivery-attempt row so one prompt still records one outcome. Giving
up quickly is deliberate: a lease held by a worker that is genuinely still
running is a real conflict the user should see, not something to hide behind a
long wait.

Both message forms must be recognized, since the owner is not always named:

```text
Session is already active in <agent>: <path>
Session is already active in another process: <path>
```

### Stop selectors and worker ownership *(verified)*

An earlier revision of this spec assumed `prime-agent stop` needed Prime's short
daemon id, and therefore that Wardian had to capture `daemon_agent_id` before it
could tear an agent down. Reading the supervisor shows otherwise.
`matchWorkers` in `dist/modes/daemon/daemon-supervisor.js` accepts the daemon
id, the session UUID, or the session name as an exact selector, falling back to
a suffix match on either id. Wardian already persists the session UUID in
`resume_session`, so teardown needs no additional lookup and
`daemon_agent_id` is informational only.

The same code establishes which workers a `stop` client can even see.
`createDaemonClientConnection` is called with `clientOwned: parsed.noSession`
for the interactive TUI and `clientOwned: true` for `--print` and `--mode rpc`.
A `client_owned` worker records an `ownerClientId`, and
`isWorkerAccessibleToClient` hides it from every other client; a `resident`
worker has no owner and is visible to all. Two consequences follow:

- Wardian's interactive PTY spawn produces a **resident** worker. That is the
  one that outlives its client, and it is exactly the case `stop` handles.
- `--print` and `--mode rpc` workers are **client-owned** and invisible to a
  separate `stop` process, but they are also torn down with their client, so
  they need no external stop. This is the mechanism behind the open
  verification gap recorded at the end of this spec: those two modes cannot
  exercise the `stop` path even in principle.

### Input delivery: RPC, not keystrokes

`prime` must not be driven through `utils/terminal_input.rs` or a tuned
`DeliveryProfile`. RPC mode supplies the equivalents directly:

| Wardian action | RPC command |
|---|---|
| Send a chat message | `{"type":"prompt","message":…,"streamingBehavior":…}` |
| Interrupt | `{"type":"abort"}` |
| Change model | `{"type":"set_model","provider":…,"modelId":…}` |
| Change effort | `{"type":"set_thinking_level","level":…}` |
| Compact context | `{"type":"compact"}` |
| Watch a subagent | `{"type":"observe","activeSessionId":…}` |
| Answer a dialog | `{"type":"extension_ui_response","id":…,…}` |

#### One prompt command, always scheduled *(verified)*

An earlier revision of this table listed `steer` and `follow_up` as separate
Wardian actions, chosen by first reading whether the session was streaming.
That is wrong in a way a live run exposes immediately. Sending a bare `prompt`
to a busy session is rejected:

```text
Agent is already processing. Specify streamingBehavior ('steer' or 'followUp')
to queue the message.
```

Reading `_promptInjectedMessage` explains it: the gate only fires when work is
already queued, and `streamingBehavior` is otherwise unused. So a single
`prompt` carrying the field covers both states, while a check-then-send design
races -- the session can start streaming between the check and the write.
`prompt_delivery` therefore never consults a streaming flag; it maps
`QueuePolicy` alone:

| Queue policy | `streamingBehavior` |
|---|---|
| `LiveOnly` | `steer` |
| `QueueIfBusy` | `followUp` |
| `MailboxOnly` | withheld; not an RPC send |

`MessageInputMode::ApprovalAction` forces `steer` regardless, since the agent
is blocked on that answer and deferring it would deadlock the turn.

Note the spelling: the standalone command is `follow_up`, but the scheduling
field is `followUp`. Two sequential prompts carrying the field were verified
live to both acknowledge `success: true` and run in order, where the earlier
design produced a hard failure on one of them.

#### Framing *(corrected)*

An earlier revision claimed the delimiter rule came from Node `readline` being
non-compliant on `U+2028`/`U+2029`. Prime does not use `readline`. Its
`attachJsonlLineReader` splits on `\n` and strips one trailing `\r`, so CRLF is
tolerated on input and `JsonlFramer` mirrors exactly that. The real constraint
is on the writing side: a command must occupy one physical line, so
`encode_command` rejects any payload containing a raw newline rather than
trusting `serde_json` to have escaped it.

Register a conservative fallback `DeliveryProfile` for the interactive TUI path
so `delivery_profile("prime")` is never the unknown-provider default, but do
not rely on it for delivery receipts.

### Lifecycle: detached workers

This is the substantive departure from every existing provider.

```mermaid
flowchart TD
    wardian["Wardian backend<br/>owns root lifecycle"]
    supervisor["Prime supervisor<br/>detached, survives client exit"]
    worker["Worker process<br/>root AgentSession + RLM descendants"]
    child1["RLM child"]
    child2["RLM child"]

    wardian -->|"spawn / stop &lt;agent&gt;"| supervisor
    wardian -.->|"rpc stdin/stdout"| worker
    supervisor --> worker
    worker --> child1
    worker --> child2
```

Ownership split: **Wardian owns the root** (spawn, stop, workspace, junctions,
identity, telemetry attribution). **Prime's daemon owns the subtree** (RLM
descendants, kernels, scheduling, worker recovery). Wardian must not attempt to
supervise or reap prime's worker processes.

Three consequences:

1. **Kill must call `prime-agent stop <agent>`.** Closing the PTY only detaches
   the client; the worker keeps running and keeps spending tokens. Wardian's
   stop path must invoke the CLI and only then tear down the PTY.
   `prime-agent shutdown` is global and must never be used for a single agent.
   The selector may be the persisted `resume_session` UUID; see *Stop selectors
   and worker ownership* above.
2. **A new status is required.** Prime agents can be *running but detached*.
   The existing status vocabulary (Idle, Processing, Action Required, Off,
   Error) has no cell for "alive, not attached to this app instance".

   Implemented as `Detached`, carried through `AgentDisplayStatus` with its own
   `--color-wardian-detached` token. Two details are deliberate. It outranks
   the persisted off flag in `deriveEffectiveStatus`, because an agent that is
   genuinely burning tokens must not be displayed as off. And its indicator
   glows without pulsing, unlike Headless: nothing in this window is streaming
   from that worker, so an animated indicator would promise live output that
   has no source.
3. **Startup reconciliation.** On app launch, `prime-agent list --all --json`
   must be reconciled against Wardian's persisted agents. Without this, a
   Wardian restart silently loses track of live agents. No other provider needs
   this; the closest precedent is Antigravity conversation recovery.

   Implemented as `reconcile_prime_detached_agents`, ordered immediately after
   `reconcile_headless_agents` so it only ever upgrades the `Off` that pass
   writes for an agent with no live process. The join key is the persisted
   `resume_session` against each row's `sessionId`, which is why that field is
   parsed; `matches_session` also accepts the short daemon id for agents bound
   before `sessionId` was read. Three constraints shape the pass:

   - It reads `settings/state.json` rather than the database, because
     `resume_session` is not an `AgentRow` column.
   - It calls `list --json`, **not** `list --all --json`. `--all` also returns
     saved sessions read back from disk, and those rows still report
     `lifecycle: "live"` with `attachedClients: 0` long after their worker
     exited. An early revision used `--all` and would have marked every
     finished Prime session as a running one, then left it inert on restore.
     `is_hosted_by_daemon` guards the same mistake at the parsing layer:
     only a session the supervisor actually hosts carries `activeSessionId`.
   - It returns early when no persisted agent uses Prime. Launching the CLI
     unconditionally would start Prime's daemon on every Wardian start, for
     users who have never used the provider.
   - Only `rlmDepth == 0` rows are adopted. An RLM descendant is a projection
     of a root tree, so adopting one would create a duplicate agent.

   The restore loop then treats `Detached` like `Headless`: the agent is
   restored inert, with no spawn. Spawning a client for a session a worker
   already holds would either lose the lease race or start a second worker.

`list --all --json` is a richer reconciliation source than anticipated
*(verified)*:

```json
{"sessions":[{
  "id":"019fd48f-8b5b-76cd-9f8b-6e65660fc3ea",
  "lifecycle":"live", "activity":"idle",
  "isSessionActive":false, "isStreaming":false, "isCompacting":false,
  "sessionFile":"…\\sessions\\019fd48f-….jsonl", "cwd":"…",
  "attachedClients":0, "messageCount":4, "unfinishedActionCount":0,
  "sessionActions":{"queuedCount":0,"steering":[],"followUps":[]},
  "created":"…","modified":"…","lastActivityAt":"…","rlmDepth":0
}]}
```

**The supervisor is machine-wide, and Wardian's kill is not** *(verified)*.
Prime runs one supervisor for every root tree, on a single named pipe
(`\\.\pipe\prime-agent-daemon`, one pid, confirmed by `shutdown` stopping
exactly one background service). Wardian's `terminate_active_agent_process`
force-kills the process tree and then drops a Job Object with
`KILL_ON_JOB_CLOSE` (`manager/mod.rs:150`).

**Measured result** *(verified)*: the supervisor is a live descendant of
whichever client first started it, so the tree kill alone is sufficient to
destroy it. Observed parent chain while the supervisor served three sessions:

```text
79092  node.exe  cli.js --mode daemon   <- supervisor, 3 sessions
  71612  node.exe                       <- the prime-agent client that started it
    67456  pwsh.exe                     <- a user terminal, not Wardian
```

The blast radius therefore includes sessions Wardian does not own. In the
observed case the supervisor had been started from the user's own terminal; had
Wardian started it instead, killing that one Wardian agent would have taken
down the user's sessions.

Wardian's response, implemented in `manager/mod.rs`:

1. `provider_forbids_process_tree_kill("prime")` suppresses
   `force_kill_process_tree` for Prime agents. The client is an ordinary PTY
   child and dies on its own.
2. The job object is released through
   `utils::process::release_job_without_killing`, which clears
   `KILL_ON_JOB_CLOSE` before the handle drops, so the safety net cannot reap
   the shared daemon either.
3. `request_prime_worker_stop` dispatches `prime-agent stop <selector> --json`
   before teardown, targeting `daemon_agent_id` when known and the persisted
   session UUID otherwise -- both are valid selectors, so the fallback is not a
   degraded path. It is fire-and-forget: the request reaches the
   supervisor over its own socket and does not depend on the client being torn
   down.

Still unverified: `stop` succeeding against a live resident worker spawned by
Wardian itself. RPC and print clients are client-owned, so the supervisor hides
their workers from any other client and removes them on normal completion --
they cannot exercise that path even in principle. Producing a resident worker
requires the interactive PTY spawn, which puts this in the native E2E layer.

`e2e-native/tests/prime-detached-worker-native.test.mjs` is that test, gated on
`WARDIAN_E2E_REAL_PRIME=1` per the real-provider rule. It does not pass yet: on
Windows 11 with Edge WebView2 151.0.4129.59 the WebView tab crashes about 50
seconds into the `spawn_agent` invoke, before the backend logs the command, so
the assertions have never been reached. Harness fragility and a real fault in
spawning Prime through the app are both still live explanations.

What those runs did establish, because the daemon was inspected after each one:
no Prime worker leaked, and both the shared supervisor and the developer's own
sessions survived every attempt including the forcibly interrupted ones. That
is weaker than the assertion the test is meant to make, and it is not a
substitute for it.

`activity`, `isStreaming`, and `attachedClients` map directly onto Wardian's
status vocabulary plus the new detached state, and `sessionActions` exposes
queue depth that Wardian otherwise has to infer. `stop <agent> --json` is the
matching mutation.

Note that `prime-agent status` reports supervisor state and is separate from
`list`: a completed one-shot JSON-mode client leaves no background service but
can still leave a catalog entry *(verified)*. Reconciliation must key off
`list`, not `status`.

### Executable resolution

The shell installer is a wrapper around `npm install -g` of a checksum-verified
release tarball *(verified)*. On Windows this produces an ordinary npm shim set
in the npm global prefix:

```
%APPDATA%\npm\prime-agent.cmd  →  node_modules\prime-agent\dist\bundle\cli.js
```

`get_executable()` therefore follows the existing Claude and Codex pattern and
reuses `providers/npm.rs::node_launch_from_npm_cmd_shim`, which parses exactly
this shim format. No bespoke installer-path probing is needed.

### Readiness

`provider_readiness` currently resolves the executable and returns. That is
insufficient for `prime`, and the obvious probe is the wrong one:
`prime-agent doctor` inspects **background services only** and reports
"No background services found" on a healthy fresh install *(verified)*. It says
nothing about the IPython kernel.

The kernel is the real gate, because it is prime's only tool. A resolvable
binary with a broken kernel produces an agent that answers every request with a
runtime-setup failure.

**On Windows, prime-agent 0.7.0's kernel auto-bootstrap is broken** *(verified)*.
It invokes `uv pip install --python <kernel-venv>/bin/python`, the POSIX venv
layout, while `uv venv` on Windows produces `Scripts\python.exe`. The install
fails with exit code 2 and the tool is unusable. This is independent of network
availability.

The supported workaround is `PRIME_AGENT_KERNEL_PYTHON`, verified working
end to end: with a correctly laid-out venv carrying `ipykernel`, the bundled
`dist/prime-agent-runtime`, and the default package set, `ipython` executes and
returns `{"status":"ok","stdout":…}`.

Wardian therefore:

1. Discovers a kernel venv at `<WARDIAN_HOME>/prime-kernel-venv` and exports
   `PRIME_AGENT_KERNEL_PYTHON` into the spawn environment, rather than relying
   on prime's auto-bootstrap.
2. Treats a missing kernel as a readiness blocker, reported in
   `ProviderReadiness.reason` distinctly from a missing binary.

Revisit item 1 when upstream fixes the venv layout; the env var remains the
documented escape hatch either way.

#### *(corrected)* Discovery is built, provisioning is not

Item 1 originally said Wardian **provisions** the venv on first use. It does
not, and there is no code that ever did. It discovers one and exports it; the
user builds it. The readiness message says so and names the directory the
running build reads, because a debug build's Wardian home is under `target/`
rather than `~/.wardian` and a `<WARDIAN_HOME>` placeholder sent people to
build an environment the app would never look at.

Two defects made the gap worse than a missing convenience:

- **The export was missing.** `apply_interactive_provider_runtime_env` and its
  process-mode twin handled only Claude, so a discovered interpreter never
  reached the child. `<WARDIAN_HOME>/prime-kernel-venv` cleared the readiness
  gate and then failed every tool call. Only `PRIME_AGENT_KERNEL_PYTHON` worked,
  and only because the child inherits the app's environment.
- **The UI called an installed provider "not installed."** Every unavailable
  provider got that suffix regardless of reason. Readiness now keeps the
  resolved `executable` when a runtime dependency is the blocker, and the
  frontend reads its presence as "needs setup" instead.

Item 2's live `ipython` probe was also dropped. Readiness populates the provider
list on every render, so it is a filesystem check; `providers/readiness.rs`
records why.

#### Provisioning, and why it is not a button

Provisioning is now built, in `providers/prime_kernel.rs`. Two decisions shaped
it.

**It runs at app startup, not at spawn and not from a settings button.** Spawn
was the obvious hook — user-initiated, once, already slow — but it makes the
first Prime launch sit for a minute with nothing to show, and the terminal pane
cannot help: the PTY does not exist yet, and the session broker rejects output
without a runtime generation. Startup has no such deadline. By the time anyone
opens the provider list the kernel is usually there, and if it is not, the
existing `ProviderReadiness.reason` carries "Wardian is setting up Prime Agent's
Python kernel" through surfaces that already render it. No new UI, no frontend
change. `ensure_provider_available_for_launch` waits on an in-flight run so a
fast user is not told to set up something Wardian is already building.

Readiness itself never starts or waits for the work. It runs on every
provider-list paint, so it only reads the state.

**Nothing half-built is ever discoverable.** `kernel_python` decides a kernel
exists by looking for the interpreter file, so a venv created before its
packages land would clear the readiness gate and then fail every tool call —
the precise failure the gate exists to prevent. The provisioner builds into a
`.staging` sibling, runs `import ipykernel, rlm` in the staged interpreter, and
only then renames it into place. `uv` exiting zero is not accepted as proof;
verified against a bare venv, which fails with `ModuleNotFoundError`. Losing the
rename to a concurrent run counts as success. Staging is removed on every exit
path.

The runtime package is **not on PyPI** — it is a source directory inside the npm
package, found by walking up from Prime's CLI entry point to the nearest
ancestor named `prime-agent` and then to `dist/prime-agent-runtime`. Deriving it
from the entry point rather than `npm root -g` guarantees it matches the install
that will actually run.

Measured on a warm `uv` cache: 4.2s, 81 MB, staging cleaned up, kernel
discoverable afterwards. A cold cache is nearer a minute.

**Not covered by an automated test at the native layer.** The native harness
exports `PRIME_AGENT_KERNEL_PYTHON` before launching the app, which
short-circuits the discovery path the provisioner populates. The gap is marked
with a `test.skip` in `prime-detached-worker-native.test.mjs`. The staging,
verification and publish steps are unit-tested.

### Scheduling

Prime persists per-session cron in `session-artifacts/<session-id>/scheduled-jobs.json`
and supports heartbeats. Wardian has `workflow/schedule.rs`.

**Wardian's scheduler stays authoritative** — it is cross-provider and already
integrated with the workflow engine. Prime's is not disabled (it is reachable
from inside the kernel regardless), but Wardian surfaces `list_schedules` and
`list_heartbeats` **read-only** so agent-created schedules running against a
Wardian-managed workspace are visible rather than invisible. Provider-side cron
that Wardian cannot see is a governance problem; provider-side cron that
Wardian displays is a feature.

`parse_schedule_list_output` reads `prime-agent schedule list --all --json`
into `PrimeScheduledJob`, with `belongs_to_session` matching a job to a Wardian
agent the same way session reconciliation does. `--all` is wanted here, unlike
the session listing: a paused job is exactly what a user needs to see, whereas
a saved session is noise. An empty listing is `{"jobs":[]}` and is not an
error; a payload with no `jobs` array is.

### Autonomous gates

Wardian already knows each project's verification commands from `AGENTS.md`.
Workflow nodes bound to `prime` emit them as gates:

```
--autonomous --autonomous-gate "npm run lint" --autonomous-gate "cargo clippy"
```

A failed gate feeds bounded command output into the next continuation so the
agent can repair it, and a passing gate permits completion even when a turn or
token limit has been reached. This makes Wardian's verification-first principle
provider-enforced for this provider rather than conventional.

`prime_gates::gates_from_agents_md` does the extraction, and the hard part is
that a checklist is prose with inline code in it. Most backticked spans are
filenames, flags, or commit-message examples, so three filters apply:

- **Section scope.** Only headings naming a pre-commit checklist or
  verification are scanned. A deeper heading subdivides the section; a heading
  at the same or shallower level ends it. Gating a turn on a setup or
  troubleshooting command the checklist never asked for would be a surprise.
- **Known runner.** A span must start with a recognized runner (`npm`,
  `cargo`, `pytest`, …) and carry an argument. This is what separates
  `npm run lint` from `AGENTS.md`, `--json`, and `feat(workflows): …`, and it
  correctly drops `git status`, which is not a verification command.
- **No shell operators.** Each gate is one argument, so a compound would be
  mangled or would quietly run a second command.

Output is capped at `MAX_GATES` because every gate runs at the end of every
turn. Run against this repository's own `AGENTS.md`, the extractor yields
exactly `npm run lint`, `npm run test`, `npm run build`, `cargo clippy`,
`cargo test`, `cargo check`, and nothing else -- including nothing from the
parenthetical `` (in `src-tauri`) `` sitting between two real commands.

`resolved_autonomous_gates` prefers explicitly configured gates and falls back
to the checklist only when none are set, so a user who chose their own gates
keeps them.

### Subagent projection

RLM children are the reason this provider is interesting. RPC exposes:

- `observe <activeSessionId>` → child event stream, wrapped as
  `observed_session_event` so it cannot be confused with the root's own events
- `observed_session_closed` on child exit
- `unobserve <activeSessionId>`

Wardian renders observed children as nested read-only cards under their root in
the Grid and Watchlist, with per-child token and cost attribution. Children are
not independent Wardian agents: they have no workspace, no class, and no
independent lifecycle. They are a projection of the root's subtree.

#### Parentage comes from the worker, not from the row *(verified)*

The listing exposes no parent pointer. Rows carry `rlmDepth` and
`hasRunningRlmChildren`, but nothing naming a row's parent, so a tree cannot be
reconstructed from depth alone once a root has more than one child.

What the rows do carry is `workerPid`, and Prime runs one worker per root
session tree. `group_session_trees` therefore groups by `workerPid`: the
`rlmDepth == 0` row in a group is the root and the rest are its subagents,
sorted shallowest first so a projection renders parents before children. A tree
deeper than one level is reported flat with each subagent's own depth
preserved, because grouping cannot recover which child spawned which
grandchild. Rows with no worker are excluded rather than guessed at.

This gives read-only projection without RPC. `observe` adds the live event
stream on top: `parse_observed_session` unwraps `observed_session_event` back
into a plain event, so a subagent's stream feeds the same `parse_output` a
root's does, and `observed_session_closed` distinguishes a clean exit from a
failed one.

Explicitly out of scope: making Wardian workflow nodes participate in prime's
A2A mesh. That would require implementing prime's daemon protocol v4 as a
client and reconciling two independent lease, journal, and recovery models.
Prime's A2A is also restricted to parent/sibling/child within a single root
tree, so cross-tree messaging is unavailable without Wardian owning every root.

### Skills round trip

Prime discovers skills via `--skill <path>` and its agent directory. Pointing
discovery at the junctioned habitat makes `~/.wardian/common/skills/*` visible
with no additional work.

The return direction is the novel part: prime's skill creator writes a new
Python-package skill into the habitat, `topology_watch.rs` observes the change,
and Wardian offers to promote it into the shared Library. An agent authoring a
first-class Wardian artifact is the ecological principle working end to end.

`prime_skills` implements the discovery half. The conversion turns out to be a
copy, not a translation: Prime follows the Agent Skills standard, so a skill is
a directory holding a `SKILL.md` with `name` and `description` frontmatter --
the same layout `library/skills/<name>/SKILL.md` already uses. `pyproject.toml`
alongside it marks a Python-backed skill, which is extra capability rather than
a different format.

Four scoping decisions:

- **Project scope only.** `project_skills_dir` resolves
  `<workspace>/.prime/agent/skills`. Prime also reads a global directory and
  npm-bundled skills, but promoting those would copy someone else's artifact
  into the Library.
- **One level deep.** A `SKILL.md` nested under a skill is that skill's
  reference material, not a skill of its own.
- **Frontmatter must open the file.** A `---` further down is a horizontal
  rule, and an unterminated block is malformed; both yield nothing rather than
  a skill invented from prose.
- **Existing Library names are left alone.** The Library copy is what gets
  deployed to agents, so offering to overwrite it from a workspace would
  silently promote the project's version to authoritative.

Verified against prime-agent 0.7.0's 13 bundled skills: all 13 parse, with
`skill-creator` and `prime-intellect` correctly identified as markdown-only.

## Implementation phases

Each phase is independently landable and independently reviewable.

| Phase | Scope | Gate |
|---|---|---|
| 0 | Environment spike: install, capture the real event stream, establish kernel viability | **Done** — see verified findings above |
| 1 | Provider contract via `--mode json`: `providers/prime.rs`, `PrimeProviderConfig`, factory, readiness, model catalog, headless args, chat transcript normalization, frontend provider option | **Done** — working provider, opaque root |
| 2 | Chat delivery off keystrokes | **Done** — `delivery/prime_send.rs` over `prime-agent send`, verified against a real agent. `providers/prime_rpc.rs` holds the JSONL protocol for a channel Wardian does not own; see the correction below for why |
| 3 | Lifecycle correctness: `stop <agent>` on kill, detached status, startup reconciliation | **Done** — **Non-optional. Do not ship 1–2 without it.** |
| 4 | Subagent projection | **Model done** — `group_session_trees`, `parse_observed_session`; UI outstanding |
| 5 | Autonomous gates from `AGENTS.md`; read-only schedule surfacing | **Done** — `providers/prime_gates.rs`, `parse_schedule_list_output` |
| 6 | Skills round trip | **Discovery done** — `providers/prime_skills.rs`; promotion action and watcher outstanding |

Phase 3 is listed after 1 and 2 for reviewability, but no phase-1 or phase-2
build may reach a release without it. An orphaned prime worker is a process
that keeps spending tokens after the user believes they stopped it.

### What is not built

The phases above landed their provider-side logic. Three integration pieces
remain, each deliberately deferred rather than half-built:

1. **The RPC transport, and it may never be built.** Input delivery no longer
   depends on it: `prime-agent send` is a second client of the same resident
   worker, so messages leave the keystroke path without Wardian owning the
   JSONL channel. What is still missing is `abort`, `set_model`, `compact`, and
   `observe`, which means phase 4's live subagent streaming stays unbuilt.
   Owning the channel costs the whole detached-worker lifecycle, for the
   reasons above, so this is a trade to decide deliberately rather than a
   deferred task.
2. **The projection UI.** `PrimeSessionTree` and `PrimeScheduledJob` are shaped
   for the Grid, Watchlist, and schedule surfaces but nothing renders them yet.
3. **Skill promotion.** `promotable_skills` identifies candidates;
   copying one into `library/skills` and hooking `topology_watch.rs`, which is
   currently scoped to `topology.json` alone, is the remaining half.

### Session identity, and what the native test found *(corrected)*

An earlier revision of this section called the missing `prime` arm in
`manager/session_identity.rs::apply_provider_identity` a latent gap, reachable
only once the RPC transport lands. That was wrong twice over, and the native
E2E is what proved it.

It was reachable. `spawn_agent` applies provider identity for whatever provider
the request names whenever a resume id is supplied, and so does the path that
turns an agent back on. Both failed with "prime does not define an
initialization identity contract".

Underneath that sat a worse one. Prime's interactive TUI is its only transport
that publishes no session header, so nothing ever produced an identity to
apply. `--resume` is only passed when `resume_session` is set and detached
reconciliation joins daemon rows on the same field, so both features were inert
rather than broken-looking.

Identity now has two sources, because one is exact and the other is early, and
neither is both:

- **The transcript.** Wardian pins `--session-dir` per agent, so the newest
  transcript there belongs to that agent. The id is read from the file's
  `{"type":"session"}` header, *not* its name: under `--session-dir` those are
  different UUIDs, because Prime reserves a file name when it constructs the
  session and then writes a header carrying another one. `--resume` accepts the
  header id and rejects the file name. Verified on prime-agent 0.7.0 against a
  completed run and an aborted one, after an earlier revision here assumed the
  name was the identity and shipped a reader that returned the wrong id.
  Exact, but Prime defers writing the file until the first assistant message.
- **The daemon listing.** Prime *reserves* the transcript path when it creates
  the session, so `list --json` reports `sessionFile` immediately, pointing
  into the pinned directory. That identifies the worker of an agent that has
  never said anything -- which is precisely the agent a teardown leaks.

Teardown uses the second. `request_prime_worker_stop` previously gave up when
the config carried no id, logged that the worker "may outlive the client", and
let it run. It now resolves the selector from the listing, and the native test
asserts the worker is gone afterwards.

### Teardown has to wait for the stop *(corrected)*

The stop was dispatched fire-and-forget, on the stated reasoning that the
request travels to the supervisor over its own socket and so does not depend on
the client Wardian is about to tear down. That reasoning ignored Wardian's own
process model: `init_app_process_supervisor` assigns the app to a
`kill_on_job_close` job object, so every child it spawns dies with it. On app
quit the stop client was killed during Node's startup, before it had connected
to anything, and the worker survived holding a live model session -- the exact
outcome the feature exists to prevent.

Observed, not reasoned about: two real delivery runs each left a worker with
zero attached clients, and issuing the identical `prime-agent stop` by hand
afterwards succeeded immediately. Teardown now waits for the client to exit,
bounded at five seconds, and logs whether the supervisor accepted.

### Delivery receipts on the interactive transport *(corrected)*

This spec said Prime's `DeliveryProfile` exists only so the TUI path does not
fall through to the unknown-provider default, and that "delivery receipts must
not depend on these markers" because the RPC transport would supply them. Until
that transport lands, the consequence was that Prime could not report a
delivery at all.

The first real run through `provider-delivery-real-native` showed the message
reaching Prime's TUI and the model answering with the expected marker, while
Wardian recorded the delivery as failed. `provider_accepted` waits on a
`turn_started` watch event, which comes from `AgentEvent::UserQuery`, which on
the PTY path comes from `parse_output` -- and Prime's TUI emits no JSON, so it
never fired.

Prime's TUI does publish its state unambiguously: a status line reading
`<spinner> Thinking · 2s` while a turn runs, and a footer carrying
`? for shortcuts` only while input is accepted. `PrimeTurnGate` reads turn start
and completion off those edges, the same shape the Antigravity gate already
uses, and `wait_for_terminal_ready_for_control_send` gained a `prime` arm
because Prime's agent status never leaves its spawn value on this transport.
Prime is now part of the real-delivery matrix rather than excluded from it.

These markers still drive the agent's *status*, but they no longer decide
whether a message was delivered. See the next section.

### Input delivery lands on `send`, not an RPC child *(corrected)*

The plan above was to swap Prime's interactive transport to `--mode rpc` and
drive everything over the JSONL channel. Reading the supervisor rules that out
as written: `--mode rpc` and `--print` both pass `clientOwned: true`, and
`isVisibleWorker` is `ownerClientId === undefined`. A client-owned worker is
hidden from every other client and reaped with the client that asked for one.

So the transport swap and phase 3 are mutually exclusive. Owning the RPC
channel would delete detached workers, the Detached status, startup
reconciliation, `prime-agent list`, `stop`, and the native lifecycle test --
the phase this spec marks "Non-optional. Do not ship 1–2 without it." The spec
asked for both without noticing they contradict.

Delivery therefore moves to `prime-agent send`, which is a second client of the
same resident worker. That satisfies the requirement this section actually
cares about -- Prime is not driven by emulated keystrokes -- while the TUI
stays, the worker stays resident, and phase 3 stays true. What it does not
provide is `abort`, `set_model`, `compact`, and `observe`, so live subagent
streaming remains unbuilt.

Three things about `send` had to be established by running it, because the
help text is wrong:

- **`--steer` and `--follow-up` do not exist.** `prime-agent help send` lists
  both; `parseSendArgs` in 0.7.0 accepts only `--from`, `--message`, and `--`,
  and rejects the two flags in every position. Scheduling is the daemon's
  decision, and the receipt reports which it made. Wardian's `QueuePolicy`
  cannot influence it, so `queued` is surfaced in `observed_state` rather than
  being papered over as a live turn.
- **`--json` prints the receipt, not the command envelope.** Unlike `stop`,
  which answers `{command, success}`, `send` prints its payload:
  `deliveryStatus` plus the `target` it resolved to. A failure never reaches
  that point at all -- the client exits non-zero with a plain-text `Error:`
  line and no JSON -- so both channels are checked.
- **The message goes through `--message`.** As a positional it is space-joined
  with any other operands, and a message beginning with `--` is rejected as an
  unknown option.

Two ordering facts also came out of real runs. Queue policy is spent before
delivery reaches this layer, so an early `Withhold` gate rejected every drained
mailbox message; and a message can be queued before Prime has registered its
worker, so the selector lookup waits for registration on the same budget the
keystroke path spends waiting for terminal readiness.

The same test also surfaced a defect with nothing to do with Prime's lifecycle:
`AgentConfigCompat::legacy_provider_config` matched every key `provider_key`
returns except `prime`, and fell through to `unreachable!()`. Deserializing any
config with `provider: "prime"` panicked. Because that runs inside
`Deserialize`, the Tauri command unwound before producing a response, so the
caller's promise never settled -- the failure looked like a hang, not an error,
and no Prime agent could be created at all. Both `unreachable!()` arms in that
file now fail soft onto a default, and a test walks every provider through the
flat JSON shape a caller actually sends.

## Affected files

**Backend**

| File | Change |
|---|---|
| `crates/wardian-core/src/models/agent_config.rs:55` | `ProviderConfig::Prime`, `PrimeProviderConfig`, `type_name()`, accessor |
| `src-tauri/src/providers/prime.rs` | New `AgentProvider` implementation |
| `src-tauri/src/providers/mod.rs` | Module registration and re-export |
| `src-tauri/src/providers/factory.rs:19` | `"prime"` resolve arm and error text |
| `src-tauri/src/providers/readiness.rs:20` | Descriptor plus `doctor` probe |
| `src-tauri/src/providers/models.rs` | `prime-agent model list` catalog source, composite ids |
| `src-tauri/src/providers/chat_transcript.rs:67` | `normalize_prime()` |
| `src-tauri/src/utils/delivery_profile.rs:32` | Conservative fallback profile |
| `src-tauri/src/manager/headless.rs:194` | `"prime"` args arm (`-p --mode json`) |
| `src-tauri/src/manager/headless.rs:1160` | `bootstrap_output_session_id` arm |
| `src-tauri/src/manager/spawn.rs:334` | Exclude `prime` from pre-bound identity guard |
| `src-tauri/src/manager/telemetry.rs:1058` | Transcript extraction and usage attribution |
| `src-tauri/src/workflow/resolve.rs:163` | Allow `prime` for workflow nodes |
| `src-tauri/src/commands/agent.rs:1085` | `bootstrap_provider_session` exclusion; stop path |

**Frontend**

| File | Change |
|---|---|
| `src/types/index.ts:2` | `UserFacingProviderName` union |
| `src/types/settings.ts:10` | `DefaultProviderSetting` union |
| `src/features/agents/providerOptions.ts:5` | `PROVIDER_ORDER`, `providerDisplayName` |
| `src/components/AdvancedSettings.tsx` | Prime config panel |
| `src/features/agents/configUtils.ts` | Config normalization |
| `src/features/terminal/terminalCapabilities.ts` | Terminal probe handling if the TUI path is retained |

## Consequences

- **Positive**: First provider whose event stream maps to `AgentEvent` without
  marker scraping or settle-delay tuning.
- **Positive**: RPC delivery removes the per-provider keystroke apparatus for
  this provider, and gives steering and follow-up as protocol commands rather
  than emulated key combinations.
- **Positive**: Agents survive app restart, a capability Wardian has for no
  other provider.
- **Positive**: Nested subagent visibility gives the Grid and Watchlist a real
  multi-session subject, directly serving the situational-awareness principle.
- **Positive**: Session JSONL under the agent workspace makes transcripts
  readable with no live provider process.
- **Positive**: One provider entry reaches nine model backends.
- **Negative**: Introduces a lifecycle shape the provider layer has not needed,
  including a new status and a startup reconciliation pass. Getting phase 3
  wrong orphans token-spending processes.
- **Negative**: Adds a Python/IPython runtime dependency no other provider has,
  and a bash dependency on Windows. Worse, prime 0.7.0 cannot bootstrap that
  kernel on Windows at all, so Wardian must provision and manage the venv
  itself until upstream fixes the layout bug.
- **Negative**: `ActionRequired` reaches Wardian only through extension UI
  requests, so a Prime session with no extension installed never shows amber.
  That is accurate rather than lossy -- Prime genuinely never blocks on the
  user otherwise -- but it means the status carries less information for this
  provider than for Claude or Codex.
- **Negative**: Two schedulers exist in the system. The read-only surfacing
  decision manages the risk but does not remove the duplication.
- **Negative**: Prime is a young project on a young upstream fork; its daemon
  protocol is at v4 with explicitly independent protocol and schema revisions,
  so the integration should expect wire changes.
