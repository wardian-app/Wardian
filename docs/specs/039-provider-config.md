# Spec 039: Provider Config Split

- **Status:** Implemented
- **Date:** 2026-05-13
- **Decider:** Architect

## Context and Problem Statement

`AgentConfig` previously mixed shared agent fields with provider-specific launch
options for Claude, Gemini, Codex, and OpenCode. That made it easy for stale
fields from one provider to remain on an agent after provider changes, clones,
or legacy state loads. It also meant provider adapters had to know which flat
top-level fields belonged to them.

Wardian needs a provider-owned configuration boundary while preserving old
persisted agent configs.

## Decision

Keep shared agent/session/workspace fields on `AgentConfig` and move
provider-owned launch settings into `provider_config`.

`provider_config` is an internally tagged JSON object:

```json
{
  "provider": "codex",
  "model": "gpt-5.4",
  "include_directories": ["<absolute-workspace-path>/shared"],
  "custom_args": "--experimental-flag",
  "provider_config": {
    "type": "codex",
    "sandbox_mode": "workspace-write",
    "approval_policy": "never",
    "profile": "wardian",
    "full_auto": true,
    "search": true,
    "skip_git_repo_check": false,
    "ephemeral": true,
    "cleared_provider_sessions": ["old-provider-thread"]
  }
}
```

Provider examples:

```json
{ "provider": "claude", "provider_config": { "type": "claude", "permission_mode": "plan", "max_turns": 20 } }
```

```json
{ "provider": "gemini", "provider_config": { "type": "gemini", "sandbox": true, "approval_mode": "auto_edit", "output_format": "json" } }
```

```json
{ "provider": "opencode", "provider_config": { "type": "opencode", "agent": "build", "port": 4096 } }
```

```json
{ "provider": "mock", "provider_config": { "type": "mock" } }
```

The top-level `provider` remains the launch provider. New configs must have a
matching `provider_config.type`. Explicit IPC updates with mismatched provider
config are rejected. Compatibility loads normalize mismatches to a default
provider config for the selected provider so wrong-provider fields are never
used to assemble launch arguments.

## Compatibility

Legacy flat persisted fields still deserialize. For example,
`codex_sandbox_mode` loads into `provider_config: { "type": "codex",
"sandbox_mode": ... }`.

Routine whole-state saves preserve legacy-flat serialization for agents that
were loaded from legacy flat state. Wardian emits the nested shape only for new
agents, clones, explicit config updates, or a future explicit migration path.

If both nested and flat fields are present, nested `provider_config` wins.

## Runtime Fields

- `fresh_provider_session_id` remains runtime-only and is not serialized.
- `codex.cleared_provider_sessions` stays in Codex provider config because it
  affects persisted resume-clearing behavior. Clone sanitization clears it.
- `opencode.port` stays in OpenCode provider config for reconnect
  compatibility. Clone sanitization clears it.
- `custom_args` remains shared because it is a raw provider escape hatch, but it
  is scoped to the selected provider. Provider changes and cross-provider clones
  clear it unless the user re-enters it.

## Provider Output

Provider output parsing and `AgentEvent` handling are independent of launch
config. Moving provider options into `provider_config` does not change JSON log
parsing, status transitions, or terminal output handling.

## Consequences

- Provider adapters read typed provider config accessors and ignore unrelated
  provider settings by construction.
- Frontend forms edit only settings that belong to the selected provider.
- The Rust serde contract and TypeScript union share the same
  `{ "type": "<provider>", ...snake_case_fields }` wire shape.
- Old state files remain readable without a mandatory migration rewrite.
