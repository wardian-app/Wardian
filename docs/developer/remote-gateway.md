# Remote Gateway

The authenticated remote API is served by `src-tauri/src/remote/gateway.rs`.
Handlers return machine-readable error codes while keeping internal errors out
of remote responses.

## Agent chat transcript errors

`GET /remote/api/agents/{session_id}/chat` returns HTTP 400 when transcript
capture or projection fails. Its JSON `code` identifies the backend stage that
failed:

| Code | Stage |
| --- | --- |
| `agent_chat_snapshot_failed` | Agent capture snapshot |
| `agent_chat_provider_capture_failed` | Provider log state, policy observation, or provider log acquisition |
| `agent_chat_archive_write_failed` | Conversation archive or provider capture state write |
| `agent_chat_projection_failed` | Provider and watch event projection |
| `agent_chat_provenance_failed` | Current capture and archived event provenance reconciliation |

These codes are sanitized stage diagnostics. They do not include the underlying
error, provider arguments, session identifiers, or filesystem paths, and they
do not identify the root cause within a stage. In particular, a stage code
observed in a fixture or test does not establish the cause of a failure in a
live session. Existing authentication and authorization failures retain their
own status and error codes.
