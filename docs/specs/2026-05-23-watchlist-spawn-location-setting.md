# Watchlist Spawn Location Setting

## Summary

Wardian exposes a global app setting that controls where newly spawned visible
agents are placed in the agent roster: top or bottom.

## Motivation

Users manage active swarms from the right watchlist roster. Some workflows treat
newly spawned agents as the most important active work and want them immediately
visible at the top. Other workflows prefer chronological accumulation and want
new agents appended to the bottom. The behavior should be configurable without
changing existing watchlist column or team preferences.

## Behavior

- The setting is named `watchlist_new_agent_position`.
- Allowed values are `bottom` and `top`.
- The default is `top`, preserving existing roster behavior.
- The setting is global and stored in `settings/app.json` as a sparse app
  settings override.
- The setting applies when a new visible agent is spawned from the Agent Config
  pane.
- Wardian moves only the newly spawned agent to the configured edge of the
  global roster order and persists that order with `reorder_agents`.
- Normal startup refreshes, status refreshes, clone placement, manual dragging,
  team placement, and explicit watchlist entry order are not reinterpreted by
  this setting.

## UI

Settings includes a **Watchlist** category with a **New agent position** row.
The row uses a select control with:

- **Top**
- **Bottom**

Changing the setting saves immediately through the existing app settings
pipeline.

## Validation

Frontend tests cover:

- app settings load/save for `watchlist_new_agent_position`
- Settings modal rendering and persistence for the Watchlist category
- spawn callback propagation of the created agent
- bottom-placement reorder behavior after a configured spawn

Backend tests cover:

- default value when the app settings file is missing
- sparse override loading
- round-trip persistence
- invalid value normalization to `top`
