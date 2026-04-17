# Spec 019: Session Persistence Policy

- **Status:** Proposed
- **Date:** 2026-04-17
- **Decider:** User

## Context and Problem Statement

Wardian's provider-native session persistence is useful for interactive agents, but it is too expensive as a default for workflow automation. When workflow runs reuse a provider session, each run can inherit growing context and increase token usage without an explicit user decision.

The existing workflow model also overloads the word "persistent." A persistent workflow agent node can mean "target an existing Wardian agent," "use that agent's provider/runtime configuration," and "resume that provider's conversation history." Those are separate concerns.

Wardian should preserve the ability to resume live agents while making workflow execution fresh by default. Workflows should be able to use an existing agent as a profile template without automatically mutating or extending that live agent's provider session.

## Proposed Decision

### Use One Workflow Agent Run Mode

Wardian will model workflow agent execution with one workflow-facing selector:

| Run mode | Meaning |
| --- | --- |
| `ephemeral` | Build a fresh execution from an agent class plus node-specific config. This is the current "temporary" workflow behavior, renamed and clarified. |
| `inherit_fresh` | Clone provider/runtime settings from an existing Wardian agent, but start a fresh provider session in a workflow-run runtime directory. |
| `inherit_resume` | Resume the selected agent's provider session and intentionally reuse its mutable runtime directory. |

The backend can still decompose the selected run mode into internal source and persistence concepts, but workflow JSON should not expose a separate `session_persistence` field. This avoids invalid combinations such as `ephemeral + resume`.

### Default Behavior

Defaults should reduce accidental token growth:

| Surface | Default |
| --- | --- |
| Interactive agent restart | `resume` |
| Manual workflow run | `fresh` |
| Scheduled workflow run | `fresh` |
| File watcher or listener workflow | `fresh`, unless explicitly configured |
| OpenClaw or future background-agent runtime | `resume` by design, because the runtime is explicitly long-lived |

This keeps live agents useful while making workflow runs artifact-oriented and reproducible by default.

### Workflow Agent Node UX

The workflow agent node should replace the overloaded `session_type` language with clearer fields:

| Field | Values | Notes |
| --- | --- | --- |
| `mode` | `ephemeral`, `inherit_fresh`, `inherit_resume` | Replaces the current user-facing distinction between temporary and persistent. |
| `agent_class` | class name | Used when `mode = ephemeral`. |
| `agent_id` | session ID | Used when `mode = inherit_fresh` or `mode = inherit_resume`. |

For backwards compatibility, legacy workflow nodes should be migrated in memory:

| Legacy value | New source | New persistence |
| --- | --- | --- |
| `session_type = temporary` | `ephemeral` | fresh provider session |
| `session_type = persistent` | `inherit_fresh` | fresh provider session |

If preserving exact old behavior is required during migration, persistent legacy nodes can be upgraded to `inherit_resume` behind a one-time compatibility flag. The preferred product behavior is `inherit_fresh`, because the user goal is to stop accidental token growth.

### Backend Execution Context

The Rust backend remains the authority for provider lifecycle decisions. Workflow execution should resolve a backend-owned execution context before launching a provider:

```rust
pub enum WorkflowAgentMode {
    Ephemeral,
    InheritFresh,
    InheritResume,
}

pub struct AgentExecutionContext {
    pub config: AgentConfig,
    pub mode: WorkflowAgentMode,
    pub resume_session: Option<String>,
}
```

Resolution rules:

1. Determine agent mode from the workflow node.
2. If `inherit_fresh` or `inherit_resume`, clone the target agent's runtime config.
3. Apply explicit workflow node overrides such as prompt, output format, timeout, and folder.
4. If mode is `ephemeral`, build a class/config execution, clear `resume_session`, and use a workflow-run runtime directory.
5. If mode is `inherit_fresh`, clear `resume_session`, include the source agent as read scope, and use a workflow-run runtime directory.
6. If mode is `inherit_resume`, pass the provider-native resume ID only when it is valid for that provider and use the source agent's mutable runtime directory.
7. Persist workflow output as workflow run artifacts, not as implicit provider memory.

`inherit_fresh` runs share the source agent as read scope for instructions, skills, and scoped memory, but receive a separate workflow-run runtime directory. Only `inherit_resume` intentionally reuses the source agent's mutable runtime directory and provider session.

### Startup Prompt Policy

Wardian should treat "introduce yourself" startup prompts as an interactive-agent bootstrap behavior, not as a universal fresh-session behavior.

Workflow-spawned runs should skip introductory prompts:

| Run mode | Startup prompt behavior |
| --- | --- |
| `ephemeral` | Do not send an intro prompt. The first provider input is the workflow node prompt. |
| `inherit_fresh` | Do not send an intro prompt. The run inherits profile scope, not conversational warm-up. |
| `inherit_resume` | Do not send an intro prompt unless the workflow prompt explicitly asks for that context. |

Regular visible agents may still send an intro prompt on fresh interactive launch when provider behavior benefits from it. Resume launches should skip it.

The launch context should keep these decisions separate:

```rust
pub enum LaunchSurface {
    InteractiveAgent,
    WorkflowNode,
    ScheduledWorkflow,
    NativeE2E,
}

pub enum StartupPromptDecision {
    SendIntro,
    SkipIntro,
}
```

Provider adapters may expose whether they need an interactive bootstrap prompt for readiness, but headless workflow runs should default to `SkipIntro`. This avoids spending tokens on non-task output and prevents intro text from polluting structured workflow results.

### Provider Semantics

Provider adapters remain responsible for translating a resolved resume ID into provider-specific flags:

| Provider | Resume behavior |
| --- | --- |
| Gemini | `--resume <session_id>` only for `inherit_resume`. |
| Claude | `--resume <session_id>` only for `inherit_resume`; do not resend fresh-session identity on resume. |
| Codex | `resume <session_id>` or headless resume form only for `inherit_resume`; fresh runs must not use the live provider ID. |
| OpenCode | `--session <session_id>` only for `inherit_resume`; fresh runs should still receive injected instructions and skill paths through `OPENCODE_CONFIG_CONTENT`. |

OpenCode is the clearest example of why the distinction matters: Wardian can clone OpenCode provider config and injected skills without automatically appending another run to the same OpenCode `sessionID`.

### Settings

Add settings at two levels:

1. **Global workflow default**: default run mode for newly created workflow agent nodes.
2. **Agent default**: optional preferred inherited run mode when this agent is selected in workflow builder surfaces.

The agent-level setting should not override an explicit workflow node choice. Initial implementation may omit these settings and default new workflow nodes to `ephemeral`, with legacy persistent nodes migrating to `inherit_fresh`.

### Documentation

Update user-facing workflow docs to explain:

- "Agent class" creates a fresh execution from class/config.
- "Existing agent profile" clones a live agent's settings.
- "Fresh session" avoids growing provider context.
- "Resume provider session" is useful for deliberate continuity but can increase token use.

Update developer docs to preserve the key invariant: provider-native resume state is runtime state, not Wardian memory.

Memory read/write behavior is intentionally out of scope for this spec except for one boundary: `inherit_fresh` may read source-agent scoped memory, but it should write workflow artifacts only. Promotion into source-agent memory belongs to [Spec 015](./015-evidence-first-memory.md).

## Consequences

- **Positive**: Workflow runs no longer accidentally consume growing provider context by default.
- **Positive**: Existing agents can still be used as reusable execution profiles.
- **Positive**: Interactive agents keep the expected resume behavior.
- **Positive**: The model aligns with Wardian's evidence-first memory direction by storing workflow artifacts instead of replaying provider transcripts.
- **Positive**: OpenClaw and future background-agent runtimes get a clear place as explicit long-lived agents.
- **Negative**: Existing workflows that relied on implicit live-session continuity may need an explicit `resume` setting.
- **Negative**: The workflow node UI and backend execution paths need migration logic for legacy `session_type` nodes.
- **Negative**: Fresh workflow runs may lose provider-native context that some users found convenient; that context should be promoted into explicit workflow inputs, memory nodes, or retrieved evidence instead.
