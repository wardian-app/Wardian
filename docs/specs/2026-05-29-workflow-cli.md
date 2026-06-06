# Workflow CLI Verbs

- **Status:** Accepted
- **Date:** 2026-05-29
- **Decider:** User

## Context

Wardian now has a Rust workflow blueprint model and deterministic engine in
`wardian-core`. The CLI needs a thin surface for agents and automation to run a
blueprint headlessly, inspect durable run artifacts, and normalize or parse
blueprint Markdown without requiring the desktop app.

The existing workflow CLI verbs remain the app-owned old workflow system surface:
`wardian workflow list`, `show`, `run`, and `stop`. Those commands continue to
coexist with workflow while the desktop app and run view migrate. The new workflow verbs use
names that avoid overloading old workflow system `run`.

## Verb Surface

The workflow CLI verbs are:

| Command | Purpose |
|---|---|
| `wardian workflow exec <path> [--executor live\|real\|full\|mock] [--provider <provider>] [--workspace <path>]` | Execute a workflow blueprint and write a durable run. |
| `wardian workflow runs` | List durable workflow runs from `<wardian-home>/logs/workflows`. |
| `wardian workflow run-show <blueprint-id> <run-id>` | Show one run's checkpoint state and event trace. |
| `wardian workflow replay <blueprint-id> <run-id>` | Rebuild final state from the run event log without executing nodes. |
| `wardian workflow parse <path>` | Parse a blueprint Markdown file and print the structured blueprint JSON. |
| `wardian workflow normalize <path> [--write]` | Normalize a blueprint and print the Markdown, or write it back in place. |

`exec` defaults to `--executor live`. The `live`, `real`, and `full` aliases
route through the running Wardian app's live control endpoint so the app-owned
workflow runtime remains the authority for live agents, PTYs, assignment
catalogs, and provider execution. `--executor mock` remains available only for
workflow-engine fixture tests that must avoid app/live runtime dependencies.

## Execution Decision

Live execution is app-backed. The CLI sends a `workflow_run` control request
with the blueprint path, optional provider, optional workspace, input object, and
role/class bindings. The app validates the blueprint, creates a caller-visible
run id, spawns the live workflow driver task, and returns the durable run
location. If the app is not running for the same `WARDIAN_HOME`, live execution
returns `app_not_running`.

Mock execution is an internal test fixture path. `exec --executor mock`
validates the blueprint, creates a caller-visible run id, and drives
`Engine::start_with_id(..., &MockExecutor::new())` inside the CLI process so
engine tests can run without depending on app/live runtime systems.

Run artifacts are written to the same durable path the real executor will use:

```text
<wardian-home>/logs/workflows/<blueprint-id>/<run-id>/events.jsonl
<wardian-home>/logs/workflows/<blueprint-id>/<run-id>/state.json
```

The CLI deliberately does not duplicate the app-only live executor. That keeps
PTY lifecycle, active-agent routing, task/reply interactions, schedules, and
provider runtime behavior behind one backend authority. The `runs`, `run-show`,
and `replay` inspection contract stays stable for live runs and engine fixture
runs.

`resume` is deferred. The engine already exposes resume primitives, but wiring
CLI resume against a mock executor is low-value and could imply real provider
recovery semantics that do not exist yet. Resume should land with the real
executor and provider-runtime handoff.

## Examples

Bash:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-workflow"
wardian workflow exec "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow exec "$WARDIAN_HOME/library/workflows/demo.md" --workspace "<absolute-workspace-path>"
wardian workflow runs
wardian workflow run-show demo <run-id>
wardian workflow replay demo <run-id>
wardian workflow parse "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow normalize "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow normalize "$WARDIAN_HOME/library/workflows/demo.md" --write
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-workflow"
wardian workflow exec "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow exec "$env:WARDIAN_HOME\library\workflows\demo.md" --workspace "<absolute-workspace-path>"
wardian workflow runs
wardian workflow run-show demo <run-id>
wardian workflow replay demo <run-id>
wardian workflow parse "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow normalize "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow normalize "$env:WARDIAN_HOME\library\workflows\demo.md" --write
```

The old workflow system app-owned commands remain valid during the migration:

```bash
wardian workflow list
wardian workflow show <id-or-name>
wardian workflow run <id>
wardian workflow stop <run-instance-id>
```

PowerShell:

```powershell
wardian workflow list
wardian workflow show <id-or-name>
wardian workflow run <id>
wardian workflow stop <run-instance-id>
```
