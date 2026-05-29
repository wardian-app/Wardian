# Workflow v2 CLI Verbs

- **Status:** Accepted
- **Date:** 2026-05-29
- **Decider:** User

## Context

Wardian now has a Rust workflow v2 blueprint model and deterministic engine in
`wardian-core`. The CLI needs a thin surface for agents and automation to run a
blueprint headlessly, inspect durable run artifacts, and normalize or parse
blueprint Markdown without requiring the desktop app.

The existing workflow CLI verbs remain the app-owned v1 surface:
`wardian workflow list`, `show`, `run`, and `stop`. Those commands continue to
coexist with v2 while the desktop app and run view migrate. The new v2 verbs use
names that avoid overloading v1 `run`.

## Verb Surface

The v2 CLI verbs are:

| Command | Purpose |
|---|---|
| `wardian workflow exec <path> [--executor mock]` | Execute a workflow v2 blueprint headlessly and write a durable run. |
| `wardian workflow runs` | List durable workflow v2 runs from `<wardian-home>/logs/workflows`. |
| `wardian workflow run-show <blueprint-id> <run-id>` | Show one run's checkpoint state and event trace. |
| `wardian workflow replay <blueprint-id> <run-id>` | Rebuild final state from the run event log without executing nodes. |
| `wardian workflow parse <path>` | Parse a blueprint Markdown file and print the structured blueprint JSON. |
| `wardian workflow normalize <path> [--write]` | Normalize a blueprint and print the Markdown, or write it back in place. |

`exec` rejects non-`mock` executor values with `unsupported_executor` until the
real executor lands.

## Execution Decision

This slice intentionally supports mock execution only. `exec` validates the
blueprint, creates a caller-visible run id, and drives
`Engine::start_with_id(..., &MockExecutor::new())`.

Run artifacts are written to the same durable path the real executor will use:

```text
<wardian-home>/logs/workflows/<blueprint-id>/<run-id>/events.jsonl
<wardian-home>/logs/workflows/<blueprint-id>/<run-id>/state.json
```

When the real workflow executor is ready, it should swap the `MockExecutor`
implementation at this boundary and keep the run path stable. That preserves
the `runs`, `run-show`, and `replay` inspection contract for both mock and real
runs.

`resume` is deferred. The engine already exposes resume primitives, but wiring
CLI resume against a mock executor is low-value and could imply real provider
recovery semantics that do not exist yet. Resume should land with the real
executor and provider-runtime handoff.

## Examples

Bash:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-workflow-v2"
wardian workflow exec "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow runs
wardian workflow run-show demo <run-id>
wardian workflow replay demo <run-id>
wardian workflow parse "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow normalize "$WARDIAN_HOME/library/workflows/demo.md"
wardian workflow normalize "$WARDIAN_HOME/library/workflows/demo.md" --write
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-workflow-v2"
wardian workflow exec "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow runs
wardian workflow run-show demo <run-id>
wardian workflow replay demo <run-id>
wardian workflow parse "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow normalize "$env:WARDIAN_HOME\library\workflows\demo.md"
wardian workflow normalize "$env:WARDIAN_HOME\library\workflows\demo.md" --write
```

The v1 app-owned commands remain valid during the migration:

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
