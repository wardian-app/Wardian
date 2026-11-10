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

`GET /remote/api/agents/{session_id}/chat` returns HTTP 503 with the sanitized
code `agent_chat_read_retryable` when its bounded reader is temporarily busy,
its runtime changes, or its projection cannot currently be read. The selected
Chat retries without treating a healthy desktop as disconnected. Invalid
query values retain HTTP 400; authentication and authorization retain their
existing statuses. Capture-stage diagnostics use these sanitized codes:

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

## Recent header windows

The optional recent-only `limit` is an integer from 1 through 80. The PWA asks
for eight headers; an omitted limit retains the default of 80. A complete
physical frame or certified display group stays atomic, so a soft target may
be exceeded within the hard 80-header budget. Older, detail and revalidation
requests retain their existing budgets.

Normal initial/latest desktop and PWA reads opt into `recent_preview=true`.
The default is false, and Older, detail and revalidation requests cannot opt
in. A non-null opaque `recent_admission` marks bounded, non-authoritative
headers from the current producer incarnation. These pages issue no Older
cursor, detail capability, aliases or row-revalidation ACK. Consumers retain
the marker unchanged and replace the window when it changes or is retired,
including an empty retirement response. Actual canonical publication must
cover every current native observation and version before the reader leaves
preview mode. That transition explicitly resets the window; clients do not
invent aliases between provisional and canonical IDs.

The existing projection owner keeps the recent slot separately from slow
archive work. Normal selection only coalesces bounded producer work; source
IO runs off the request thread. Source, privacy or runtime changes invalidate
the slot. An unavailable or expensive native source remains a normal pending
read rather than bypassing admission. See the repository specification
`docs/specs/2026-11-07-chat-recent-publication.md` for producer fences and caps.

The opaque revision identifies the requested window as well as its source
scope. Changing the recent target must not produce a false unchanged result.
Older starts before the oldest row actually emitted. When a new raw suffix
has no qualified archive frontier yet, the reader waits for publication
rather than returning a cursor past omitted headers. Clients carry issued
revisions and cursors unchanged; they do not parse their structure.

## Chat row revalidation

`GET /remote/api/agents/{session_id}/chat` accepts the optional
`revalidate_ids` value as one URL-encoded JSON array, for example
`revalidate_ids=%5B%22source-id%22%5D`. Renewal requests include the current
opaque `revision` and omit `before` and `detail`. The array has one to eight raw
IDs, each at most 512 UTF-8 bytes; repeated query keys and comma-separated
values are not alternate encodings.

The response uses the ordinary metadata page and acknowledges a complete
batch in `revalidated_ids`. The backend validates ownership and current source
scope before returning selected rows; it does not read neighboring history or
expand bodies. Incomplete indexing, a changed head, or an invalid request does
not produce a successful partial acknowledgement. Authentication and gateway
authorization remain those of the existing chat endpoint. See the [IPC and DTO
contract](./ipc-events.md#chat-row-revalidation)authorization remain those of the existing chat endpoint. Canonical pages
with a null source epoch continue through ordinary paging when their session,
conversation, and generation match and no native source evidence is loaded.
Incomplete native scope retains the existing window and older cursor for retry.
The guarded empty first-page bootstrap applies only before committed rows or an
older frontier exist. See the [IPC and DTO
contract](./ipc-events.md#ordinary-canonical-reads-and-first-page-bootstrap) and [internal specification](https://github.com/wardian-app/Wardian/blob/main/docs/specs/2026-10-26-chat-row-revalidation.md).

## Remote Inbox availability

`GET /remote/api/queue` and `POST /remote/api/queue/action` return HTTP 503 with
code `automation_inbox_unavailable` when automation run-index reconciliation
fails and no complete runtime projection is available. A failed refresh does
not replace a cached complete projection or report an empty queue as a
successful response. The next request can retry reconciliation; the error
contains no local filesystem details.
