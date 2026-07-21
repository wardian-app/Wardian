# Runtime Debugging

Use [orchestration](orchestration.md) for normal live-task control, waiting,
and observation. Use this reference when a command fails or when a development
app and CLI need to share the same runtime state.

## Interpret Common Errors

Errors are JSON on stderr. Common cases:

- `not_in_session`: Self lookup was requested outside a managed process; pass
  an explicit name or UUID.
- `not_found`: The target does not exist; list agents and use its UUID.
- `ambiguous`: A name matched multiple agents; use the UUID.
- `db_unavailable`: Neither the live app nor `state.db` answered.
- `app_not_running`: A live-control command could not reach the app; this is
  exit code 6.
- `not_supported`: The command shape is recognized but not implemented, such
  as `send --thread`.

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
