# Settings Defaults and Overrides

## Goal

Wardian settings must distinguish computed defaults from user-set values. This
lets Wardian change safer or more appropriate defaults later without
overwriting values a user explicitly chose.

## Storage Model

Settings files under `<WARDIAN_HOME>/settings/` use a versioned sparse override
document:

```json
{
  "schema_version": 2,
  "overrides": {
    "default_provider": "codex"
  }
}
```

The effective runtime setting is computed as:

```text
effective setting = defaults for current app/OS context + user overrides
```

Absent keys track Wardian's current computed default. Present keys are
user-set, even if the value equals the current default.

## Files

- `settings/app.json` stores app preference overrides such as theme, terminal
  font size, terminal font family, and Gemini auto-patch.
- `settings/shell.json` stores runtime overrides such as shell selection,
  default provider, regular agent session policy, and Codex runtime policy.

Runtime consumers continue to receive resolved effective settings. The Settings
UI receives both the resolved settings and sparse overrides so saving one row
does not accidentally materialize every default into a user override.

## Platform Defaults

Defaults are computed, not fixed JSON constants. They may depend on OS or other
local runtime facts. For example, terminal font size and family can differ by
platform while the persisted file still stores only user intent.

Settings should store symbolic choices where possible. A default shell should
track the current platform default rather than persisting a resolved executable
path unless the user explicitly selects a concrete shell or custom executable.

## Codex Runtime Default

New Codex runtime defaults are intentionally safer:

```json
{
  "sandbox_mode": "workspace-write",
  "approval_policy": "on-request",
  "full_auto": false
}
```

Per-agent Codex runtime fields remain true overrides. If a Codex agent has no
explicit sandbox, approval, or full-auto configuration, it inherits the global
runtime policy. If a per-agent sandbox or approval policy is explicitly set,
global full-auto is not inherited unless the agent also explicitly sets
`full_auto: true`.

## Legacy Migration

Legacy resolved settings files do not contain source metadata. Wardian migrates
them silently by comparing against historical defaults:

- Values that match historical defaults are treated as inherited defaults.
- Values that differ from historical defaults are preserved as user overrides.
- The historical Codex default
  `danger-full-access + never + full_auto: true` is treated as inherited and
  therefore moves to the safer current default.

This migration is intentionally silent because Wardian currently has a small
user base and the old Codex default was unsafe for average users.

## UI Rules

- Controls should update override state only when the user changes a setting.
- Saving should persist sparse overrides, not the full effective settings object.
- Default-tracking choices can appear as normal dropdown options where that
  keeps the UI clear.
- Binary controls that need a default-tracking state should use a select or
  segmented control rather than a plain two-state toggle.
