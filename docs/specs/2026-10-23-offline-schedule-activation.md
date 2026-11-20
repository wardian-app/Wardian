# Offline Schedule Activation Policy

At scheduler activation, Wardian captures one cutoff after saved-agent restoration has completed and its roster snapshot has persisted, or after confirming that no saved roster exists. An unreadable roster leaves automation triggers stopped.

An occurrence strictly before that cutoff elapsed while Wardian was closed. Recurring schedules move to the next future occurrence without incrementing the occurrence count or changing run history. An occurrence exactly at the cutoff remains due. This reconciliation runs once per scheduler activation; ordinary delayed ticks continue to dispatch due work, including while the app is minimized.

Missed one-time schedules and specific-date schedules with no later eligible date remain stored and show **Missed** in Monitor. A specific-date schedule with a later eligible date advances and preserves its last actual run status and error. Skips do not increment the occurrence count or replace the last actual run timestamp and error. **Run now** makes a retained schedule due through the existing scheduler path, preserving its saved input, assignments, provider, workspace, and end condition.

Calendar roll-forward includes an occurrence at the activation cutoff. A projected date beyond the saved end date is ineligible; it cannot conceal an exhausted specific-date schedule. Saved end conditions also continue to govern manual recovery.

The core schedule store owns each read-modify-write transaction. Startup reconciliation, timer ticks, runtime status updates, and desktop/CLI edits use the same advisory lock at `<wardian-home>/library/schedules.lock`. The lock is released before provider dispatch; a failed save prevents dispatch. Unchanged ticks do not rewrite the schedule file.

The scheduler does not start when schedule state cannot be loaded or a startup reconciliation cannot be persisted. That prevents a failed normalization from dispatching stale offline occurrences.
