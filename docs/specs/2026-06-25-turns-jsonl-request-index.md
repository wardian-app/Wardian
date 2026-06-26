# turns.jsonl Request Index

## Decision

Wardian treats `turns.jsonl` format version 2 as a cheap, deterministic index over
`conversation.jsonl`. It is derived from existing structured archive records and
contains no agentic analysis, scoring, blame assignment, or free-form notes for
future agents.

One `turns.jsonl` row represents one request/response unit: a user-originated
request plus the assistant, tool, and lifecycle records that follow it until
the next user-originated request or conversation boundary. Provider-native
`turn_id` values stay on `conversation.jsonl` records as provenance, but they
do not define turn rows because providers can use tool-call IDs as turn IDs.

## Refresh Behavior

Wardian refreshes `turns.jsonl` whenever it refreshes the normalized archive,
including open conversations. The writer reads the current `conversation.jsonl`,
`events.jsonl`, and `sources.jsonl`, derives all turn rows, writes
`turns.jsonl` through the existing atomic JSONL writer, and updates
`manifest.json` plus the conversation index with `turn_count` and `has_turns`.

This favors practical reader behavior over avoiding small rewrites. The turn
file is much smaller than raw logs, and active conversations are the main place
where missing turn indexes make agent review expensive.

## Row Shape

Each row includes:

- Row version: `schema: 2`, matching `manifest.format_versions.turns: 2`.
- Stable ordering: `conversation_id`, `turn_index`, and `turn_key`.
- Mechanical state: `status` and `status_source`.
- Bounds: `seq_start`, `seq_end`, `started_at`, and `updated_at`.
- Typed request data: `request.seq`, `request.kind`, `request.text`, and
  `request.text_truncated`. Goal rows may also include compact
  `request.objective_text` so readers can avoid parsing large provider
  continuation scaffolding for the active objective.
- Last assistant message in the row as `assistant_result`, when present.
- Mechanical aggregates under `counts`, `tools_used`, `files`,
  `external_side_effects`, and `failure_signals`. `files` is populated from
  structured metadata, explicit patch headers, conservative command-path
  extraction, and exact path mentions in request/assistant/tool text. Side
  effects come from structured metadata, structured command fields, explicit
  `apply_patch` records, or exact URL-pattern extraction; file-edit side
  effects include touched paths when the patch header names them.
- Provenance under `record_refs` and `provider_native_refs`.

Allowed turn statuses are `in_progress`, `pending_response`, `responded`,
`interrupted`, `lifecycle`, `context_only`, `superseded`, and `unknown`.
Wardian does not write a success status unless a future structured success
signal exists. `unknown` is reserved for malformed or currently unclassified
rows rather than normal no-response rows.

Allowed request kinds include `user_request`, `goal_start`,
`goal_continuation`, `agent_context`, `lifecycle`, `unknown_user_message`, and
`unknown`. AGENTS.md injections and Codex goal continuation context are typed so
summary readers can skip them without parsing raw prompt text.

`manifest.turn_count` is the physical number of rows in `turns.jsonl`, including
filterable `context_only` rows. Readers that want user task counts should filter
by `request.kind` or `status` instead of treating the manifest count as a task
metric.

## Non-Goals

`turns.jsonl` does not summarize intent, assess quality, assign semantic blame,
infer recovery, infer side effects from arbitrary prose, or decide task success.
Readers that need deeper meaning should load the referenced raw records from
`conversation.jsonl`.
