# Wardian CLI

Wardian includes a standalone `wardian` command for inspecting known agent sessions from a terminal or from inside an agent process.

## Installation

The desktop app copies the bundled CLI on startup:

- Windows: `%USERPROFILE%\.wardian\bin\wardian.exe`
- macOS/Linux: `$HOME/.wardian/bin/wardian`

Wardian also attempts to add that `bin` directory to the user PATH. Restart the terminal after first launch if `wardian` is not found.

Set `WARDIAN_HOME` to redirect state and the CLI install location for tests or isolated runs. The CLI reads `state.db` from that home directory.

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
wardian agent list --scope project
wardian agent list --scope all
```

List filters:

- `--status <status>` filters by normalized status, such as `idle`, `processing`, or `action_required`.
- `--class <class>` filters by agent class.
- `--project <project>` filters by project and implies `--scope all`.

Output options:

- `--fields name,status,uuid` returns JSON with only those fields.
- `--field status` returns one bare value plus a newline.
- `--verbose` adds `pid`, `started_at`, `workspace`, and `last_status_at`.
- `--pretty` returns aligned text for humans instead of JSON.

Default JSON includes `schema: 1` and an `agent` or `agents` payload.

## Exit Codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Generic command error |
| 2 | Agent not found |
| 3 | `WARDIAN_SESSION_ID` is not set for self lookup |
| 4 | Wardian state database is unavailable |
| 5 | Lookup matched multiple agents |

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
