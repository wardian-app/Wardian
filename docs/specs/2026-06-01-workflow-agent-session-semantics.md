# Workflow Agent Session Semantics

- **Status:** Implemented
- **Date:** 2026-06-01
- **Branch:** `debug/v2-workflow-crash`
- **Scope:** Workflow agent assignment, background execution, conversation leases, and `Headless` status semantics.

## 1. Problem

Workflow can bind a node role such as `reasoning_gate` to a provider, a saved
Wardian agent profile, or a currently running agent. The current implementation
does not express those choices precisely enough. It can treat "use Assistant" as
either "send work to Assistant's open terminal" or "run Assistant's saved profile
headlessly" depending on online/offline state. That makes the UI hard to explain
and creates a real concurrency hazard: two processes can try to use the same
provider conversation transcript at the same time.

The underlying model needs to distinguish:

- the selected Wardian agent identity/profile;
- the runtime route used for this workflow step;
- whether the provider conversation is the agent's canonical saved conversation
  or a fresh temporary conversation;
- the status signal other Wardian providers should observe while a saved
  conversation is occupied outside the visible terminal.

## 2. Goals

This spec defines one workflow-native model for agent assignment that is easy to
explain and safe to execute.

The model should:

- let users assign a workflow role to a named Wardian agent without learning
  backend terms first;
- preserve the path for workflow-supplied fresh/headless provider workers;
- preserve the path for using a saved agent profile in the background;
- prevent transcript collisions on a saved provider conversation;
- define `Headless` narrowly enough that other Wardian providers can rely on it;
- support scheduled/passive workflows such as passive heartbeat without
  confusing visible agent status;
- leave room for mailbox-based offline prompting later.

## 3. Non-Goals

- Do not rename providers or provider-specific session identifiers.
- Do not remove headless provider execution.
- Do not make every background run visible in the agent roster.
- Do not use `Headless` as a generic synonym for all background work.
- Do not require workflows to spawn or open a visible agent terminal before they
  can use a saved agent profile.
- Do not fully implement mailbox-based offline prompting in this spec. It is a
  future sibling route.

## 4. User-Facing Model

Workflow authoring should ask where a role should get its agent capability from,
not whether the executor is live, persisted, headless, or offline.

Primary choices:

1. **Use an agent**
   - Select an existing Wardian agent such as `Assistant` or `Trader`.
   - The workflow uses that agent's identity, saved setup, workspace, provider,
     tools, instructions, and conversation policy.
   - Wardian chooses an execution route that respects conversation safety.

2. **Use a new temporary agent**
   - Select a provider and workspace for this workflow role.
   - The run uses a workflow-owned temporary background conversation.
   - No named Wardian agent status is occupied.

When a role uses an existing agent, the user can choose a conversation policy:

1. **Use the current conversation**
   - Prefer the visible/open agent session when it is input-ready.
   - If the agent is off, the workflow may use the saved provider conversation
     in the background.
   - If the agent is busy, the workflow must not start a parallel background
     resume against the same provider conversation.

2. **Start a separate background conversation**
   - Use the selected agent's profile and workspace, but do not reuse the
     agent's saved provider conversation.
   - This is safe even if the visible agent is open or processing.
   - The original agent's `Headless` status is not set.

The UI should avoid asking users to choose "headless" directly. `Headless` is a
runtime status, not an authoring mode.

## 5. Internal Execution Modes

The engine should normalize user choices into explicit execution modes before a
run starts.

### 5.1 `agent_current_conversation`

Use a selected Wardian agent and its canonical provider conversation.

Routes:

- **Open route:** if the agent is live and input-ready, submit the prompt through
  the visible agent PTY/mailbox delivery path and wait for completion.
- **Background resume route:** if the agent is not live/offline, acquire a
  conversation lease and run the saved provider conversation headlessly.
- **Busy route:** if the agent is live but processing/action-required, do not
  background-resume the same saved conversation. The workflow applies its busy
  policy: wait, queue, skip/defer, or fail.

This mode may set `Headless`, but only when it takes the background resume route.

### 5.2 `agent_background_fresh`

Use a selected Wardian agent's saved profile without using its saved provider
conversation.

Route:

- Run headlessly with the selected agent's provider, workspace, instructions,
  profile settings, and projected habitat material, but without the saved
  provider `resume_session`.

This mode never sets `Headless` on the selected agent because it does not occupy
the agent's canonical provider conversation.

### 5.3 `temporary_provider`

Use workflow-supplied provider/workspace/config without selecting a Wardian
agent identity.

Route:

- Run headlessly with a workflow-owned session id and fresh provider
  conversation.

This mode never sets `Headless` on a named Wardian agent.

### 5.4 Future: `agent_mailbox`

Queue work for an agent to handle later through Wardian's mailbox.

Route:

- Persist an offline prompt/mailbox item for the named agent.
- The agent consumes it when it becomes live and input-ready.

This mode does not set `Headless` because no provider conversation is actively
being used by a background executor.

## 6. Status Semantics

`Headless` remains a Wardian agent status, but its meaning is narrow:

> **Headless** means this Wardian agent's saved provider conversation is
> actively leased by a background executor.

`Headless` applies only to `agent_current_conversation` when it uses the
background resume route.

`Headless` does not apply to:

- a visible/open agent working in its terminal (`Processing`);
- a fresh background conversation created from an agent profile;
- a temporary provider worker;
- a queued mailbox prompt;
- generic workflow background execution.

The roster should show `Headless` for the named agent whose saved provider
conversation is leased. Workflow observe/monitor surfaces should show the
workflow run/node that owns the lease.

If the visible agent terminal is open while a background resume lease exists,
the visible agent must not accept new work for that saved conversation until the
lease is released. The UI can show the agent as `Headless` with context such as:

```text
Assistant
Headless
Passive Heartbeat using saved conversation
```

If a fresh background conversation is running from Assistant's profile, the
roster should not replace Assistant's visible status with `Headless`. A secondary
badge such as "1 background run" is acceptable, but it must not imply that the
canonical saved conversation is occupied.

## 7. Conversation Lease

Background resume requires an explicit lease. A lease represents exclusive use
of a Wardian agent's saved provider conversation outside the visible terminal.

Recommended persisted shape:

```json
{
  "schema": 1,
  "leases": [
    {
      "agent_id": "fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2",
      "provider": "gemini",
      "resume_session": "610b3a93-b82f-4cf6-a0c9-ed3ccbd704b0",
      "owner_kind": "workflow_run",
      "owner_id": "passive-heartbeat/1780278501503-704eeeca",
      "owner_node_id": "agent-1",
      "mode": "background_resume",
      "started_at": "2026-06-01T01:48:21Z",
      "heartbeat_at": "2026-06-01T01:50:21Z",
      "expires_at": "2026-06-01T02:08:21Z"
    }
  ]
}
```

Lease rules:

1. Acquire before launching a background resume.
2. Refuse, wait, or skip if a non-expired lease already exists for the same
   `agent_id` or the same non-empty `resume_session`.
3. Set the agent's externally visible status to `Headless` while the lease is
   active.
4. Refresh `heartbeat_at` while the executor is alive.
5. Release on successful completion, failure, or cancellation.
6. Recover stale leases on app start by checking expiry and run state.
7. Do not acquire a lease for background fresh or temporary provider routes.

The lease protects the provider conversation, not only the Wardian UUID. If two
Wardian profiles somehow point at the same provider `resume_session`, only one
may use it at a time.

## 8. Busy Policies

When `agent_current_conversation` targets an agent that is live but not
input-ready, the workflow must not silently background-resume the saved
conversation. The run should apply a policy:

- **Wait:** park the workflow node until the agent becomes input-ready, then use
  the open route.
- **Queue:** enqueue through the existing delivery/mailbox path if the workflow
  is allowed to wait behind current work.
- **Skip/defer:** record that this scheduled/passive invocation did not run
  because the agent was busy.
- **Fail:** fail the node/run with a clear reason.

Recommended defaults:

- manual run: `fail` until wait/queue parking is implemented;
- scheduled/passive run: `skip/defer`;
- explicitly authored blocking workflow: `queue` or `wait` once those policies
  are backed by durable workflow parking;
- automation that must complete in this run: `fail`.

The policy should be part of the run/schedule invocation context, not inferred
from provider status alone.

Current implementation note: the run dialog exposes only `skip` and `fail`.
`wait` and `queue` remain part of the wire model for compatibility and future
parking work, but they must not be presented as functional UX until implemented.

## 9. Data Model

Current workflow schedule/run bindings use a map such as:

```json
{
  "reasoning_gate": "fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2"
}
```

That shape is ambiguous. The new model should support structured role
assignments while continuing to read legacy bindings.

Recommended shape:

```json
{
  "assignments": {
    "reasoning_gate": {
      "target_type": "agent",
      "agent_id": "fb7107aa-4fd1-411f-b6bb-9c5a306d5ae2",
      "conversation": "current",
      "busy_policy": "skip"
    },
    "builder": {
      "target_type": "agent",
      "agent_id": "77d49592-02e2-440f-87e2-ae9143ed60dc",
      "conversation": "fresh_background"
    },
    "summarizer": {
      "target_type": "temporary_provider",
      "provider": "codex",
      "workspace": "<absolute-workspace-path>"
    }
  }
}
```

Field meanings:

- `target_type`: `agent` or `temporary_provider`.
- `agent_id`: selected Wardian agent profile.
- `conversation`: `current` or `fresh_background`.
- `busy_policy`: `wait`, `queue`, `skip`, or `fail`.
- `provider` / `workspace`: only for temporary provider assignments.

Legacy binding compatibility:

- A binding value that matches a provider name maps to
  `temporary_provider(provider=<value>)`.
- A binding value that matches an agent id maps to
  `agent_current_conversation(agent_id=<value>)`.
- Legacy scheduled invocations should default busy policy to `skip`.
- Legacy manual invocations should default busy policy to `fail` until durable
  wait/queue parking is implemented.

## 10. Runtime Flow

For each task/decision node:

1. Resolve the role's structured assignment.
2. Resolve the selected agent profile or provider config.
3. Select the route:
   - temporary provider -> headless fresh;
   - agent + fresh background -> headless fresh from profile;
   - agent + current conversation + live input-ready -> live/open route;
   - agent + current conversation + offline -> acquire lease, then background
     resume;
   - agent + current conversation + busy -> apply busy policy.
4. Execute the route.
5. For the live/open route, create a task interaction and require the target
   agent to answer with `wardian reply <request-id> --status ... --stdin`.
   Terminal `idle` is not completion by itself; it only enables transcript
   marker compatibility checks. `blocked` and `failed` replies fail the node.
6. Parse output using existing workflow task/decision output semantics.
7. Release leases and update schedule/run status.

## 11. UI Placement

### Run/Schedule Dialog

Use assignment language:

```text
Agent assignments

Reasoning Gate
Agent: Assistant
Conversation: Current conversation
When busy: Skip this run
```

For a fresh profile run:

```text
Reasoning Gate
Agent: Assistant
Conversation: Separate background conversation
```

For a temporary provider run:

```text
Reasoning Gate
Temporary agent: Gemini
Workspace: <absolute-workspace-path>
```

### Observe/Monitor

Show both the selected agent and the route actually taken:

```text
Reasoning Gate
Assistant
Route: Background resume
Status: Headless
```

or:

```text
Reasoning Gate
Assistant
Route: Open session
Status: Processing
```

### Agent Roster

`Headless` should appear only when an active conversation lease exists. A fresh
background run from an agent profile may be shown as a secondary activity badge,
but must not overwrite the visible/canonical conversation status.

## 12. Migration

Existing workflow bindings and schedules remain valid. Migration can be lazy:

- read legacy `bindings` into structured assignments at runtime;
- write structured `assignments` for new or edited schedules;
- preserve old `bindings` fields until all call sites and CLI paths understand
  assignments;
- add a later cleanup pass to stop writing `bindings`.

The migration must not silently change existing schedules from resume behavior
to fresh behavior. If a legacy binding points to an agent id, it means
`agent_current_conversation`.

## 13. Testing

Unit tests:

- resolve provider binding -> temporary provider mode;
- resolve agent binding -> agent current conversation mode;
- live input-ready agent chooses open route;
- offline agent chooses background resume and acquires lease;
- busy live agent does not choose background resume;
- background fresh never uses `resume_session` and never sets `Headless`;
- lease acquisition rejects same `agent_id`;
- lease acquisition rejects same `resume_session` across different agent ids;
- stale leases are expired/recovered.

Integration tests:

- scheduled passive heartbeat against an offline agent profile sets `Headless`
  while running and releases it on completion;
- scheduled passive heartbeat against a busy live agent is skipped/deferred by
  default;
- manual workflow against a busy live agent fails clearly by default, and later
  waits or queues according to policy once those policies are implemented;
- background fresh from a busy live agent profile completes without setting
  `Headless`;
- observe/monitor surfaces show route and status.

Native E2E tests:

- live open route submits to a visible provider terminal;
- background resume does not submit to a visible terminal and blocks conflicting
  live submission while leased;
- app restart expires or recovers stale leases without leaving the agent stuck
  in `Headless`.

## 14. Consequences

- **Positive:** Users can say "use Assistant" without learning provider session
  internals.
- **Positive:** `Headless` becomes a reliable coordination signal for Wardian
  providers.
- **Positive:** Scheduled/passive workflows can safely use saved agent
  conversations without colliding with visible work.
- **Positive:** Fresh background work from an agent profile remains available
  without occupying the named conversation.
- **Negative:** The assignment model becomes more explicit and requires UI,
  schema, CLI, and backend migration work.
- **Negative:** Busy-policy behavior must be surfaced carefully so scheduled
  workflows do not appear to "do nothing" without explanation.
- **Negative:** Conversation leases introduce another persisted runtime state
  that needs crash recovery.
