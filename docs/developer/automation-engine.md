# Automation Engine Architecture

Wardian's current automation engine is the durable blueprint runner in `wardian-core`.
Blueprints live as markdown-backed automation definitions, execute through
`wardian_core::engine`, and are launched by the Tauri automation commands in
`src-tauri/src/commands/automation.rs`.

The old JSON automation system used `run_automation`, `AutomationDefinition`,
`automation_engine`, trigger nodes, and live telemetry events. Treat those names as
old automation system references only; new automation work should use the blueprint,
run-log, and schedule APIs below.

## Core Concepts

### Blueprint

An automation blueprint is the authored graph. It declares nodes, edges, fields, and
registry-backed node types. The current authoring surface writes markdown
blueprints under `library/automations`, and the backend parses and validates them
through `wardian_core::automation`.

### Run

A run is one execution instance of a blueprint. Runs are durable on disk under
`logs/automations/<blueprint-id>/<run-id>/` and write:

- `events.jsonl` for append-only execution events;
- `state.json` for the current checkpoint;
- run-local files such as cancellation markers.

The frontend Observe and Monitor modes read these durable files through
`automation_list_runs` and `automation_read_run`; automation progress is not driven by
the old automation system telemetry events.

Checkpoint replacement preserves complete snapshots for concurrent readers.
On Windows, the workflow store uses `ReplaceFileW` for an existing checkpoint
so readers sharing deletion can finish reading the old snapshot while the
driver publishes the next one. Initial creation uses the ordinary atomic move.
Readers that deny delete sharing still cause a write error; the store does not
retry, truncate the checkpoint in place, or extend provider timeouts.

### Invoker

An invoker supplies the context for a run. Manual runs, schedules, and future
file/webhook listeners all use the same contract:

- `input`: the trigger payload available to template fields as `trigger.output`;
- `bindings`: per-run role or class overrides for agent selection;
- optional provider and workspace overrides.

Schedules are persisted invokers stored in `library/schedules.json` and managed
by `schedule_create`, `schedule_update`, `schedule_list`, `schedule_pause`,
`schedule_resume`, `schedule_remove`, and `schedule_run_now`. New and updated
schedules require an existing workspace; `schedule_update` changes a schedule
in place and preserves its identity and execution history.

Listeners are persisted invokers stored in `library/listeners.json` and managed
by `listener_list`, `listener_save`, `listener_delete`, `listener_set_enabled`,
`listener_set_webhook_secret`, `listener_set_poll_headers`,
`listener_gateway_config`, and `listener_gateway_save`. One record holds a
tagged `trigger` enum with three variants - `file_watch`, `webhook`, and
`web_poll` - so a fourth event source costs a variant rather than another
invoker family.

Three properties of the listener design are load-bearing:

- **Runtime state is separated from configuration.** Everything the app writes
  lives under `runtime`; the user owns the rest. `enabled` therefore has exactly
  one writer, and the rate ceiling records `runtime.disabled_reason` instead of
  flipping it.
- **Arming is gated on a configuration fingerprint.** Each fire writes `runtime`
  back to the same file, so keying re-arm on file change alone would make a
  listener disarm its own watcher by working.
- **Run identity comes from the event.** A deterministic run id derived from
  `(listener_id, event_identity)` plus a claim lock makes a retried webhook
  delivery, a replayed file burst, and a repeated poll fingerprint resolve to
  the run that already exists.

Credentials live in `library/listener-secrets.json`, separate from the
inspectable listener config. HMAC verification needs the raw secret, so the file
split - not hashing - is what keeps `listeners.json` safe to print and render.

Runs carry `listener_id` in `invocation.json` beside `schedule_id`. The two are
additive rather than one generalized `invoker_id`, because renaming would
invalidate every run directory already on disk.

Design record: [Automation Listener Invokers](https://github.com/wardian-app/Wardian/blob/main/docs/specs/2026-09-05-automation-listener-invokers.md)
(repository-internal; excluded from the public docs build).

### Registry

During execution the engine keeps a registry of run data:

- `nodes.<id>.output`: the output from completed nodes;
- `trigger.output`: the invocation input payload;
- `storage`: persistent storage scoped to this automation run and made available
  to interpolation. It is not shared across runs or agents.

Template fields resolve against this registry before each node executes.

Blueprint validation is fail-closed: decision choices must be unique,
non-empty, valid port identifiers with outgoing edges; edges must name ports
declared by both endpoint node types; loop containers must have a reachable
body; inbound edges from outside a loop body are rejected; and nested loops are
rejected until nested-loop replay semantics exist.

## Execution Flow

1. The frontend or scheduler calls `automation_run` with a blueprint path and
   invocation context.
   The CLI's default `wardian automation exec <path>` path sends the same request
   through the Wardian live control endpoint.
2. The backend parses and validates the blueprint.
3. `LiveStepExecutor` resolves agents, shell/script actions, and app
   notifications. Deterministic state operations run in the core engine.
4. `wardian_core::engine` drives runnable nodes, records events, and checkpoints
   state.
5. Observe and Monitor refresh durable run state through `automation_read_run`.

Resume, startup recovery, and human approval use the same durable run records:

- `automation_resume` resumes an explicitly resumed durable run, such as one
  parked before more work is dispatched;
- app startup marks runs that were still `running` at process exit as `failed`
  with an interruption reason, because their worker tasks and provider
  processes are no longer owned by the new app process;
- detached launch and resume workers persist a `RunFailed` event and failed
  checkpoint if the global headless-execution guard cannot be acquired, so a
  worker-start failure cannot leave a run falsely active;
- scheduler resolution or validation failures persist a one-event terminal
  launch-failure artifact; CLI replay recognizes it without loading a blueprint;
- `automation_approve` grants or rejects an approval gate;
- `automation_cancel` writes a cancellation marker; the engine consumes it at the
  next dispatch boundary, or immediately records a durable `run_failed`
  cancellation event when the run is parked for approval.

The append-only event sequence is validated identically by replay and resume.
New `run_started` events carry the durable run id, allowing recovery to retain
identity when a checkpoint is missing. Observe mode folds branch, decision,
loop, and approval transitions from the same event stream.

New runs persist the parsed blueprint beside the checkpoint and record its
content hash in `run_started`. Resume, replay, and approval continuation reject
invalid or changed graphs, preventing a mutable library edit from changing an
existing run's routing. Approval decisions are bound to the node named by the
durable `awaiting_approval` event. Cancellation of an approval-parked run can
be committed from the checkpoint and event log without loading the library
blueprint.

Startup recovery replays the immutable snapshot before appending its
interruption failure. This folds any valid event-log tail, preserves the
correct next sequence, and refuses to rewrite malformed or unrecoverable runs.
Loop body nodes enter only through the container's `body` port; validation
rejects body nodes that are not reachable from that port. Observe's node
inspector receives the selected event index and does not display future output.

## Agent Execution

Task and decision nodes resolve their `agent` field through the automation
resolver:

- `role:<name>` or `class:<name>` resolves to a headless worker unless an
  invocation binding overrides it;
- explicit active-agent bindings can route a role to a selected agent;
- provider-supplied fresh agents remain available through provider/workspace
  defaults when no active-agent binding is supplied.

Headless execution uses the provider adapters behind
`run_headless_with_options`, with structured output parsed into node outputs.

Active-agent execution uses the visible agent PTY, but completion is still an
explicit structured contract. Wardian creates a task interaction for the
automation node, appends a `wardian reply <request-id> --status ... --stdin`
instruction to the delivered prompt, and waits for that reply before completing
the node. Terminal `idle` status alone is not completion, and a printed
`wardian reply ...` command in the transcript is treated as ordinary assistant
text. `blocked` and `failed` replies fail the node with the reply body as the
diagnostic.

## Old Automation System

The old automation system remains relevant only as migration history and
compatibility cleanup:

- `automation_engine/`
- `AutomationDefinition`
- `run_automation`
- `list_automations`
- `ScheduledRun`
- `scheduled_automations.json`

Do not add new behavior to that surface. New automation behavior belongs in
`wardian-core`, `src-tauri/src/automation/`, and the unversioned automation Tauri
commands.
