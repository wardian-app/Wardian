# Workflow Monitor Adaptive Cards

## Status

Proposed on 2026-07-14.

## Context

The Workflows Monitor currently renders each activity through one dense row
layout. Time, workflow identity, status, schedule, assignment metadata, and
actions receive similar visual weight even though their importance changes by
operator task. Assigned agents are especially difficult to scan because they
appear as muted metadata near the far edge of the row.

The workflow sidebar compresses schedule state, recurrence, and the next-run time
into one truncated metadata line. This makes the most important planning
information unreadable at normal sidebar widths and omits the assigned agents
entirely.

## Decision

Replace the universal activity row with adaptive workflow cards. Each Monitor
section will use a card variant suited to the question that section answers:

- **Scheduled** answers who will run the workflow, when it runs next, and how
  often it recurs.
- **History** answers who ran the workflow, when it ran, and how it ended.
- **Running** answers who is executing the workflow and its current live state.
- **Needs attention** answers who owns the workflow and what operator action is
  required.
- **All** keeps its existing operational sections and renders each section with
  the appropriate card variant rather than forcing one universal column order.

The workflow sidebar will use a compact Scheduled-style card so upcoming time,
recurrence, assigned agents, and previous execution remain readable in the narrow
pane.

This is a frontend presentation change. Existing workflow DTOs, persisted
schedule data, polling, filtering, history pagination, virtualization, and
backend commands remain unchanged.

## Information Hierarchy

### Shared card content

Every card keeps the following information available:

1. Workflow display name
2. Assigned agents or temporary providers, labeled by workflow role
3. Semantic status and any actionable failure
4. Relevant execution times
5. Applicable workflow actions

Raw identifiers must not occupy the primary scan line. They remain available
through action labels, test identifiers, or expanded diagnostic detail where
needed.

### Scheduled cards

Scheduled cards emphasize:

1. Workflow name
2. Assigned agents
3. Next execution time
4. Recurring schedule
5. Previous execution time

Pause, resume, run-now, and edit actions remain directly available. A schedule
failure uses the semantic error tone and exposes its error summary without
hiding the next execution or assignment context.

### History cards

History cards emphasize:

1. Workflow name
2. Assigned agents
3. Run timestamp
4. Outcome and duration when available

Opening the run remains directly available. A failed run presents its failure
summary in the semantic error tone.

### Running and needs-attention cards

Running cards emphasize live state, assigned agents, and the run's start or
updated time. Needs-attention cards additionally emphasize the required human
action. Existing open-run controls remain directly available.

### Compact sidebar cards

The sidebar card shows:

- Workflow name
- Up to two role-aware agent summaries
- Next execution time
- Schedule
- Previous execution time
- Pause or resume and run-now actions

The sidebar does not repeat the full blueprint id in its primary layout. Search
continues to match workflow names, blueprint ids, statuses, errors, and assigned
agent labels.

## Agent Assignment Presentation

Collapsed cards show up to two role-aware assignment chips when the workflow has
agent assignments. Pure script workflows do not show an invented default
assignment. Examples include `writer · Librarian`, `reviewer · Paper-Reviewer`,
and `publisher · Temporary Codex`.

When more than two assignments exist, a `+N agents` button expands an inline
assignment map containing every role, target name, target type, and conversation
mode available in the current DTO. This keeps card heights predictable without
hiding the complete workflow team.

Agent ids resolve through the application agent roster. If an agent no longer
exists, the stored agent id remains visible rather than being replaced with an
incorrect label. Temporary-provider assignments use a humanized provider name.

## Time Presentation

Time labels are calendar-aware and local to the operator:

- Same day: `Today, 3:20 PM`
- Next day: `Tomorrow, 9:45 AM`
- Near future or recent past: `Thu, Jul 16 · 9:35 AM`
- Farther date: `Oct 1, 2026 · 8:00 AM`

Relative context such as `3 hours ago` may appear as supporting text, but it
does not replace the calendar timestamp. Exact local date, time, and timezone
remain available through the element title or accessible description.

Fallbacks are explicit:

- No previous run: `Never run`
- Paused schedule: `Paused`
- No upcoming run: `Not scheduled`
- Invalid timestamp: display the original value without generating misleading
  relative text

## Component Boundaries

The implementation will extract focused units from the current Monitor module:

- A pure activity and presentation-model layer that selects card mode and
  derives highlighted fields
- A shared calendar-aware workflow time formatter
- A shared role-aware assignment summary with inline expansion
- A full-size Monitor workflow card
- A compact sidebar workflow card
- The existing Monitor container, which retains store access, polling,
  filtering, history pagination, and virtualization

The sidebar already receives the application agent roster through its layout
container. That roster will be passed to the workflow glance component so the
sidebar and full Monitor resolve names consistently without issuing a duplicate
agent-list request.

## Data Flow

The full Monitor continues to combine `RunSummary[]`, `WorkflowSchedule[]`, and
`AgentConfig[]`. Pure presentation helpers derive the appropriate card mode,
resolved assignments, primary and secondary time labels, status tone, and issue
summary.

The sidebar combines `WorkflowSchedule[]`, active `RunSummary[]`, and the
already-loaded `AgentConfig[]`. Store actions and callbacks retain their current
ownership. Expanding assignment detail is local presentation state and does not
change persisted workflow state.

## Error Handling and Accessibility

- Missing agents fall back to their stored ids.
- Missing or invalid dates use explicit fallback text.
- Long collapsed values truncate visually but retain full title or accessible
  text; expanded assignments never truncate the role-to-target mapping.
- Status and failure meaning is conveyed by text in addition to semantic color.
- Assignment expansion is a button with accurate expanded state and keyboard
  operation.
- Existing action buttons retain descriptive accessible names and tooltips.

## Testing

Frontend unit and component tests will cover:

- Scheduled, History, Running, and Needs-attention information priorities
- All-view section-specific card variants
- One, two, and more-than-two agent assignments
- Assignment expansion, keyboard-accessible state, unknown agents, and
  temporary providers
- Today, tomorrow, near-date, distant-date, never-run, paused, missing, and
  invalid timestamps
- Failed schedules and failed historical runs
- Existing pause, resume, run-now, edit, open-run, search, and filtering actions
- Existing history pagination, virtualization, and bounded rendering
- Sidebar compact-card content and search behavior

Browser E2E will verify the changed Monitor and sidebar interactions with the
mock provider. The final change will run the repository frontend lint, test,
build, and applicable E2E commands plus the full backend pre-commit checks.

## Documentation and Screenshot Evidence

Update the workflow user guide to explain the section-specific Monitor cards,
calendar-aware timestamps, and expandable assignments. Capture feature-specific
screenshots of the full Monitor and narrow sidebar, store them under the
repository screenshot convention, and embed representative HTTPS-hosted
evidence in the pull request description.

## Out of Scope

- Backend or workflow DTO changes
- New workflow statuses or actions
- Changes to schedule execution semantics
- Replacing Monitor polling with an event stream
- Redesigning Observe mode
- Changing history retention or pagination limits

## Acceptance Criteria

The design is complete when:

1. Assigned agents are primary, readable content in the Monitor and sidebar.
2. Scheduled cards emphasize next execution and recurrence.
3. History cards emphasize run time and outcome.
4. Running and Needs-attention cards emphasize live state and ownership.
5. More than two assignments are discoverable through an accessible inline
   expansion without making every collapsed card taller.
6. Calendar-aware labels remain unambiguous for today, tomorrow, nearby dates,
   and distant dates.
7. Existing Monitor actions, filters, polling, and virtualized history behavior
   continue to work.
