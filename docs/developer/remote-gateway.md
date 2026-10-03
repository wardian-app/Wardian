# Remote Gateway

The authenticated remote API is served by `src-tauri/src/remote/gateway.rs`.
Handlers return machine-readable error codes while keeping internal errors out
of remote responses.

## Listener ownership and shutdown

The desktop runtime owns one `RemoteListener`. Startup and settings persistence
share a configuration gate; repeated saves of the same running configuration
reuse its listener. Changed settings stop and join the previous server before
binding the replacement. Disabling remote access also stops the listener.
Settings saves await binding and return an error if another process owns the
port. Wardian never kills or adopts an unrelated port owner.

Exit fences late listener starts, requests graceful server shutdown, and joins
the listener task. Shutdown allows two seconds for HTTP connections to drain,
then aborts and joins the server task. Restart uses Tauri's `request_restart`,
and the Windows installer handoff uses `exit`, so both run the exit cleanup hook
before the process ends. The installer still waits for the old process to exit.
An already running older Wardian must exit before a replacement can bind its
port; this listener owner does not authorize cross-process termination.

Backend tests bind real loopback sockets to verify repeated and concurrent
saves, configuration replacement, disable, shutdown, and foreign-port refusal.

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

## Remote Inbox availability

`GET /remote/api/queue` and `POST /remote/api/queue/action` return HTTP 503 with
code `automation_inbox_unavailable` when automation run-index reconciliation
fails and no complete runtime projection is available. A failed refresh does
not replace a cached complete projection or report an empty queue as a
successful response. The next request can retry reconciliation; the error
contains no local filesystem details.
