# Chat Work Log Signal

- **Status:** Proposed
- **Date:** 2026-06-07

## Context

Wardian chat mode renders non-message `AgentChatEvent` records as activity
blocks and grouped work logs. This is useful for supervising agent work, but
the current renderer shows too much low-signal provider plumbing. In particular,
successful `tool_result` events with no meaningful payload can appear as visible
rows that say only `Tool result` and `Exit code: 0`.

The screenshot that triggered this spec shows a grouped work log with two shell
commands followed by two separate result bullets. Those result bullets contain
no user-relevant information beyond successful completion. They make the work
log harder to scan and imply that something happened when the real information
is already carried by the command rows.

This is separate from markdown rendering. Markdown fidelity affects how message
text is displayed. Work-log signal affects which tool events are visible, how
tool call/result pairs are summarized, and what details remain available for
copying or debugging.

## Goals

- Reduce low-signal activity rows in chat mode.
- Merge empty successful tool results into their corresponding tool call when
  possible.
- Preserve failures, nonzero exits, meaningful output, changed files, diffs,
  search results, approvals, and errors.
- Keep raw event details available through copy actions or expanded diagnostic
  output where useful.
- Keep the grouped work-log surface compact and trustworthy.

## Non-Goals

- Changing provider transcript parsing in this slice.
- Removing tool visibility from chat mode.
- Hiding failures or nonzero exit codes.
- Changing the backend `AgentChatEvent` DTO.
- Building a full command timeline or trace viewer.

## Current Behavior

`src/features/grid/AgentChatView.tsx` currently hides some empty running
`tool_call` placeholders, but most `tool_result` events pass through:

- `shouldShowChatEvent()` hides empty running calls, not empty successful
  results.
- `activityContent()` falls back to status text when a result has no text.
- `workEntrySummary()` falls back to `Exit code: 0`.
- `deriveChatRows()` groups adjacent tool calls and results, but the grouped
  work log still renders each event as a separate `WorkEntry`.

The result is visible duplication: a shell command row says what ran, while the
following result row only confirms success.

## Decision

Introduce a work-event presentation layer between normalized transcript events
and rendered chat rows. The layer classifies each tool event as one of:

- **visible:** render as its own activity row or grouped work entry;
- **mergeable result:** attach completion metadata to the nearest compatible
  preceding tool call;
- **suppressed result:** hide visually because it carries no user-relevant
  signal;
- **diagnostic only:** omit from the main visual flow but include in copied raw
  group details.

The first implementation can live in `AgentChatView` or a small helper module,
but the behavior should be tested as its own unit so future provider adapters do
not reintroduce noise.

## Signal Rules

### Always Visible

Render these events visibly:

- `error` events;
- `approval` events and any `action_required` status;
- tool results with `status: failed` or `status: cancelled`;
- tool results with nonzero `exit_code`;
- tool results with meaningful `text`;
- tool results with changed files, paths, diffs, JSON payloads, search matches,
  todo updates, or other structured output;
- terminal fallback output.

### Mergeable

A `tool_result` is mergeable when all of the following are true:

- it is adjacent to, or provider-linked to, a preceding compatible `tool_call`;
- it has no meaningful text;
- it has no changed paths or structured payload;
- it has `status: succeeded` or `exit_code: 0`;
- it is not an approval, error, cancellation, or action-required event.

Merged result metadata appears on the command row as subtle completion text,
for example `succeeded`, `exit 0`, or `0.8s` if duration metadata exists.
The metadata should not create a second bullet in grouped work logs.

### Suppressed

A successful result with no meaningful output may be suppressed when no matching
tool call can be found. This avoids rendering generic `Tool result` rows that
only say `Exit code: 0`.

Suppression must be conservative. If the renderer is unsure whether a result
contains useful signal, it should render the result.

### Meaningful Text

Text is meaningful when it contains user-relevant content beyond routine status
or empty-result boilerplate. Examples of non-meaningful text:

- `succeeded`
- `success`
- `ok`
- `done`
- `exit code: 0`
- `Wall time: ...` lines when paired only with other empty-result boilerplate
- empty `Output:` labels
- whitespace-only text

Examples of meaningful text:

- stdout or stderr from a command;
- a test summary;
- file content;
- search matches;
- JSON or structured data;
- diff output;
- changed-file summaries;
- provider error messages.

## Grouped Work Log Behavior

Grouped work logs should summarize work, not replay provider event plumbing.

Requirements:

- A command/result pair with an empty successful result renders as one work
  entry.
- The work entry title and summary prioritize the command, file path, tool name,
  or specific operation.
- Successful empty results do not appear as separate bullets.
- Non-empty results appear under the relevant command when pairing is available.
- Failed results appear visibly even if the matching command is also visible.
- The group event count should reflect visible work entries, not suppressed
  low-signal result events.
- Changed files remain surfaced at the group level.

## Pairing Rules

The first slice should use deterministic local pairing:

- Prefer explicit provider metadata if available, such as a call id, tool id,
  turn id, or parent id.
- Otherwise pair a result with the nearest previous unpaired tool call in the
  same turn.
- If turn identity is missing, pair only adjacent call/result sequences.
- Do not pair across user or assistant message boundaries.
- Do not pair across approvals or errors unless provider metadata explicitly
  links the events.

When pairing fails, apply the suppression rules conservatively.

## Copy And Debuggability

Visual suppression must not destroy evidence.

- Copying a work group should include suppressed diagnostic-only result details
  when they exist.
- Copying an individual visible command row should include merged result
  metadata and any non-empty result output.
- The renderer may expose a future debug toggle for raw event details, but that
  toggle is not required in this slice.

## Architecture

Add a small presentation model before rendering rows, for example:

```text
src/features/grid/workLogPresentation.ts
```

Potential model:

```text
PresentedWorkEntry
- id
- primary_event
- merged_result_events[]
- diagnostic_events[]
- title
- summary
- status
- command
- output
- changed_paths[]
- visible
```

`deriveChatRows()` should group presented work entries rather than raw tool
events. `ActivityRow` and `WorkEntry` should receive already-classified
presentation data instead of recomputing signal from raw event fields in
multiple places.

This keeps the renderer honest: event normalization remains the backend's job,
while frontend presentation decides what is useful to show in a dense
supervision surface.

## Testing

Add focused frontend tests for:

- empty successful `tool_result` with `exit_code: 0` is not rendered as a
  standalone row;
- adjacent shell command plus empty successful result renders as one visible
  command entry;
- grouped work-log event count excludes suppressed empty results;
- nonzero `exit_code` remains visible;
- failed and cancelled tool results remain visible;
- meaningful stdout remains visible and copyable;
- changed-file metadata on a result remains visible at row or group level;
- JSON, diff, todo, search, and file-content outputs are not suppressed;
- result pairing does not cross assistant/user message boundaries;
- copy group includes suppressed diagnostic result metadata.

Browser E2E should cover one representative mock-provider transcript with
multiple commands and empty successes. Native E2E is only needed if the
implementation claims provider-runtime or PTY behavior changed.

Frontend PRs must capture a feature-specific screenshot under
`e2e/screenshots/chat-work-log-signal/<timestamp>/` and embed a hosted image in
the PR description.

## Acceptance Examples

### Empty Successful Result

Input events:

```text
tool_call: command = git status --short --branch
tool_result: status = succeeded, exit_code = 0, text = null
```

Expected visible output:

```text
git status --short --branch
succeeded - exit 0
```

There is no separate `Tool result` row.

### Nonzero Exit

Input events:

```text
tool_call: command = npm run test
tool_result: status = failed, exit_code = 1, text = "1 failed"
```

Expected visible output keeps the failure visible with the command and output.

### Meaningful Success

Input events:

```text
tool_call: command = npm run docs:build
tool_result: status = succeeded, exit_code = 0, text = "build complete in 7.03s"
```

Expected visible output includes the successful output because it carries useful
evidence.

## Rollout

1. Add presentation tests that reproduce the screenshot behavior.
2. Add work-log presentation helpers and conservative signal classifiers.
3. Update `deriveChatRows()` to group presented entries.
4. Update visible activity rows and copy behavior.
5. Verify existing activity-block, approval, copy, and lazy-rendering tests.
6. Capture screenshot evidence for the PR.

## Open Questions

- Should routine `ok` and `done` result text be suppressed for all providers, or
  should that list be provider-specific?
- Should command duration be parsed from existing result text such as `Wall
  time: 0.8 seconds`, or only shown when structured metadata exists?
