# Titlebar Telemetry Visibility Setting

## Context

Wardian's top bar shows a compact CPU, memory, and active-agent telemetry
cluster beside the left sidebar toggle. This is useful in development and
agent-heavy workflows, but it can feel too diagnostic for normal installed
desktop use.

Wardian already distinguishes official stable release builds from development,
prerelease, and local source-built binaries for the in-app updater. The telemetry
default should follow that same build eligibility boundary so official installed
stable builds start quieter without hiding diagnostics from development builds.

## Decision

Add a global app setting named `titlebar_telemetry_visible` in
`settings/app.json`. The setting controls only the CPU, memory, and active-agent
count cluster in the top bar. It does not disable telemetry collection, agent
metrics events, Dashboard cards, Graph details, Grid card status, or Watchlist
status columns.

Default behavior:

- Official stable release build: hidden by default.
- Development build, prerelease build, or unmarked source-built release binary:
  visible by default.

The official stable release check uses the same local build facts as updater
eligibility: a non-debug build whose compile-time `WARDIAN_UPDATE_CHANNEL` is
`stable`.

## UX

Settings adds **Appearance > Top bar telemetry** with two options:

- **Show**
- **Hide**

The setting saves immediately through the existing app settings store. Changing
it updates the top bar without requiring restart. The left sidebar toggle remains
visible in all cases.

## Persistence

The setting lives in the existing sparse app settings document:

```json
{
  "schema_version": 2,
  "overrides": {
    "titlebar_telemetry_visible": false
  }
}
```

When no override exists, the backend computes the effective default for the
current build. Existing settings files continue to load because missing fields
inherit the current default.

## Testing

Coverage includes:

- backend default behavior for non-stable and stable release contexts
- sparse app settings override persistence
- frontend settings store load, save, and migration behavior
- Settings modal rendering and save behavior
- titlebar hiding of CPU, memory, and active-agent text while preserving the
  sidebar toggle
