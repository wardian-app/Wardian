# Codex Computer Use Policy and Isolated Home Reconciliation

## Goal

Allow the official `computer-use@openai-bundled` Codex plugin only for
Electrical Engineer and Mechanical Engineer agents, without weakening Codex
approval controls or sharing mutable Codex state between Wardian agents.

## Scope

This change is limited to the Codex provider's policy resolution, per-agent
home reconciliation, launch arguments, diagnostics, and verification. It does
not introduce a plugin-management UI, change the process-wide `CODEX_HOME`, or
allow all Codex plugins for all agents.

## Current Failure

Wardian launches every interactive Codex session with:

```text
--disable plugins --disable apps
```

Those global feature disables suppress an otherwise installed and enabled
Computer Use plugin. The existing per-agent habitat has the correct isolation
boundary, but its `config.toml` is copied from the user's global Codex home on
every projection. That is not a managed base plus preserved per-agent overlay.

## Design

### Plugin policy

Introduce a Codex plugin policy resolver with a default-deny allowlist.

- `Electrical Engineer` and `Mechanical Engineer` receive
  `computer-use@openai-bundled`.
- All other classes, including `Coder`, receive no Codex plugins by default.
- An allowed-plugin record includes whether it requires Codex's app surface.
  Computer Use requires that surface.
- The policy is expressed in Wardian-owned configuration and can support
  future class and agent overrides, but this slice seeds only the two named
  class defaults.

The resolver is the only source used by home reconciliation, process launch,
and diagnostics. This prevents an installed plugin from silently becoming
available to an unallowlisted agent.

### Per-agent Codex home

Each agent continues to receive its own habitat `CODEX_HOME` at:

```text
<WARDIAN_HOME>/agents/<agent-id>/habitat/.codex
```

Reconciliation has three distinct inputs:

1. A Wardian-managed Codex base supplies shared defaults, marketplace sources,
   approved plugin versions, and shared MCP definitions.
2. A preserved agent overlay supplies that agent's trusted projects and any
   agent/project-specific Codex configuration.
3. Wardian writes only its managed configuration keys and plugin install state
   derived from the effective allowlist.

The reconciler must not copy or delete mutable agent state. In particular it
must preserve `sessions/`, `history.jsonl`, logs and state SQLite files plus
sidecars, memories, goals, workspace trust, and agent/project overrides. It
must not hardlink those paths to a global Codex home.

Authentication bootstrap material and immutable marketplace/plugin implementation
files may be projected from a shared cache when Codex supports it. The list of
installed/enabled plugins remains in the individual agent home and is reconciled
only to the effective allowlist.

### Plugin installation

When reconciliation discovers an allowlisted plugin is absent, it invokes the
Codex plugin installer with the target agent's `CODEX_HOME`, using the exact
approved selector `computer-use@openai-bundled`. It never installs a plugin for
an unallowlisted class. Marketplace implementation bits may remain in Codex's
immutable/shared cache; the target home remains the installed/enabled authority.

An install failure leaves the home intact and is reported by diagnostics with
the installer error. It does not fall back to a globally enabled plugin.

### Launch behavior

Interactive Codex launch arguments are derived from the resolved policy.

- With no allowed plugins, retain `--disable plugins --disable apps`.
- With an allowed plugin that does not require apps, omit `--disable plugins`
  but retain `--disable apps`.
- With an allowed plugin that requires apps, omit both disable flags.

`--no-alt-screen`, the existing sandbox selection, and the ordinary Codex
approval policy continue unchanged. This change must not use
`--dangerously-bypass-approvals-and-sandbox` and does not suppress Computer
Use confirmations or user interruption controls.

The bundled Computer Use manifest is interactive and its runtime uses the
privileged Computer Use channel. Therefore it is marked as app-surface
dependent and receives neither disable flag.

### Refresh and restart semantics

Reconciliation writes a non-sensitive policy fingerprint into the agent
habitat. A running session records the fingerprint used at launch. If the
effective policy or managed base changes after launch, diagnostics report
`restart_required: true`; the existing session is never assumed to gain tools.
A fresh agent session reconciles its home before Codex starts and uses the new
fingerprint.

Class updates keep their existing `restart_required` behavior. The diagnostic
surface makes the policy-triggered case visible even when the agent's persisted
configuration did not change.

### Diagnostic surface

Add a read-only `wardian agent doctor <target>` route for Codex agents. It
returns structured JSON containing:

- effective `CODEX_HOME`;
- effective allowed plugins and their required surfaces;
- discovered installed/enabled plugins in that home;
- interactive launch flags;
- active and current policy fingerprints plus `restart_required`;
- diagnostic reasons for an expected plugin being absent, such as
  `not_allowlisted`, `plugins_feature_disabled`, `apps_feature_disabled`,
  `not_installed`, `installer_failed`, or `restart_required`.

The command does not return authentication credentials, config contents, or
private session data. Non-Codex agents return a typed not-applicable response.

## Acceptance Criteria

1. A fresh Electrical Engineer or Mechanical Engineer Codex session has
   `computer-use@openai-bundled` installed and enabled in its own agent
   `CODEX_HOME`, with neither plugin nor app feature disabled at launch.
2. An opt-in native real-provider test starts a fresh eligible session and
   proves the Computer Use skill/tool is available through a harmless
   capability query that does not operate an application.
3. A normal Coder session has no Computer Use installation or exposure and
   retains both global feature-disable arguments.
4. Reconciliation after a managed-base or policy update preserves each
   agent's sessions, history, SQLite state, memories, goals, workspace trust,
   and unowned project/agent configuration; it reports restart required for a
   session launched under the prior policy fingerprint.
5. Diagnostics report the effective home, policy, install status, launch flags,
   restart state, and a precise absence reason without revealing credentials or
   state content.

## Verification Strategy

Unit tests cover policy resolution, class defaults, launch argument selection,
TOML managed-key reconciliation, no-overwrite state preservation, plugin
installer request construction, fingerprint mismatch, and diagnostic reasons.
CLI/control tests verify `wardian agent doctor` serialization and target
resolution. Native tests use an isolated `WARDIAN_HOME`; the real-provider
Computer Use capability check remains explicitly opt-in because it requires a
locally available Codex runtime and the bundled plugin surface.

Run the relevant Rust tests during development, then run `cargo clippy`,
`cargo test`, and `cargo check` from `src-tauri` before handoff. Update provider
documentation to explain class-scoped plugin policy and the diagnostic command.
