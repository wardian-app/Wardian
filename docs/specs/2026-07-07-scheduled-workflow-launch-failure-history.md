# Scheduled Workflow Launch Failure History

## Context

Scheduled workflow runs can fail before the workflow blueprint is parsed. A
missing or unreadable blueprint path produces a scheduler error such as
`parse failed: io error: The system cannot find the file specified.` Before this
change, that failure was written only to the schedule record. The workflow run
history reads durable run directories, so there was no latest run entry to open
or inspect.

## Decision

When a scheduled launch fails before a blueprint can be parsed, Wardian now
writes a failed run artifact under the workflow run log directory using the
requested blueprint id and a generated run id. The artifact includes:

- `invocation.json` with the schedule id, provider, workspace, bindings, and
  normalized assignments.
- `events.jsonl` with a `run_failed` event carrying the launch error.
- `state.json` with `status: failed` and the same failure message.

The scheduler still records `last_run_status: failed` and `last_run_error` on
the schedule itself. The Monitor view treats that schedule failure as the latest
visible outcome when there is no active or upcoming scheduled run, so an older
completed run can no longer mask the launch failure.

History rows also preserve schedule context when the run summary carries a
`schedule_id`. The Monitor resolves that id against the loaded schedule records
before rendering the row, so scheduled runs keep their cadence and assignment
labels instead of falling back to `Manual only` and `Default`.

## Consequences

- New scheduled launch failures appear in workflow history even when no
  blueprint graph could be loaded.
- Existing missing history entries are not backfilled; only future failures get
  durable run artifacts.
- Scheduled history rows show current schedule metadata when the schedule still
  exists locally.
- Opening one of these failed launch runs may not have a blueprint graph, but
  the run state and event timeline still explain why the launch failed.
