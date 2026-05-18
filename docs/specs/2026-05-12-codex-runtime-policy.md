# Codex Runtime Policy

* **Status:** Implemented
* **Date:** 2026-05-12
* **Decider:** Architect

## Context

Wardian can run Codex agents from interactive terminals, CLI tasking, queues, and workflows. Interactive runs can tolerate prompts. Autonomous runs cannot reliably handle Codex approval prompts or Windows sandbox setup prompts because they block progress outside Wardian's workflow engine.

Per-agent Codex sandbox fields remain useful as explicit overrides, but they are not the right place for the default execution posture. Most Wardian automation should inherit a settings-level provider policy.

## Decision

Wardian stores a global Codex runtime policy in `settings/shell.json`:

- `sandbox_mode`
- `approval_policy`
- `full_auto`

The default policy is autonomous:

- `sandbox_mode: danger-full-access`
- `approval_policy: never`
- `full_auto: true`

For current Codex CLI versions, Wardian maps `full_auto: true` to `--dangerously-bypass-approvals-and-sandbox`. On Windows, Wardian also passes `-c windows.sandbox="unelevated"` so a user-level `[windows].sandbox = "elevated"` Codex config cannot launch a UAC setup helper during tool execution. Wardian does not pass the older unsupported `--full-auto` flag.

Agent-level Codex `provider_config.sandbox_mode`, `provider_config.approval_policy`, and `provider_config.full_auto` remain explicit overrides. Legacy flat fields such as `codex_sandbox_mode`, `codex_approval_policy`, and `codex_full_auto` still deserialize into the nested Codex provider config for compatibility. If an agent explicitly sets sandbox or approval values, Wardian uses those values instead of the global full-auto default. If an agent explicitly enables full auto, Wardian uses the bypass flag.

## Consequences

- Regular agents, CLI-launched clones, queues, and workflows share the same Codex default unless an agent overrides it.
- Default Codex launches avoid Windows sandbox setup prompts and inline approval prompts.
- Users can still opt individual agents into more restrictive Codex modes through advanced agent settings.
- The policy is intentionally visible in Settings so blank per-agent fields no longer look equivalent to full access.

## Verification

- Provider unit tests cover the default bypass flag, the Windows sandbox-backend override, and explicit per-agent policy overrides.
- Settings tests cover loading, defaulting, and saving the Codex runtime policy.
- Native release verification should include a Codex launch on Windows and confirm the logged args include `--dangerously-bypass-approvals-and-sandbox` when the default policy is active.
