# Provider-Native Communication Control Plane

- **Status:** Proposed
- **Date:** 2026-06-07

## Context and Problem Statement

Wardian's current communication path is still too dependent on terminal input
delivery. The existing provider-aware delivery transport improves readiness
gating and reports unverified delivery states, but ordinary inter-agent
messages can still reduce to "bytes were written to a PTY." That is not enough
for reliable agent coordination.

The failure mode is visible in live use: a structured `wardian ask` can receive
a valid reply while watch evidence degrades with `cursor_expired`, and live
delivery can report `submit_sent_unconfirmed`. For OpenCode, terminal delivery
is especially fragile because the TUI owns the input box, key handling,
autocomplete, wrapping, and provider state.

cmux points to a better architecture:

- provider-native hooks and plugins handle structured events and decisions;
- local sockets connect provider-side code to the orchestrator;
- terminal surface automation is a compatibility layer, not the source of
  truth for provider communication;
- tmux compatibility is implemented by a fake `tmux` shim that maps tmux verbs
  onto orchestrator surface APIs.

Wardian needs a provider-native communication control plane that preserves its
visible live sessions while proving delivery through provider-owned events,
APIs, logs, or hooks.

## Goals

- Make Wardian-owned interactions durable before any provider transport runs.
- Treat PTY writes as one possible transport attempt, not delivery proof.
- Use provider-native hooks, plugins, APIs, and transcripts whenever a provider
  exposes them.
- Support Claude, Codex, Gemini, Antigravity, and OpenCode with explicit
  provider-specific adapters.
- Add a cross-platform local control channel for provider hooks/plugins.
- Support Windows, macOS, and Linux without requiring real tmux, Unix sockets,
  POSIX-only wrapper scripts, or global PATH mutation.
- Make delivery state observable through CLI, UI, logs, and tests.
- Keep provider action requests separate from ordinary messages.

## Non-Goals

- Replacing `portable-pty` or ConPTY.
- Requiring real tmux on any platform.
- Implementing full tmux compatibility.
- Routing provider permission decisions through `send-keys` when hooks/plugins
  can return provider-native decisions.
- Treating OpenCode as a PTY-first provider.
- Implementing Claude Teams split-pane or tmux compatibility in the first
  provider-native communication slice.
- Mutating global provider config without marker ownership and uninstall
  behavior.
- Proving provider behavior through browser-only tests.

## Decision

Add a provider-native communication control plane with five layers:

1. **Durable interaction store**: every outbound message, ask, reply, action
   request, and provider decision receives a Wardian event id before transport.
2. **Local control channel**: provider hooks, plugins, wrappers, and shims call
   back into Wardian over a per-`WARDIAN_HOME` endpoint.
3. **Provider adapters**: each provider declares how it binds sessions, reports
   status, receives messages, returns action decisions, and proves delivery.
4. **Surface prompting**: ordinary live prompts enter through a Wardian-owned
   surface input transaction, with provider-aware readiness and observation.
   This remains separate from provider action decisions.
5. **Delivery evidence model**: a delivery is only `provider_applied` after a
   provider-native event, API response, hook acceptance, transcript/log
   advancement, or explicit plugin acknowledgment proves it.

This extends, rather than replaces,
[Agent Delivery Transport](./2026-05-22-agent-delivery-transport.md).

## Interaction Model

Wardian should persist these communication records:

| Record | Purpose |
| --- | --- |
| `message` | An ordinary inter-agent or user-to-agent message. |
| `ask` | A message that expects a structured `reply`. |
| `reply` | A response to a Wardian request id. |
| `action_request` | A provider permission, question, plan, approval, or tool-gate event that requires a decision. |
| `action_decision` | A Wardian UI/CLI/user/agent decision for an action request. |
| `delivery_attempt` | A single provider transport attempt with timing and evidence. |

Each record should include:

- stable Wardian event id;
- source Wardian session id when known;
- target Wardian agent id and provider session id when known;
- provider id;
- workspace id or normalized workspace path;
- rendered body or structured payload reference;
- status;
- attempt count;
- created and updated timestamps;
- provider evidence cursor when available;
- last error or blocked reason.

Direct live prompt submissions must not persist the full prompt body by default.
They should store delivery metadata plus a redacted body reference, such as a
hash and byte count. Full body retention requires an explicit audit-retention
mode. Persisted delivery errors must omit raw provider stdout/stderr unless a
separate debug-retention surface owns that data.

The interaction store must be the source of truth for `ask` completion. Watch
output and terminal transcript are evidence, not the completion authority.

## Delivery States

Use additive delivery states that can coexist with existing CLI schema fields:

| State | Meaning |
| --- | --- |
| `queued` | Wardian persisted the interaction but did not attempt provider transport. |
| `transport_started` | A delivery worker selected a provider transport. |
| `input_sent_unconfirmed` | Wardian sent PTY/surface input but has no provider proof yet. |
| `prompt_echo_observed` | Wardian observed the prompt body in the provider composer or terminal surface before submit. |
| `submit_sent_unconfirmed` | Wardian sent the submit key but has no provider turn proof yet. |
| `provider_received` | A provider hook/plugin observed the interaction or trigger. |
| `provider_applied` | The provider accepted the message, action decision, or prompt. |
| `provider_rejected` | The provider rejected the decision or message in a known way. |
| `blocked` | Delivery is unsafe, missing config, waiting for action, or unavailable. |
| `timed_out` | The delivery attempt expired without proof. |
| `failed` | A non-retryable transport or provider error occurred. |

`input_sent_unconfirmed` must not be displayed as successful delivery. It can
be useful evidence, but automation should treat it as uncertain.

## Regular Prompting Model

cmux's ordinary prompt path is terminal-surface input. Its provider hooks and
plugins mostly observe accepted turns or handle permissions, questions, and
plan decisions. Wardian should copy that separation.

Regular live prompting should use a backend-owned surface input transaction for
all supported providers unless a provider-specific prompt API is deliberately
implemented as a separate non-TUI mode. The transaction should be explicit:

1. Resolve the target by Wardian agent id and provider session id.
2. Check provider readiness for the current input generation.
3. Acquire the per-session surface input lock.
4. Write the prompt payload with the provider's configured text/paste strategy.
5. Observe prompt echo or composer visibility when the provider profile can do
   so.
6. Send the provider-specific submit key only after the payload is accepted or
   after a bounded profile-specific fallback window.
7. Observe provider proof: hook event, transcript/log advancement, status
   transition, terminal turn marker, or timeout.
8. Return the strongest evidence state.

This model is especially important on Windows. Sending text followed quickly by
Enter is not a correctness mechanism under ConPTY. Text entry can arrive late,
be bracketed-paste buffered, be swallowed during TUI readiness changes, or have
the carriage return interpreted before the provider composer owns the payload.
Wardian should treat `\r` as the submit key for terminal input, but submit
success requires observation. A result of "we wrote text and Enter" is
`submit_sent_unconfirmed`, not `provider_applied`.

Provider profiles should define:

- text strategy: literal text, bracketed paste, named paste operation, or
  provider-specific fallback;
- submit key: usually `\r`, but expressed as a profile field rather than hard
  coded;
- minimum payload-settle or echo-wait policy before submit;
- prompt echo/composer recognizer when available;
- turn-start/status/transcript recognizers after submit;
- retry policy.

Retries must be conservative. If the prompt body or submit key may have reached
the provider, Wardian must not blindly resend. It should surface
`input_sent_unconfirmed` or `submit_sent_unconfirmed` with captured evidence.
Retry is safe only before any bytes enter the provider surface, or after the
provider adapter proves the previous attempt was rejected without side effects.

Surface prompting should support background and cold sessions through bounded
per-surface queues:

- `sent`: bytes accepted by a live surface;
- `queued`: input accepted into a bounded surface queue;
- `input_queue_full`: queue capacity reached;
- `surface_unavailable`: no writable runtime or queue;
- `process_exited`: provider process is gone;
- `target_not_found`: Wardian cannot resolve the target.

The queue must be item- and byte-bounded, drain in order, and report queue-full
as a structured error. It must not require UI focus.

## Headless Delivery Boundary

Wardian's current headless agents are a separate delivery transport from live
surface prompting. A workflow or off-agent run that executes headlessly should
not enter the surface input queue, because there is no live TUI composer to
drive. Instead, Wardian launches the provider's one-shot or print-mode CLI,
passes the prompt as provider process input or arguments, captures stdout and
stderr, and normalizes the provider result.

This makes headless prompt delivery more deterministic than live PTY injection:
there is no ConPTY submit-key race and no focus/composer ownership problem.
It still needs provider-specific evidence and error handling, because each
provider's headless mode has different session, resume, JSON, timeout, and
stdout/stderr behavior.

The durable interaction store should cover both live and headless records, but
the transport attempt type must differ:

- `headless_process`: spawn provider process, attach Wardian/session identity
  env, pass the prompt through the provider's supported headless interface,
  capture provider output, and mark delivery from process exit plus parsed
  provider response.
- `live_surface`: enqueue or send a surface input transaction, observe echo or
  readiness, submit with the provider profile key, and wait for provider proof.

Workflow live-agent execution already approximates this split: it creates a
Wardian interaction task, submits the prompt to the active PTY, then completes
only when a structured `wardian reply` or compatible transcript marker appears.
The provider-native communication work should generalize that model for all
live sends while preserving the existing headless process boundary.

## Provider Matrix

| Provider | Primary robust path | Message delivery | Action requests and replies | Status and session binding | Fallback |
| --- | --- | --- | --- | --- | --- |
| Claude | Wrapper-injected hooks plus Wardian local control channel. | Surface input transaction with echo/submit/turn observation. Prove with `UserPromptSubmit`, transcript/session evidence, or status transition. | Native `PermissionRequest` hook returns provider-expected JSON through hook stdout. Other lifecycle hooks report telemetry. | Wardian already assigns `--session-id`; wrapper exports Wardian ids and injects settings only inside managed terminals. | Truthful `input_sent_unconfirmed` or `submit_sent_unconfirmed`; Claude Teams split-pane support is deferred. |
| OpenCode | Wardian OpenCode plugin for action events and telemetry, plus surface input for ordinary live prompts. | Surface input transaction unless Wardian later implements a separate non-TUI OpenCode prompt mode. Plugin events can prove accepted user messages after the fact. | Plugin handles permission, question, plan feedback, and session permission APIs from inside OpenCode. | Plugin binds OpenCode `sessionID` to Wardian agent/session id and reports session events. | Queue or unconfirmed surface delivery; avoid blind TUI retries after submit. |
| Codex | Hook config where supported, plus transcript/session-log proof. | PTY/surface prompt submission, proven by `UserPromptSubmit`, log advancement, or turn start/completion events. | Native hook path for permission/tool events when available; fail closed when hook coverage is missing. | Bind through provider session id, `CODEX_HOME`, workspace, and hook payloads. | PTY with provider-aware readiness and unconfirmed result; restart required after hook config changes. |
| Gemini | Hook config with version-gated event names. | PTY/surface prompt submission, proven by `BeforeAgent`/equivalent lifecycle event, stream event, or transcript. | Tool gate hooks such as `BeforeTool` or compatible aliases return decisions when supported. | Bind through provider session output, workspace, and hook payloads. | PTY with readiness gating; support both older and current hook names during migration. |
| Antigravity | Hook config for lifecycle/tool gates plus transcript proof. | PTY prompt submission initially; do not use provider-internal continuation hooks for external sends until real-provider tests prove behavior. | Native PreToolUse-style decision JSON for allow, deny, ask, or force-ask where supported. | Bind through conversation id, transcript paths, workspace, and hook payloads. | PTY with status/transcript proof; treat `SessionEnd` as a possible turn boundary, not process exit. |

## Provider Requirements

### Claude

Wardian should add a Claude launcher/wrapper layer that is native to Wardian,
not a POSIX-only shell script. The launcher must:

- run only for Wardian-managed provider processes;
- verify the Wardian control endpoint is reachable before injecting hooks;
- avoid wrapping noninteractive commands that should pass through untouched;
- avoid recursively invoking itself when resolving the real Claude executable;
- inject a generated settings file or CLI settings payload with hook commands;
- preserve existing Claude session assignment and resume behavior.

Hook events should include at least:

- `SessionStart`;
- `UserPromptSubmit`;
- `PreToolUse`;
- `PermissionRequest`;
- `Stop`;
- `SessionEnd`;
- `Notification`;
- subagent completion events when the installed Claude version emits them.

Permission requests should become Wardian `action_request` records. A user or
agent decision should become an `action_decision`, then the hook should return
the provider-native JSON shape to Claude. Provider-native hook acceptance is the
delivery proof for that decision.

Claude Teams split-pane support is deferred. When revisited, it should be
implemented through fake tmux compatibility and capability detection, not
through the permission hook path.

### OpenCode

Wardian should implement OpenCode as the first plugin provider prototype for
action events and telemetry.
The plugin must:

- install into a marker-owned global or project-local OpenCode plugin location;
- refuse to overwrite unmarked user files;
- connect to `WARDIAN_CONTROL_ENDPOINT`;
- bind OpenCode `sessionID` to the Wardian agent id and workspace;
- subscribe to session, message, permission, question, todo, and plan-related
  events exposed by the installed OpenCode version;
- submit permission, question, and plan replies through OpenCode APIs from
  inside the plugin;
- acknowledge Wardian event ids only after the OpenCode API accepts the action
  or a later OpenCode event proves processing.

OpenCode ordinary live prompting should use the same surface input transaction
as other live providers until Wardian deliberately designs and validates a
separate non-TUI prompt mode. The plugin can still provide strong observation
when OpenCode emits message events after the prompt is accepted. If surface
delivery is unconfirmed, Wardian must not blindly retry by typing the body
again into the OpenCode TUI.

### Codex

Wardian should add Codex hook installation behind a conservative capability
flag. The adapter must:

- write hook config structurally into the per-agent `CODEX_HOME`;
- preserve and merge user config rather than replacing it;
- mark Wardian-owned hook entries for update and uninstall;
- report hook coverage as partial in diagnostics;
- require restart or explicit reload when Codex does not observe live config
  edits;
- keep PTY prompt delivery as the main transport until hooks prove prompt
  receipt reliably.

Codex delivery proof can come from:

- `UserPromptSubmit` hook payload;
- `thread.started` or `turn.started` event after a queued message is submitted;
- provider transcript/session log advancement;
- observed terminal transition as a weaker fallback.

Known hook gaps and provider issues should keep Codex from being a hard
dependency for the control plane rollout.

### Gemini

Wardian should support Gemini hooks through a version-aware config writer. The
adapter must:

- detect the installed hook schema or support both current and older event
  names where safe;
- configure lifecycle hooks for session start/end and before/after agent turn
  events;
- configure tool-gate hooks for side-effecting operations when supported;
- return provider-native decisions for tool gates;
- preserve existing include-directory and skill patch behavior.

Gemini prompt delivery remains PTY-based. Proof should prefer structured stream
events and lifecycle hooks over terminal repaint text.

### Antigravity

Wardian should add Antigravity hook support as a separate adapter rather than
reusing Gemini assumptions. The adapter must:

- write config in Antigravity's expected customization location and format;
- support lifecycle, pre-invocation, stop, notification, turn-completion, and
  tool-gate events where available;
- treat `SessionEnd` as a possible turn boundary;
- bind events to Antigravity conversation id and transcript path;
- keep existing real-workspace and projection behavior intact.

External Wardian messages should continue through PTY delivery until real
provider tests prove a better provider-native prompt path.

## Local Control Channel

Wardian should expose one per-home local endpoint for provider-side code:

- macOS/Linux: Unix domain socket is acceptable.
- Windows: named pipe or loopback TCP with an auth token.

The endpoint should be abstracted as `WARDIAN_CONTROL_ENDPOINT` so hooks,
plugins, wrappers, and shims do not hardcode OS-specific transport paths.

Every request should carry:

- Wardian protocol version;
- provider id;
- Wardian session id when available;
- provider session id when available;
- request id or event id;
- payload;
- deadline for blocking decisions.

Blocking provider decisions must have explicit timeout behavior. When the
provider process exits, Wardian should resolve pending requests as
`agent_exited` rather than leaving them pending.

## Cross-OS Requirements

### Launchers and Wrappers

Provider wrappers should be implemented as Rust/native launchers or paired
platform shims:

- POSIX shells can use small shell scripts only as thin delegates.
- Windows should prefer `.exe` launchers and `.cmd` delegates.
- PowerShell scripts should be diagnostic-only because execution policy can
  block them.

Executable discovery must avoid wrapper recursion:

- POSIX: search PATH while skipping Wardian shim directories.
- Windows: honor `PATHEXT`, detect `.exe`, `.cmd`, and `.bat`, and skip
  Wardian shim directories.

### Config Writes

Provider config writers must parse and merge structured formats:

- JSON for OpenCode and many hook configs;
- TOML for Codex where needed;
- provider-specific config directories from environment overrides.

Writers must create marker-owned blocks or files, refuse to overwrite unmarked
user files, and support update/uninstall.

### IPC

Do not assume Unix sockets on Windows. Provider-side code should receive a
single `WARDIAN_CONTROL_ENDPOINT` value and a short-lived auth token where
needed.

### PTY Input

ConPTY and Unix PTYs should be treated as different implementations behind the
same provider delivery profile:

- send Enter as `\r` when submitting terminal input;
- wait for payload echo, composer visibility, provider readiness, or a
  provider-profile fallback before sending `\r` when possible;
- treat text plus `\r` as unconfirmed until provider proof arrives;
- normalize captured output to `\n`;
- preserve UTF-8 through native code paths;
- avoid clipboard as a delivery transport;
- do not build shell command strings when structured cwd, argv, and env are
  available.

## Tmux Compatibility

Claude Teams split-pane support is deferred from the first implementation.
This section records the future design boundary so it does not leak into the
regular prompting or provider-action work.

Tmux compatibility is a fake tmux surface, not a dependency on real tmux.
It should reuse Wardian's existing backend control and terminal-attach
foundation rather than introduce a separate multiplexer process.

Wardian already has relevant backend pieces:

- a per-`WARDIAN_HOME` control endpoint with Windows named-pipe and Unix socket
  implementations;
- `TerminalAttachState`, which tracks terminal attachments, ownership,
  geometry, vt100 screen state, and live PTY byte streaming;
- existing PTY input senders, resize paths, and non-draining terminal snapshot
  paths used by the remote PWA.

Tmux compatibility should add the missing tmux-facing layer on top of those
pieces:

- a parser for the supported tmux argv subset;
- a mapping store from tmux ids such as `%1` to Wardian agent, pane, surface,
  attachment, or future split ids;
- backend operations for create/split/focus/close/read/send that call Wardian
  APIs instead of controlling an external tmux server.

Wardian should expose a private shim directory only inside Wardian-managed
provider process trees. It should prepend that directory to the provider
environment and set:

- `TMUX` to a tmux-shaped value that identifies the Wardian endpoint;
- `TMUX_PANE` to a tmux-shaped pane id such as `%1`;
- `WARDIAN_TMUX_COMPAT=1`;
- `WARDIAN_CONTROL_ENDPOINT`;
- `WARDIAN_WORKSPACE_ID`;
- `WARDIAN_WINDOW_ID`;
- `WARDIAN_SURFACE_ID`;
- provider-specific feature flags such as
  `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` for Claude Teams.

The shim must be a thin compatibility entry point, not a new authoritative
runtime. The implementation should prefer the existing Wardian CLI/control
binary as the real executable:

- POSIX: a private `tmux` shell script delegates to
  `wardian __tmux-compat <argv...>`.
- Windows: a private `tmux.cmd` delegates to
  `wardian.exe __tmux-compat %*` for `cmd.exe` and PowerShell shellouts.
- Windows direct-process support should first try to expose the existing
  Wardian executable through an alias, link, or launcher mode only if
  real-provider tests prove a provider directly spawns `tmux` without a shell.
  Do not design a separately built `tmux.exe` as the default artifact.

Wardian should not install or shadow `tmux` globally.

### P0 Supported Tmux Verbs

| Verb | Wardian behavior |
| --- | --- |
| `new-session` | Create or bind a Wardian tmux-compat workspace/session record. |
| `new-window` | Create or bind a Wardian window/surface group. |
| `split-window` | Create a new Wardian pane/surface, optionally with a startup command. |
| `send-keys` | Map literal text and special keys to the target surface input API. |
| `capture-pane -p` | Return normalized visible/logical text from the target surface. |
| `select-pane` | Focus a target Wardian surface. |
| `select-window` | Focus a target Wardian window/surface group. |
| `list-panes` | Return a tmux-format-compatible pane inventory for the supported fields. |
| `list-windows` | Return a tmux-format-compatible window inventory for the supported fields. |
| `kill-pane` | Close a Wardian pane/surface. |
| `kill-window` | Close a Wardian window/surface group. |

P1 compatibility may add `resize-pane`, `swap-pane`, `break-pane`,
`pipe-pane`, `wait-for`, buffer verbs, and `display-message` only after tests
show provider tools require them.

Unsupported verbs should return clear errors. Wardian must not claim full tmux
compatibility.

### `send-keys` Rules

The parser should preserve argv whenever possible.

- With `-l`, treat following tokens literally and join them with spaces.
- Without `-l`, map special keys:
  - `Enter`, `C-m`, `KPEnter` to `\r`;
  - `Tab`, `C-i` to `\t`;
  - `BSpace`, `Backspace` to DEL `0x7f`;
  - `Escape`, `Esc`, `C-[` to ESC;
  - `C-c` to `0x03`;
  - `C-d` to `0x04`;
  - `C-z` to `0x1a`;
  - `C-l` to `0x0c`.
- Do not append an extra newline unless an Enter-like key is explicitly sent.

Startup commands should prefer structured process spawn with cwd, argv, and env.
If a shell command is unavoidable, the implementation must choose a shell-aware
form for the actual shell family.

## Implementation Plan

The first implementation slice must unify every structured prompt injection
surface before provider hooks/plugins are enabled. `submit_prompt_to_agent`,
`wardian send`, command panel injection, library injection, grid chat, remote
PWA prompt sends, workflow live routing, and mailbox drain must share the same
backend live-surface delivery service. Raw terminal keystrokes remain outside
this path and continue to use `send_input_to_agent` or `send_binary_input_to_agent`.

### Phase 0: Design Guardrails and Test Fixtures

- Add a provider-native communication feature flag.
- Define Rust models for interaction records, action requests, delivery
  attempts, and delivery evidence.
- Add deterministic fake provider fixtures for hook/plugin/control-channel
  events.
- Add cross-platform control endpoint abstraction with fake in-process backend
  for tests.
- Add native test fixtures for Windows ConPTY and Unix PTY input observation.

Exit criteria:

- Unit tests can create an interaction, attach delivery attempts, and resolve
  an `ask` from the interaction store without watch evidence.
- The control endpoint abstraction has passing tests for request routing,
  timeout, auth failure, and process-exit cleanup.

### Phase 1: Interaction Store and Delivery Evidence

- Persist messages, asks, replies, action requests, decisions, and delivery
  attempts in Wardian state.
- Update `wardian send`, `wardian ask`, and UI delivery paths to record
  interactions before transport.
- Return delivery states that distinguish `input_sent_unconfirmed` from
  `provider_applied`.
- Keep existing PTY delivery behavior behind the new evidence model.
- Add `prompt_echo_observed` and `submit_sent_unconfirmed` evidence states.

Exit criteria:

- `wardian ask` returns a structured reply even if watch evidence later returns
  `cursor_expired`.
- CLI and UI show unconfirmed PTY delivery as uncertain.
- Existing send behavior remains compatible for users who do not opt into
  provider-native adapters.

### Phase 2: Regular Surface Prompting Transactions

- Add a backend surface input transaction used by live prompt delivery.
- Keep stable target resolution by Wardian session/surface id, not UI focus.
- Add bounded per-surface queues for cold or background surfaces.
- Split payload write, payload observation, submit key, and provider proof into
  separate delivery attempt phases.
- Encode provider submit keys through delivery profiles.
- Add Windows ConPTY tests for delayed text entry, submit arriving before echo,
  swallowed Enter, bracketed-paste settle, and CR/LF handling.
- Prevent retries after any attempt that may have delivered bytes or submit.

Exit criteria:

- A live prompt can return `sent`, `queued`, `input_queue_full`,
  `surface_unavailable`, `process_exited`, or `target_not_found`.
- A Windows native test proves Wardian does not mark text plus `\r` as
  provider-applied without echo, status, hook, or transcript evidence.
- A delayed composer echo causes Wardian to wait before submit when the provider
  profile requires echo observation.
- Unknown post-submit state returns `submit_sent_unconfirmed` instead of
  retrying.

### Phase 3: Local Control Channel

- Implement the per-`WARDIAN_HOME` local endpoint.
- Add provider hook/plugin request handlers:
  - `event.push`;
  - `action.request`;
  - `action.resolve`;
  - `message.read`;
  - `message.ack`;
  - `session.bind`;
  - `status.update`.
- Add timeout and process-death cleanup for blocking decisions.
- Add logs and CLI inspection for pending control-channel requests.

Exit criteria:

- A fake provider can create an action request, wait for a Wardian decision,
  and receive a provider-shaped reply.
- Pending requests become `timed_out` or `agent_exited` deterministically.

### Phase 4: OpenCode Plugin Prototype

- Add marker-owned OpenCode plugin installation and uninstall.
- Connect the plugin to `WARDIAN_CONTROL_ENDPOINT`.
- Bind OpenCode session ids to Wardian agents.
- Report session, message, permission, question, todo, and plan-related events.
- Use plugin events as observation for ordinary surface-submitted prompts.
- Resolve permission, question, and plan decisions through OpenCode APIs.
- Acknowledge action request ids only after OpenCode accepts or processes them.

Exit criteria:

- Real-provider native E2E proves an OpenCode agent can receive a Wardian
  message through the surface input transaction with honest delivery evidence.
- Permission/question reply paths are provider-applied through OpenCode APIs.
- Plugin restart/rebind does not duplicate action decisions or prompt
  observations.

### Phase 5: Claude Hook Launcher and Action Requests

- Add a Wardian Claude launcher/wrapper that injects hook settings only inside
  managed sessions.
- Route Claude lifecycle and permission hooks into the local control channel.
- Convert `PermissionRequest` events into Wardian action requests.
- Return provider-native hook JSON after a Wardian decision.
- Preserve current Claude session id and resume behavior.

Exit criteria:

- Real-provider native E2E proves a Claude permission request appears as a
  Wardian action request and resolves through hook stdout.
- Normal prompt delivery remains visible in the live session and reports proof
  when a hook/transcript event confirms submission.

### Deferred Phase: Fake Tmux Compatibility for Claude Teams

- Add `wardian __tmux-compat <argv...>`.
- Add private shim generation for POSIX `tmux` and Windows `tmux.cmd`.
- Reuse the existing Wardian control endpoint and terminal attach/input/resize
  primitives instead of adding a new multiplexer process.
- Add a tmux-compat store under `WARDIAN_HOME`.
- Implement P0 verbs: `new-session`, `new-window`, `split-window`,
  `send-keys`, `capture-pane -p`, `select-pane`, `select-window`,
  `list-panes`, `list-windows`, `kill-pane`, and `kill-window`.
- Launch Claude Teams with the private shim directory, fake `TMUX`,
  `TMUX_PANE`, and `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`.

Exit criteria:

- Windows native E2E proves a provider child resolves Wardian's private tmux
  shim without modifying global PATH.
- `tmux split-window` creates a Wardian pane.
- `tmux send-keys -t <pane> "hello" Enter` reaches the target PTY.
- `tmux capture-pane -p -t <pane>` returns normalized output.
- No global PATH or user tmux installation is modified.

### Phase 6: Codex Hooks

- Add marker-owned Codex hook config writer under the agent `CODEX_HOME`.
- Route supported hook events to the control channel.
- Use Codex hooks and logs as delivery proof when available.
- Keep PTY prompt delivery and readiness gating as the main message transport.
- Surface hook coverage and restart requirements in diagnostics.

Exit criteria:

- Codex hook events update status and delivery evidence in fake-provider tests.
- Real-provider tests prove no duplicate submissions when hook proof and PTY
  evidence arrive in different orders.

### Phase 7: Gemini and Antigravity Hooks

- Add version-aware Gemini hook config writer.
- Add Antigravity-specific hook config writer.
- Route lifecycle and tool-gate events into the control channel.
- Return provider-native tool-gate decisions.
- Keep existing instruction, skill, and workspace behavior unchanged.

Exit criteria:

- Fake-provider tests cover both current and legacy Gemini hook names.
- Antigravity tests treat turn-completion and `SessionEnd` boundaries
  correctly.
- Real-provider opt-in tests prove tool-gate decisions are applied.

### Phase 8: UI, CLI, and Diagnostics

- Show interaction status, delivery attempts, blocked reasons, and provider
  evidence in the UI and CLI.
- Add `wardian message` or equivalent diagnostics for reading pending
  interactions and delivery attempts.
- Add setup diagnostics for missing plugins, hooks, wrappers, or shim
  installation.
- Add uninstall/update paths for marker-owned provider integrations.

Exit criteria:

- A user can inspect why a message is queued, unverified, provider-applied,
  blocked, or failed.
- Provider integration health is visible before communication is attempted.

## Testing Strategy

Use the lowest test layer that can prove each behavior.

### Unit Tests

- Interaction store status transitions.
- Delivery evidence precedence and idempotency.
- Provider adapter capability matrix.
- Config writer merge/update/uninstall for each provider.
- Tmux argv parser and `send-keys` special-key mapping.
- Cross-platform endpoint parsing.

### Native Fake-Provider Tests

- Hook event to action request to decision to provider reply.
- Plugin message read and ack.
- Timeout and process-exit cleanup.
- PTY submission with unverified result.
- Regular surface prompt phases: payload written, echo observed, submit sent,
  provider proof observed, and submit unconfirmed.
- Watch cursor degradation after structured reply.
- Tmux P0 verb mapping.

### Real-Provider Native E2E

Required for claims about:

- OpenCode plugin action handling and telemetry observation;
- regular surface prompt delivery for each provider;
- Claude permission hook resolution;
- Claude Teams fake tmux behavior if the deferred phase is enabled;
- Windows ConPTY line endings and special keys;
- Windows ConPTY timing around payload echo and submit;
- provider-specific config reload or restart requirements.

Browser E2E may verify UI projection only. It cannot prove PTY, hook, plugin,
or native provider behavior.

## Source Notes

cmux mechanics that informed this design:

- `Resources/opencode-plugin.js`: OpenCode plugin, feed push, permission and
  question replies.
- `Resources/bin/cmux-claude-wrapper`: Claude wrapper-injected hooks.
- `CLI/cmux.swift`: OpenCode plugin installation, feed hook handling, Claude
  Teams and tmux compatibility entry points.
- `CLI/CMUXCLI+AgentHookDefinitions.swift`: provider hook definitions.
- `CLI/CMUXCLI+TmuxCompatSupport.swift`: tmux verb parsing and special-key
  mapping.
- `Sources/TerminalController.swift`: surface and feed socket verbs.
- `Sources/GhosttyTerminalView.swift`: terminal surface input queue behavior.
- `Sources/TextBoxInput.swift`: prompt text dispatch, submit key sequencing,
  pasteboard serialization, and visible-text waits.
- `Sources/Feed/FeedCoordinator.swift`: blocking feed decisions and reply
  waiters.
- `docs/feed.md`: cmux feed architecture.

Wardian specs this design extends:

- [Agent Delivery Transport](./2026-05-22-agent-delivery-transport.md)
- [Provider Readiness Gating](./2026-05-19-provider-readiness-gating.md)
- [Communication Interaction Graph](./2026-05-21-communication-interaction-graph.md)
- [OpenCode Provider Stabilization](./2026-04-07-opencode-provider-stabilization.md)

## Risks

- Provider hook APIs can change without notice.
- Regular surface prompting remains fallible. Wardian must expose unconfirmed
  states instead of treating terminal input as provider delivery proof.
- Codex hook coverage is currently less reliable than Claude or OpenCode
  plugin surfaces.
- OpenCode plugin install locations and event names may drift.
- Windows `.cmd` shims can lose argv fidelity for complex input. The first
  implementation should keep the shim thin and move parsing into
  `wardian __tmux-compat`; if direct-process provider behavior requires an
  executable named `tmux`, use an alias/link to the existing Wardian binary
  rather than a separately maintained binary.
- Provider-native context injection can consume tokens when it adds text to the
  model context. Telemetry, permission decisions, and local state sync are
  token-free unless inserted into a prompt.
- Full tmux compatibility is large. Wardian should support only observed verbs
  and return explicit unsupported-command errors.

## Rollout Order

1. Interaction store and evidence model.
2. Regular surface prompting transactions.
3. Local control channel.
4. OpenCode plugin action and telemetry path.
5. Claude hook launcher and action path.
6. Codex hooks and evidence integration.
7. Gemini and Antigravity hooks.
8. UI/CLI diagnostics and integration health.
9. Deferred: fake tmux compatibility for Claude Teams, including capability
   detection on Windows.

This order fixes the most broken provider first, stabilizes the strongest hook
surface second, and keeps Claude Teams/tmux compatibility out of the regular
prompting and provider-action work until the base communication path is stable.
