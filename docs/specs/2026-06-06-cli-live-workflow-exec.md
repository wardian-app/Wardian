# CLI Live Workflow Execution

- **Status:** Accepted
- **Date:** 2026-06-06
- **Decider:** User

## Context

PR #501 fixed the app-side workflow executor so live active-agent nodes complete
through the structured `wardian reply` contract instead of terminal idle status.
The standalone CLI still defaulted `wardian workflow exec <path>` to the local
mock executor and rejected `real`/`live` executor values, so agents could not
launch the app-owned full workflow runtime from automation.

## Decision

`wardian workflow exec <path>` now defaults to the app-backed live executor.
`--executor live`, `--executor real`, and `--executor full` are aliases for the
same control-endpoint launch path. The CLI sends a `workflow_run` request to the
running Wardian app with the path, JSON input object, and role/class bindings.

`--executor mock` remains a local deterministic path for offline tests and run
artifact round trips. It continues to use `MockExecutor` in the CLI process.

## Rationale

The live executor depends on app-owned state: active agent PTY senders, input
readiness, task interactions, assignments, persisted agent config, and provider
runtime management. Duplicating that logic in the CLI would create a second
source of truth for agent lifecycle and PTY behavior. Routing through the live
control endpoint keeps workflow execution consistent with the desktop app and
scheduler.

## Behavior

- Running `wardian workflow exec <path>` requires the Wardian app to be running
  for the same `WARDIAN_HOME`.
- If the app is unavailable, the CLI returns `app_not_running`.
- The live response includes `schema`, `ok`, `run_id`, `blueprint_id`,
  `run_dir`, and `executor: "live"`.
- Running `wardian workflow exec <path> --executor mock` does not contact the
  app and writes durable run artifacts under
  `<wardian-home>/logs/workflows/<blueprint-id>/<run-id>/`.

## Examples

Bash:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-workflow"
wardian workflow exec "$WARDIAN_HOME/library/workflows/autoreview.md"
wardian workflow exec "$WARDIAN_HOME/library/workflows/autoreview.md" --executor mock
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-workflow"
wardian workflow exec "$env:WARDIAN_HOME\library\workflows\autoreview.md"
wardian workflow exec "$env:WARDIAN_HOME\library\workflows\autoreview.md" --executor mock
```
