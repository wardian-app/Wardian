# Runtime Debugging

## Observe Agent Output

Use `agent watch` for bounded observation:

```bash
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include status,output --timeout 10m
```

The default response includes status, provider-adapted transcript text,
sanitized terminal output, delivery details, and a cursor. Use `--raw` or
`--include raw_output` only when debugging terminal escape sequences or repaint
behavior. Marker matching checks transcript text, sanitized output, and an
internal raw-output fallback. `--follow` is reserved and returns
`not_supported`.

Use `--until output:<token>` only when output-substring compatibility is needed.
Explicit `status:<status>`, `event:<kind>`, and `delivery:<state>` conditions
retain watch-based behavior.

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
