# Agent Cloning

## Status

Proposed

## Context

Wardian users can already spawn, pause, resume, clear, rename, group, and delete agents from the roster context menu. They also manage reusable identity through common, class, and instance-level `AGENTS.md` files plus deployed skills. Creating another agent with the same setup is currently manual: users must reopen the spawn panel, copy provider settings, choose the same workspace, and optionally recreate instance-level files or skills.

Cloning should preserve Wardian's physical-first model without accidentally carrying opaque provider conversation state. Provider sessions, logs, generated habitats, and telemetry are runtime artifacts. Agent classes, settings, workspace paths, instance instructions, and instance skills are user-authored profile data.

## Goals

- Add a single-agent clone action to the agent right-click menu.
- Keep the top-level context menu compact.
- Make the default clone behavior fast and safe.
- Distinguish visible setup/profile cloning from provider conversation memory.
- Keep clone creation backend-owned so session IDs, provider bootstrap, saved state, and habitat projection remain authoritative in Rust.

## Non-Goals

- Bulk clone for multi-selected agents or teams.
- Provider conversation/session memory cloning.
- Cross-provider conversion rules beyond copying supported config fields.
- A settings preference for the default clone action in the first iteration.

## User Experience

The shared `AgentContextMenu` gets one top-level item:

- `Clone`

The item is both clickable and expandable:

- Clicking `Clone` directly runs `Fresh Clone`.
- Hovering opens:
  - `Fresh Clone`
  - `Profile Clone`
  - `Custom...`

For the first implementation, clone is available only when exactly one agent is targeted. Bulk selections and team context menus should hide or disable cloning.

If `Custom...` is not implemented in the same change, omit it rather than shipping a dead-end menu item.

## Clone Modes

### Fresh Clone

Fresh clone creates another agent with the same visible setup and a clean provider conversation.

It carries:

- `agent_class`
- `folder`
- provider
- model and provider settings
- sandbox, approval, permission, output, and custom argument settings
- include directories
- session persistence preference
- git worktree setting
- global and class skills, by virtue of using the same class and normal include resolution

It resets or regenerates:

- `session_id`
- `session_name`, using a unique copy name such as `<source>-copy` or `<source>-copy-2`
- `resume_session`
- `fresh_provider_session_id`
- `is_off`
- provider session exclusion lists and other runtime-only resume helpers
- telemetry, terminal title, query count, log path, and status

It does not copy:

- provider conversation memory
- generated `habitat/`
- provider history/session files
- logs
- permission request logs
- instance-local `AGENTS.md`
- instance-local `.agents/skills`

### Profile Clone

Profile clone does everything Fresh Clone does and also copies whitelisted agent-local profile files from `WARDIAN_HOME/agents/<source_session_id>/` into the new agent directory.

Initial whitelist:

- `AGENTS.md`
- `.agents/skills`

The copy must exclude runtime and generated data, including:

- `habitat/`
- provider session/history directories and files
- `claude/permission-requests.jsonl`
- logs
- telemetry or database-derived state

Profile clone still starts a fresh provider conversation.

### Custom Clone

Custom clone opens a compact wizard for users who want to change name, provider, class, workspace, or clone mode before creation. This can be implemented after Fresh/Profile clone unless the UI work is included in the same change.

## Backend Design

Add a Tauri command named `clone_agent`.

Suggested request shape:

```ts
{
  source_session_id: string;
  mode: "fresh" | "profile";
  session_name?: string;
  provider?: string;
  folder?: string;
  agent_class?: string;
  start?: boolean;
}
```

The command should:

1. Load the source `AgentConfig` from active state.
2. Build a sanitized clone config.
3. Generate a unique clone name when none is provided.
4. Call the same backend spawn path used by normal agent creation.
5. For profile clone, copy whitelisted profile files after the new agent directory exists and before provider habitat projection needs them.
6. Save state and emit the normal agent update events.

Sanitization must be centralized and unit tested. It should never reuse source `session_id` or provider conversation identifiers.

## Frontend Design

`AgentContextMenu` should accept an optional clone handler:

```ts
onClone?: (agentId: string, mode: "fresh" | "profile") => MaybePromise;
```

The menu should render clone only for a single agent context. The parent click calls `onClone(agentId, "fresh")`; submenu items call `fresh` or `profile`.

`App.tsx` owns the handler and invokes `clone_agent`, then refreshes agents.

## Testing

Backend:

- Unit test clone config sanitization.
- Unit test unique clone name generation.
- Unit test profile copy whitelist and runtime exclusions.
- Command-level test if practical with isolated `WARDIAN_HOME`.

Frontend:

- Context menu shows `Clone` for a single agent.
- Parent click invokes fresh mode.
- Submenu invokes fresh/profile modes.
- Bulk and team context menus do not expose an active clone action.

E2E:

- Browser E2E with mock provider for roster right-click fresh clone if the mock layer can prove the behavior.
- Native E2E only if validating real provider spawn, filesystem projections, or PTY behavior.

