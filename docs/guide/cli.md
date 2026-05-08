# Wardian CLI

Wardian includes a standalone `wardian` command for inspecting known agent sessions from a terminal or from inside an agent process.

## Installation

The desktop app copies the bundled CLI on startup:

- Windows command: `%USERPROFILE%\.wardian\bin\wardian.cmd`
- Windows implementation binary: `%USERPROFILE%\.wardian\bin\wardian-cli.exe`
- macOS/Linux command: `$HOME/.wardian/bin/wardian`
- macOS/Linux implementation binary: `$HOME/.wardian/bin/wardian-cli`

Wardian also attempts to add that `bin` directory to the user PATH. Restart the terminal after first launch if `wardian` is not found.

Set `WARDIAN_HOME` to redirect state, the CLI install location, and the live app control endpoint for tests or isolated runs.

For development, set the same `WARDIAN_HOME` before starting the desktop app and before running CLI commands. Otherwise the app debug home and the CLI default production home may differ.

When the desktop app is running for the same `WARDIAN_HOME`, the CLI asks the app for live agent snapshots before falling back to `state.db`. Request `status_source` when you need to know which path answered:

- `live` means the status came from the running desktop app.
- `persisted` means the CLI fell back to durable `state.db` state.

## Agent Identity

Wardian injects `WARDIAN_SESSION_ID` into managed agent processes. Inside an agent terminal, `wardian agent` resolves that session automatically.

Outside a managed agent process, pass a name or UUID:

```bash
wardian agent coder-a1
wardian agent show uuid-1
```

## Commands

```bash
wardian agent
wardian agent <name-or-uuid>
wardian agent show [name-or-uuid]
wardian agent list --scope workspace
wardian agent list --scope all
wardian agent kill <name-or-uuid>
wardian agent pause <name-or-uuid>
wardian agent resume <name-or-uuid>
wardian agent spawn --provider codex --class Reviewer --name reviewer-a1 --workspace <absolute-workspace-path>
wardian agent clone <name-or-uuid> --name coder-a2
wardian agent wait reviewer-a1 --until idle --timeout 10m
wardian agent wait reviewer-a1 --until idle --next --timeout 10m
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include status,output,delivery --timeout 10m
wardian workflow list
wardian workflow show <id-or-name>
wardian workflow run <id>
wardian workflow stop <run-instance-id>
wardian send "review this" --to coder-a1
wardian send "review this" --to reviewer-a1 --wait-until idle --timeout 10m
wardian send "status?" --to class:Coder
wardian send "stand down" --to all
```

Mutating commands use Wardian's local control endpoint and require the desktop app to be running for the same `WARDIAN_HOME`. This includes agent lifecycle commands, `workflow run`, `workflow stop`, and `send`.

`workflow list` and `workflow show` try the running app first, then read workflow JSON files from disk when the app is unavailable.

`agent spawn` requires both `--provider` and `--class` so the created agent's runtime and role are explicit.

`agent wait <target> --until <status>` blocks inside the CLI process until a single agent name or UUID reaches a normalized status such as `idle`, `processing`, `action_required`, `off`, or `error`. Plain `wait` returns immediately when the target is already in the requested status. Add `--next` to wait for a newer matching observation. Use `--timeout` with `ms`, `s`, or `m` units.

`agent watch <target>` returns a live snapshot with agent status, retained output, recent events, delivery details, and a cursor. Add `--until` to block until `status:<status>`, `output:<substring>`, `event:<kind>`, or `delivery:<state>` is observed. `watch` accepts only one name or UUID in this slice. `--follow` is reserved and returns `not_supported`.

`send` submits a provider-aware message into the target agent runtime. Targets can be an agent name, UUID, `class:<ClassName>`, or `all`. `--stdin` reads the message from standard input, and `--file <path>` reads it from a file. `--wait-until <status>` is available for single-agent targets and waits from a pre-send watch cursor for a newer matching status observation. `--thread` is reserved but not implemented yet; when the app is running, using it returns `not_supported`.

Successful `send` responses include `delivery[]`. Failed or partial delivery returns a nonzero exit with JSON on stderr and `details.delivery[]`, including `runtime_state`, `delivery_state`, and provider-specific input errors.

List filters:

- `--status <status>` filters by normalized status, such as `idle`, `processing`, or `action_required`.
- `--class <class>` filters by agent class.
- `--workspace <absolute-path>` filters by workspace and implies `--scope all`.

Output options:

- `--fields name,status,uuid` returns indented JSON with only those fields.
- `--field status` returns one bare value plus a newline.
- `--field status_source` returns `live` or `persisted`.
- `--verbose` adds `pid`, `started_at`, and `last_status_at`.
- `--pretty` returns aligned text for humans instead of JSON.

Default JSON is indented for terminal readability. It includes `schema: 1` and an `agent` or `agents` payload with `name`, `uuid`, `class`, `provider`, `workspace`, and `status`.

## Exit Codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Generic command error |
| 2 | Agent not found |
| 3 | `WARDIAN_SESSION_ID` is not set for self lookup |
| 4 | Wardian state database is unavailable |
| 5 | Lookup matched multiple agents |
| 6 | Desktop app is not running for a live control command |

Errors are written to stderr as JSON:

```json
{
  "schema": 1,
  "error": {
    "code": "not_in_session",
    "message": "WARDIAN_SESSION_ID environment variable is not set",
    "hint": "Pass a name or uuid to look up a specific agent from outside a Wardian-managed agent process: `wardian agent <name>`.",
    "details": {
      "command": "agent",
      "requested": "self"
    }
  }
}
```
