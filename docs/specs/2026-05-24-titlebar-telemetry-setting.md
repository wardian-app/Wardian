# Titlebar Telemetry Visibility Setting

## Context

Wardian's top bar shows a compact CPU, memory, and active-agent telemetry
cluster beside the left sidebar toggle. This is useful in development and
agent-heavy workflows, but it can feel too diagnostic for normal installed
desktop use.


## Decision

Add a global app setting named `titlebar_telemetry_visible` in
`settings/app.json`. The setting controls only the CPU, memory, and active-agent
count cluster in the top bar. It does not disable telemetry collection, agent
metrics events, Dashboard cards, Graph details, Grid card status, or Watchlist
status columns.

Default behavior:

- Every build: hidden by default.
- An explicit **Show** choice remains visible across restarts and upgrades.

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

When no override exists, the backend uses the hidden default. Existing settings
files continue to load because missing fields inherit the current default;
explicit `true` overrides remain respected.

## Testing

Coverage includes:

- backend hidden-default behavior across build contexts
- sparse app settings override persistence
- frontend settings store load, save, and migration behavior
- Settings modal rendering and save behavior
- titlebar hiding of CPU, memory, and active-agent text while preserving the
  sidebar toggle
