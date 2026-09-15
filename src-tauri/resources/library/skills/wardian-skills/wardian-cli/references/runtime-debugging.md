# Runtime Debugging

Use [orchestration](orchestration.md) for normal live-task control, waiting,
and observation. Use this reference when a command fails or when a development
app and CLI need to share the same runtime state.

## Discover Syntax Without Opening State

```bash
wardian schema
wardian schema delivery show
wardian schema browser '<target>' click
```

Each response describes one command and its immediate children from Clap.
Drill down only as needed. This is syntax discovery, not a response DTO schema,
an exhaustive account of conditional validation, or proof of runtime support.
Use `--help` for human-readable syntax. Generated CLI JSON is compact; existing
`--pretty` modes select human output, not indented JSON. Browser and
`automation node-types` require their own `--json`; `library read` returns raw
content. Agent `--fields`/`--field` projections are not global output flags.

Agent show/list, conversation list/show, and Inbox reads fall back to disk only
when the control endpoint is unavailable. Live rejections, malformed responses,
and timeouts remain errors; stale disk data does not replace them. Request
`status_source` in agent projections; conversation and Inbox responses include
it at the top level. `live` identifies the app response, `persisted` the disk
fallback. This is distinct from an Inbox item's evidence source.

Outside a managed session, default/workspace agent listings use the current
working directory. In a managed session, workspace scope requires an assigned
workspace or an explicit `--workspace <path>`; missing workspace is an error,
not a fleet listing. `--workspace` overrides neighbors. Use `--scope all` only
when the complete roster is intended.

## Offline Does Not Mean Read-Only

Live agent control, messaging, graph edits, browser operations, notifications,
and artifact presentation require the app for the same `WARDIAN_HOME`.
Library, memory, teams/watchlists, and schedule mutations can write offline.
Automation normalization writes only with `--write`; replay reconstructs state
without running steps. Artifact inspection can fall back to disk.

For a strict read-only audit, use schema/help or direct bounded reads of known
files. Agent disk fallback and telemetry inspection open a writable state
database and run migrations. Memory inspection initializes/migrates its store,
and list/recall can recover a pending agent replacement. Library class access
initializes defaults. Do not run these against live state merely to test
whether they are side-effect-free; use an authorized isolated fixture for that.

Telemetry summaries do not ingest provider sources, but this does not bypass
the database-opening effects above. For normal usage:

```bash
wardian telemetry summary --horizon week --dimension provider
```

## Interpret Common Errors

Errors are compact JSON on stderr. Branch on `error.code`, and retain
`error.details` and `error.hint` when reporting failures. Backend codes are
preserved as strings, including unfamiliar codes; this list is not exhaustive:

- `invalid_arguments`: CLI argument parsing failed. Follow the discovery hint
  with `wardian schema <command path>` or the relevant `--help`.
- `validation_failed`: Automation semantic validation failed; the command exits
  nonzero and diagnostics are in `error.details.diagnostics`.
- `invalid_json`: An `--input` or `--assignments` document is unreadable, invalid
  JSON, not an object, or incompatible with the requested input type.
- `not_in_session`: Self lookup was requested outside a managed process; pass
  an explicit name or UUID.
- `missing_managed_sender`: Peer messaging requires an existing managed
  session. Do not invent or override its identity.
- `not_found`: The target does not exist; list agents and use its UUID.
- `ambiguous`: A name matched multiple agents; use the UUID.
- `db_unavailable`: The required state database could not be located, opened,
  or prepared; this does not mean an answering app's error was ignored.
- `app_not_running`: A live-control command could not reach the app; this is
  exit code 6.
- `not_supported`: The requested operation is unavailable. Retired command
  spellings and flags are parser errors, not compatibility routes.
- `control_endpoint_timeout`: The endpoint did not answer in time; persisted
  fallback is not used to mask this failure.
- `memory_identity_required` / `invalid_memory_capability`: Memory requires
  managed identity and its inherited capability; `--agent` is not a bypass.

Inspect JSON on stdout as well as errors on stderr. A messaging admission
receipt is not task completion; inspect the correlated reply's `reply_status`.
Timeout does not imply non-delivery or authorize replay; use
[messaging](messaging.md) to interpret evidence before another action.

## Run The App And CLI Together

Set one explicit `WARDIAN_HOME` for both the dev app and CLI so they share the
same control endpoint and state.

macOS/Linux shell:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
npm run dev
```

Second terminal:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
npm run dev
```

Second terminal:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

After a release build, use the repository-root target output:

```bash
./target/release/wardian-cli agent list --scope all
```

Windows release builds use an `.exe` name:

```powershell
.\target\release\wardian-cli.exe agent list --scope all
```
