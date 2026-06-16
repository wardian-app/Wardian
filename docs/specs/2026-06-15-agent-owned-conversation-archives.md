# Agent-Owned Conversation Archives

- **Date:** 2026-06-15
- **Status:** Implemented
- **Context branch:** `feat/conversation-logs`

## Context

Wardian currently projects chat history on demand from provider logs, provider
databases, live watch state, and terminal-derived fallback state. That is useful
for the chat view, but it is not a durable Wardian-owned record. Once short-term
context is cleared, provider sources move, or bounded watch state is discarded,
there is no stable archive that another agent can parse to understand what was
asked, what the agent did, and how the conversation progressed.

Durable conversation archives are useful for two related purposes:

1. Preserving evidence of LLM-assisted work in a local, inspectable format.
2. Giving Evolver and other agents a consistent trace of past conversations so
   they can learn user patterns and create useful skills, workflows, or tools.

The archive must be reliable before it is broad. It should preserve Wardian-run
conversations from the point where logging is enabled, without trying to
retroactively infer unclear boundaries from provider-owned history.

## Decision

Wardian will add agent-owned conversation archives. The canonical archive lives
under each agent's Wardian directory:

```text
.wardian/
  agents/
    <agent-id>/
      conversations/
        index.jsonl
        <conversation-id>/
          manifest.json
          conversation.jsonl
          events.jsonl
          sources.jsonl
          artifacts/
```

A future top-level aggregate index may be added as a derived cache, but it is
not canonical in v1. The owning agent's `conversations/` directory is the source
of truth.

## Conversation Boundaries

A Wardian conversation is an agent-owned archive segment bounded by the active
provider transcript source. The boundary is not the lifetime of an agent
identity, and it is not simply a provider-native session id. Provider session ids
are provenance attached to the Wardian conversation.

Conversation ids are Wardian-generated and globally unique across a Wardian
home. Storage remains agent-owned, but global uniqueness lets
`wardian conversation show <conversation-id>` resolve a conversation without an
extra agent argument.

A new conversation starts when provider continuity changes. In v1, the main
boundary is Wardian's provider-continuity clear/reset behavior. Any code path
that implicitly performs the same clear, such as worktree enable, join, disable,
or repair flows, must route through the same conversation rollover function so
logging behavior is identical.

A pure terminal display clear or scrollback repaint does not create a new
conversation unless it also resets provider continuity.

## Capture Model

Conversation logging runs in the backend lifecycle and transcript path, not in
the React chat view. The chat view can read archived conversations later, but
capture must not depend on UI polling.

Wardian appends only completed semantic records:

- Delivered user or inter-agent input.
- Completed assistant messages.
- Completed tool calls and tool results.
- Approval or denial outcomes.
- Errors that affect the conversation.
- Important lifecycle records, including clear, provider-source rollover,
  worktree switch, session start, and session close.

The capture process is incremental and non-blocking. It should use provider
source cursors when reading provider logs or databases, batch small append work,
and avoid parsing or rewriting large files repeatedly. Archival failures are
reported through logs or diagnostics but must not block provider I/O, terminal
rendering, clear/reset, worktree switching, or agent lifecycle operations.

## Files

### `manifest.json`

Stores conversation-level metadata:

- `schema`
- `conversation_id`
- owning agent id, name, and class
- workspace
- provider
- provider session/source ids
- provider source key used for continuity rollover and open-archive hydration
- effective logging setting
- created, updated, and closed timestamps
- status: `open`, `closed`, or `interrupted`
- boundary reason
- file format versions

`manifest.json` updates should be written atomically.

### `conversation.jsonl`

This is the primary post-hoc narrative for agents. It must be sufficient for
Evolver or a reviewer agent to understand what happened without reconstructing
the dialogue from provider-specific logs.

Records are ordered and completed. They include visible user prompts,
inter-agent prompts as they appear to the provider, assistant responses, tool
activity, approvals, errors, important lifecycle events, bounded excerpts, and
artifact pointers.

Common fields include:

- `schema`
- `seq`
- `at`
- `kind`
- `role`
- `speaker_type`
- `text`
- `tool`
- `status`
- `summary`
- `excerpt`
- `event_refs`
- `source_refs`
- `artifact_refs`

Wardian does not need perfect human-vs-agent attribution in v1. Inter-agent
messages often land through the same provider input path as user prompts, so
attribution should be best effort.

### `events.jsonl`

Stores fuller canonical event records. It includes the records represented in
`conversation.jsonl` plus more detailed structured data for status transitions,
tool metadata, provider-specific normalized fields, and lifecycle events.

### `sources.jsonl`

Stores provenance records without copying whole provider histories by default.
Records may include:

- provider
- provider session id
- source kind
- source path or database identity
- cursor, offset, row id, or comparable source position
- provider event type
- hash
- copied raw-payload artifact reference, when present

### `artifacts/`

Stores full payloads that should not be embedded in the narrative file, such as
large stdout/stderr, large tool results, screenshots, or bounded raw provider
slices.

Tool output handling is deterministic:

- Outputs up to 8 KiB are stored inline in `conversation.jsonl`.
- Outputs over 8 KiB get a bounded excerpt in
  `conversation.jsonl`.
- The full output is stored in `artifacts/`.
- The rule applies consistently regardless of tool type or success/failure.

### `index.jsonl`

Each agent has a local conversation index at
`agents/<agent-id>/conversations/index.jsonl`. It uses append-only upsert
records where the latest record for a `conversation_id` wins.

The index stores mechanical metadata only:

- `conversation_id`
- agent id, name, and class
- workspace
- provider
- provider session ids
- start and end timestamps
- status
- boundary reason
- first prompt excerpt
- last record excerpt
- record count
- artifact count
- path

No LLM-generated titles or summaries are created in v1.

## Settings

Conversation logging has global and per-agent controls:

```text
global: enabled | disabled
agent: default | enabled | disabled
```

`default` means the agent follows the global setting. Core Wardian defaults
global logging to `enabled` because Wardian is local-first and inspectable, but
the setting must be visible and easy to disable.

When effective logging is disabled:

- Wardian writes no conversation archive records.
- Wardian does not silently backfill from provider logs later.
- Re-enabling starts fresh from that point forward.
- A new archive opens only on the next completed semantic record or fresh
  provider source.

Retention is out of scope for v1. There is no automatic deletion, max age, or
max archive size policy beyond deterministic artifact thresholding.

## CLI and Backend API

V1 exposes a minimal read surface so agents do not need to know filesystem
conventions:

```text
wardian conversation list
wardian conversation list --agent <agent-id-or-name>
wardian conversation list --scope all
wardian conversation show <conversation-id>
```

Inside a Wardian-managed agent terminal, `list` defaults to the current agent.
`--scope all` reads all agent-local indexes. `show` defaults to the
agent-readable narrative: manifest basics plus `conversation.jsonl` records.

Optional detail flags may be included in v1 if low-cost, otherwise deferred:

```text
--events
--sources
--artifacts
```

Matching backend/Tauri commands should exist for parity, but a full UI is not
required in v1.

## Reliability Requirements

- Conversation archives are agent-owned and filesystem-readable.
- Capture is opportunistic and non-blocking.
- Only completed semantic records are appended.
- Record streams use append-only JSONL.
- `manifest.json` updates are atomic.
- Provider log/database reads use source cursors.
- Clear/reset and implicit clear paths share one rollover function.
- Disabled logging never backfills missed history.
- Large outputs follow the deterministic 8 KiB artifact threshold policy.
- Provider-native raw history is referenced by provenance and copied only as
  bounded payloads or explicit artifacts.

## Test Coverage

Implementation should include focused tests for:

- Archive creation under `agents/<agent-id>/conversations/`.
- Incremental completed-record append.
- Clear/reset finalizing the current archive and starting a fresh one.
- Worktree switching triggering the same rollover behavior as explicit clear.
- Disabled logging writing nothing.
- Re-enabled logging starting fresh without backfill.
- Long tool output creating a deterministic excerpt plus artifact.
- `wardian conversation list` and `show` reading agent-owned archives.
- Provider source cursor/provenance behavior for every provider touched by the
  implementation slice.

## Non-Goals for V1

- Retention policy.
- LLM-generated summaries, titles, or analysis.
- Full conversation search.
- Editing or deleting archived conversations through CLI/UI.
- Copying entire provider-native histories by default.
- Treating top-level global conversation storage as canonical.
- Perfect human-vs-agent prompt attribution.
