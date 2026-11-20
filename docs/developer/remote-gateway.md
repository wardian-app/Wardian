# Remote Gateway

The authenticated remote API is served by `src-tauri/src/remote/gateway.rs`.
PWA handlers return machine-readable `code` values; paired-client handlers use
the `{ "error": "stable_code" }` shape. Both keep internal errors out of
remote responses.

## PWA Chat attachments

PWA uploads use the session-authenticated namespace, independently of the
paired-client text-only API. `POST /remote/api/agents/{session_id}/attachments/{upload_id}`
accepts one raw file body, its validated Content-Type, and an
`x-wardian-filename` header encoded with encodeURIComponent. The upload ID is
a client-generated canonical UUIDv4. Host, exact Origin, live device/session,
CSRF and rate limits are checked before body consumption. Actual streamed
bytes are limited to 10 MiB under a 60-second deadline.

GET returns metadata without bytes; DELETE discards an unsubmitted upload.
GET permits the absent Origin header produced by ordinary same-origin browser
fetches, using the existing canonical-Host/read-only boundary policy. A supplied
foreign Origin or cross-site fetch metadata is still rejected. Mutations require
explicit Origin and CSRF.
Ownership is bound to device, origin and target. Responses never contain host
paths. The server resolves bytes into the target workspace's
`.wardian/attachments/remote/` directory so ordinary provider read permissions
apply. The metadata index lives under `<wardian-home>/remote/attachments/`.
Pending files expire after one hour; admitted files retain seven days.
The owned upload root carries a managed `.gitignore` excluding its contents.
Uploads fail closed on a conflicting or linked marker; project ignore rules
are preserved.

PWA `send_prompt` accepts optional `attachment_ids` and `submission_id`. The
server resolves IDs into file references using the desktop prompt formatting
contract. Image bytes use those same references; no clipboard/paste staging
occurs. Attachment sends require message mode, not provider commands or Inbox
choices. `GET /remote/api/agents/{session_id}/attachment-submissions/{submission_id}`
recovers the persisted status after response loss. An uncertain send is never
replayed automatically. All attachment responses are no-store, and the PWA
service worker bypasses the whole API namespace.

Attachment submission IDs are `<issued_epoch_ms>.<UUIDv4>`. Fresh admission
allows five minutes of age and sixty seconds of future clock skew; existing
proofs have a fixed 24-hour recovery lifetime. The ledger is limited to 2,048
records and 8 MiB, with terminal payload-digest tombstones and expired-proof
cleanup. Unknown stale IDs and expired proofs return 410 instead of creating a
new operation. A status 404 permits an explicit retry with the same ID and
frozen payload, preserving duplicate safety when the original request is late.

## Paired-client v1 authentication

Paired clients use the isolated `/remote/client/v1` namespace. The persisted
`client_origin_allowlist` is empty by default, and each entry is a canonical
browser origin. HTTPS is required except for an explicitly configured HTTP
loopback origin used by local development. The request `Origin` must exactly
match a current entry, and the canonical gateway `Host` is checked separately.

Desktop pairing offers bind one client origin, gateway identity, device key,
and requested grants. Omitting `requested_scopes` preserves the existing
`agents:read` and `chat:read` defaults. `chat:send` is a separate explicit
grant: an existing read-only device is never upgraded implicitly. Desktop
approval displays the credential kind, exact client origin, key fingerprint,
and read/send grants from the immutable pending request.

Client auth uses `POST /remote/client/v1/auth/challenge` followed by
`POST /remote/client/v1/auth/token`; the client signs the returned UTF-8
signing payload with P-256 ECDSA and SHA-256, using DER-encoded signatures.
Before signing, clients validate the gateway origin, actual client origin,
audience `wardian_remote_client_v1`, device ID, challenge ID, nonce, fingerprint
and expiry. They reconstruct the exact payload and compare it with
`signing_payload`; arbitrary server-provided strings must not be signed.
Public keys are SPKI DER encoded as standard base64. The signed UTF-8 payload
uses LF separators and no trailing newline:

```text
wardian.remote.client.auth.v1
origin:<gateway-origin>
client_origin:<client-origin>
audience:wardian_remote_client_v1
device:<device-id>
challenge:<challenge-id>
nonce:<nonce>
```

The issued bearer is returned once, expires after 15 minutes, and is represented
in runtime memory only by its verifier and a separate audit ID. A token with
`chat:send` also returns a durable 32-byte random idempotency namespace bound to
the device, exact client origin, and audience. Its expiry is fixed at 24 hours
from creation; reauthentication reuses that namespace while it remains active.
Every protected request rechecks current configuration and durable device
authority. Protected client routes require bearer authorization; cookies and
PWA sessions do not grant paired-client access.

The PWA protocol at `/remote/api` remains independent. Do not add a client
`/auth/session` alias or use the eager transcript helper to implement client
chat. Chat reads must use the shared bounded reader and its opaque cursor and
lazy-detail contract.

Paired clients read agent history through
`GET /remote/client/v1/agents/{session_id}/chat`, authorized by `chat:read`.
The route shares the PWA Chat query and canonical page contract: `before` is an
opaque older-page cursor, `revision` pins reads, `detail` loads a bounded body
chunk, and `revalidate_ids` requests source-row renewal. Renewal returns the
admitted source IDs only after the shared reader completes the request.
Authorization is admitted when `require_client_session` completes the current
configuration and bearer/grant checks and reads an active durable device record
bound to the exact origin. Revocation published after that check blocks later
requests; an already-admitted bounded page or detail read may finish. A request
whose device-authority read occurs after revocation publication fails before
reading Chat.

## Paired-client message send

`POST /remote/client/v1/agents/{session_id}/messages` requires a live `chat:send`
grant, a bearer token, and one `Idempotency-Key` header in the form
`<idempotency_namespace>.<canonical-lowercase-UUIDv4>`. The JSON body has exactly
`text` and `input_mode`; `input_mode` must be `message`, text must contain a
non-whitespace character, and its UTF-8 encoding is limited to 65,536 bytes.
The raw JSON request body is limited to 512 KiB, including escaping and field
overhead.
The server preserves admitted text exactly. Terminal control characters are
rejected, except for line feed and tab. This prevents message text from
embedding provider submit or escape instructions. The route admits no command,
approval, lifecycle action, attachment, or alternate target field.

The first accepted request atomically writes a `state.db` operation and outbox
row before dispatch. The unique key is `(namespace_id, request_uuid)` and is
independent of target. Repeating the same key and exact target/text/mode returns
the same operation with `202`; changing any payload field returns
`409 idempotency_conflict`, including when the changed target does not exist.
The operation UUID is also the stable Wardian interaction/native intent ID.
The operation row keeps the original namespace expiry and delivery deadline;
broker cleanup does not remove this idempotency proof.

`GET /remote/client/v1/message-operations/by-key` takes the same bearer and
`Idempotency-Key`, with no body or query. It reads the operation only inside
the active namespace. A missing key returns `404 operation_not_found`; an
expired or unknown namespace returns `410
idempotency_namespace_expired_or_unknown` and is never recreated for that key.

The operation DTO reports `operation_id`, `request_id`, `session_id`,
`delivery_state`, `accepted_at`, `updated_at`, `idempotency_expires_at`, and
`error_code`. While the native delivery record exists, `delivery_state` and
`updated_at` follow its persisted phase and timestamp. Startup's
`submitted_unconfirmed` fence is preserved when the broker still reports only
`queued` or `dispatching`. The outer operation retains its last state after
broker cleanup. A lost response is
recovered by querying or repeating the same key and body. A duplicate in
`admitted` may schedule the same operation; once
dispatch is marked `dispatching`, recovery records
`submitted_unconfirmed` and never blindly submits it again. An uncertain result
is status-only. A `failed_before_submit` duplicate remains the same failed
operation; another attempt requires a new user intent and key.

The live-surface send path acquires the per-agent delivery lock before the
remote configuration and device authority gates. Payload preparation,
composer checks, and provider readiness happen before the final authority
recheck. Live delivery reacquires those gates for every terminal write, including
payload and submit-key bytes;
native transports hold them only across the provider write or HTTP submission
acknowledgement; headless delivery holds them only through process spawn. Each
path releases the gates before waiting for a provider receipt or completion, so
revocation or origin removal serializes with the irreversible submission
boundary. The request text, bearer, and namespace are excluded from logs and
error responses. Client errors use
`{ "error": "stable_code" }`; throttled responses include `Retry-After`.

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
headers from the current producer incarnation. These provisional pages issue no
Older cursor, detail capability or row-revalidation ACK. Consumers retain
the marker unchanged and replace the window when it changes or is retired,
including an empty retirement response. Actual canonical publication must
cover every current native observation and version before the reader leaves
preview mode. That transition explicitly resets the window. Only exact aliases
issued by the producer for the current returned revision establish a bridge;
clients do not infer correspondence or reuse aliases from a prior revision.

After a qualified bridge is issued, readers also forward its current opaque
`revision` on Older and detail requests. They omit preview backing hints on
those requests. The revision preserves representation continuity; it does not
replace the cursor or detail reference that authorizes the read.
Passive latest reads can return an empty continuity envelope for that validated
admission. It carries no rows or read capabilities; readers retain their loaded
window and use the existing Head-change revalidation rules.

Both authenticated Chat read routes accept the optional `preview_backing_ids`
query value as one URL-encoded JSON array, for example
`preview_backing_ids=%5B%22preview-id%22%5D`. It contains at most eight nonempty,
non-whitespace IDs, each at most 512 UTF-8 bytes. An initial empty array is valid.
The parameter requires `recent_preview=true` and cannot accompany `before`,
`detail` or `revalidate_ids`, including an empty revalidation array. Repeated
query keys, malformed JSON, non-string members and any invalid member reject
the whole request with the existing HTTP 400 `agent_chat_projection_unavailable`
error. Omitting the parameter preserves the existing read behavior.

These are client-forwarded hints for the selected agent's preview backing IDs.
The gateway forwards them through the existing authenticated agent read scope;
the producer resolves them against its current source and publication proof.
They grant no additional authority and issue no Older cursor, detail capability
or row-revalidation ACK. Valid unknown, out-of-scope or stale IDs expose no
lookup or differentiated error oracle. Only producer-proven exact aliases in
the current returned revision can establish canonical identity; a hint alone
cannot establish a match or renew a prior one.

Certification belongs to the recent admission and is shared with desktop and
other readers. Reopening while the admission lives retains unresolved gaps and
sticky overflow state. A new admission re-evaluates the source. Reader-specific
display limits and hints do not reset that shared state.

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
