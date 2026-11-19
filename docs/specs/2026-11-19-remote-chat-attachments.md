# Remote Chat Attachments

- **Status:** Implemented with path-only delivery
- **Date:** 2026-11-19
- **Issue:** #1753
- **Predecessor:** [Chat composer and attachment pipeline](2026-08-16-chat-composer-and-attachment-pipeline.md)

## Outcome and boundary

The mobile PWA can attach a photo or file to one agent's Chat composer. One
compact attach button offers Files and Camera. Uploaded files use the same
prompt-reference formatting contract as desktop attachments.
The Rust gateway owns upload authorization, bytes, metadata, and lifetime;
the PWA holds a transient view of that state.

This slice serves paired PWA devices through `/remote/api` session auth.
Pairing alone is insufficient: the device must first establish its normal
authenticated PWA session. The isolated `/remote/client/v1` bearer protocol
and its text-only message contract remain unchanged. There is no public file
download route, arbitrary destination path, remote URL fetch, multipart batch,
video/audio support, or background/offline upload queue.

## Authenticated upload contract

Add `POST /remote/api/agents/{session_id}/attachments/{upload_id}` with a raw
binary body. The client generates a canonical UUIDv4 before uploading and
retains it until removal or confirmed submission. A single encoded filename
header carries the display basename; `Content-Type` describes the file bytes.
DTO properties use snake_case. Success returns `upload_id`, sanitized `name`,
`size_bytes`, validated `media_type`, and `expires_at`, never a host path.

Before consuming bytes, apply the existing audited mutation boundary:
canonical gateway Host, exact allowed Origin, live PWA session and current
device authority, `x-wardian-csrf` matching that session's nonce, mutation rate
limit, and target-agent authorization. Reject missing/null/mismatched origins
and revoked devices. Recovery GETs follow the existing read-only gateway policy:
canonical Host and cookie authority are mandatory; supplied foreign Origin or
cross-site fetch metadata is rejected, while an absent Origin is allowed for
ordinary same-origin browser fetches. Their owner binding uses the canonical
gateway origin. Upload, deletion, status, and submission all recheck
current authority. Do not widen CORS or accept paired-client tokens as PWA
sessions. Authentication must run before the streaming body extractor.

Bind each record to the device, authenticated origin, and target agent. Session
renewal by the same device and origin may recover it; another device, origin,
or target cannot enumerate, use, overwrite, or delete it. Unknown and foreign
IDs return the same not-found read response and idempotent successful deletion
response; a foreign deletion changes no bytes. Recheck authority before publishing a
completed upload and immediately before irreversible provider submission.

`GET` on the same resource returns metadata/status without bytes; `DELETE`
removes an unsubmitted upload idempotently. Both require session/origin
authorization; DELETE also requires CSRF. Responses use `Cache-Control:
no-store`. Service-worker caching excludes these routes and binary bodies.

Repeating a completed upload ID with identical validated name/type/bytes
returns its existing metadata without extending its expiry. Different content
returns 409. Compute the digest while streaming. Concurrent writers for the
same ID cannot publish two records. A status lookup resolves a lost response;
an incomplete attempt may be restarted under the same ID after its writer
has stopped. Upload retries never send a prompt.

## Bounded input and storage

- Maximum file size: 10 MiB, enforced by counted streaming bytes even without
  Content-Length. Reject an oversized declared length before reading.
- Maximum four attachments and 25 MiB per submitted message.
- Maximum eight pending files / 50 MiB per device, 200 MiB pending globally,
  and two active uploads per device. Reserve capacity before accepting bytes;
  concurrent requests cannot exceed quotas. Upload timeout is 60 seconds.
- Allow PNG, JPEG, WebP, GIF, BMP, PDF, UTF-8 plain text, Markdown, CSV, and
  JSON. Validate extension and type together, image decoding and signatures,
  PDF signature, and UTF-8/text structure as applicable. Images have a
  25-megapixel decoded ceiling, checked before allocation. Reject SVG, HTML,
  executable formats, archives, and unknown or mismatched types. Camera
  HEIC gets an explicit unsupported-format error; no implicit conversion.
- Names are display data only: bounded to 128 UTF-8 bytes, with separators,
  control characters and traversal components rejected. Storage filenames
  are generated UUIDs with validated extensions. Names never become shell
  input, HTML, terminal controls, or filesystem destinations.

Write bytes under the target workspace at
`<agent-workspace>/.wardian/attachments/remote/<upload-id>/`, with authoritative
metadata under `<wardian-home>/remote/attachments/`. The server resolves the
workspace from the current agent configuration. Use private permissions
where supported, an exclusive temporary file, and atomic publication after
validation. The gateway generates every path. Reject symlink/junction escapes;
cleanup never follows links or trusts client-supplied paths. Do not execute
uploads or render their contents as HTML. Local thumbnail previews use object
URLs only for validated browser image types and revoke URLs on removal/unmount.
Each owned upload root has a managed `.gitignore` that excludes all its
contents from normal Git staging. Existing project ignore rules are untouched;
a conflicting or linked marker fails closed instead of being overwritten.

Pending uploads expire after one hour, independent of retries. A persisted
metadata record tracks ownership, bytes, expiry and delivery association.
Removal, failed uploads, cancellation, revocation and expired pending records
release reservations and delete their bytes. Startup and a periodic sweep
remove abandoned temporary files and expired records, including after crashes.
Run cleanup only under recorded dedicated attachment roots.

Claude in normal permission mode reads inside its working directory with Read;
Codex reads inside its workspace with filesystem/image tools. No additional
read grant or bypass flag is needed. An explicit narrower read-deny policy
retains its ordinary permission prompt or denial. Test with a non-bypass Claude
agent and a Codex agent. Local agents sharing a workspace can inspect these
files; gateway authorization does not isolate them from local filesystem access.

Submitted files remain available for seven days after acceptance so asynchronous
agents can read their references. Pin files during an active delivery attempt;
delete after that pin is released and retention expires. Submitted storage has
a separate 1 GiB global cap: reject new admission rather than evict files still
within retention. Enforce pending and retained quotas together at transfer.
The UI identifies attachments as temporary; permanent knowledge belongs in the
agent workspace through normal agent work. No file bytes, original names,
prompt contents, tokens, or absolute paths enter audit logs or remote errors.

## Submission and desktop pipeline handoff

Extend only the PWA `send_prompt` action with optional `attachment_ids` and a
client-generated `submission_id` in the form `<issued_epoch_ms>.<UUIDv4>`.
New IDs must be no more than five minutes old or sixty seconds in the future.
Known IDs retain a fixed 24-hour recovery/idempotency lifetime. Unknown old IDs
and expired proofs return 410 and are never recreated, so removing expired
proofs cannot cause a stale request to dispatch again. Attachment-bearing sends require message
mode; reject command mode, provider-choice replies and alternate target fields.
Resolve IDs on the server, verify uniqueness/count/aggregate size and ownership,
then atomically associate all files with that submission. Clients cannot supply
paths. Permit attachment-only messages with the existing fallback text, “Please
inspect the attached files.” Existing text-only sends keep their behavior.

Use the formatting contract of `promptWithChatAttachments`: append an Attached
files list containing server-resolved paths. Extract or mirror this small
contract in a tested backend helper rather than accepting formatted references
from the phone. Preserve safe generated basenames and literal path references.

Deliver every attachment, images included, as a server-resolved path reference
through ordinary prompt delivery. Agents read images with their own tools
(Claude Read or Codex view_image). No clipboard mutation, provider paste key,
keystroke image staging or unsupported-image-provider gate is introduced.
Structured image inputs over non-keystroke transports (Codex app-server or a
future Claude mod attachment API) are future enhancements outside this slice.

Persist the submission association and its exact target/prompt/ordered IDs
before provider side effects. Repeating the same submission ID and payload
returns its receipt/state; a changed payload returns 409. A file claimed by
another live submission cannot be reused implicitly. After a terminal proof
expires, an explicit new user intent may reclaim same-owner retained files
with a new key and refresh their retention; live dispatch fences remain pinned.
A status lookup by submission
ID allows recovery after response loss. Record a dispatching fence before the
first provider submission. Restart or ambiguous failure after this
fence yields `submitted_unconfirmed` and prohibits automatic replay. A proven
failure before side effects releases claims for an explicit retry. Removing a
chip after acceptance does not retract delivery or delete retained files.

Bound the submission ledger to 2,048 records and 8 MiB; refuse new admission
before side effects if either limit would be exceeded. Compact terminal records
to payload-digest/status tombstones instead of retaining both prompt copies.
Cleanup removes expired terminal proofs, preserves active fences and keeps the
independent seven-day file retention promise. Startup reads are bounded too.
An authenticated status 404 means no proof was found; the UI offers an explicit
retry with the same key and exact frozen payload. It does not silently generate
a new key. A 410 permanently rejects that old key, allowing an explicit new
user intent without a delayed old request later submitting.
An expired file uses `attachment_expired`, separately from `submission_expired`.
The composer preserves the prompt and shows expired file chips as failed until
the user removes and attaches them again; it does not repeatedly send dead IDs.

## Compact composer and failures

Keep the prompt and send button in their existing row. Add one icon button
with accessible label “Attach file”; its small menu offers Files and Camera.
Files uses a hidden file input with supported MIME types/extensions in accept
so iOS can offer JPEG camera conversion; Camera uses an image input with
`capture="environment"` as a platform hint and normal picker fallback. No
persistent toolbar, modal attachment editor, or separate upload screen.

Chips occupy one horizontally scrolling strip only while files exist. Each
shows a small image thumbnail or file glyph, truncated name, and accessible
remove action. Show determinate upload progress using an upload-progress API
(for example XHR); 100% transferred remains validating until server acceptance.
Keep touch controls at least 40 CSS pixels and use semantic theme tokens.

States are selected → uploading → validating → ready, with failed/cancelled
branches. Remove aborts an active request, drops the local chip, and attempts
authenticated deletion; TTL cleanup covers lost cancellation responses. Failed
chips expose concise error text and explicit retry/remove controls. Send is
disabled while any retained chip is incomplete or failed, or the target is
unavailable. A successful send clears the prompt and chips only on receipt.
An uncertain send keeps its submission ID and offers status recovery, never
automatic resend. A definite pre-submit failure preserves ready attachments.

Use stable sanitized codes for auth/CSRF, invalid filename/type/image,
file-too-large, quota/rate limits, expired upload, busy writer, disk failure,
unsupported provider, delivery failure and uncertain delivery. Respect
Retry-After; do not retry auth failures or upload bytes in the background.
Session expiry requires reconnect/authentication before recovery. Reload may
discard local File objects; server expiry still guarantees orphan cleanup.

## Evidence required after approval

1. Backend tests exercise real HTTP streaming and auth ordering, Host/Origin,
   CSRF, revocation, cross-device/target access, traversal/link escape,
   chunked oversize, type/signature/decoded-size limits, quotas under
   concurrency, atomic cleanup, restart recovery, expiry and sanitized errors.
2. Delivery tests prove ID-to-path resolution, text/image path-reference parity, normal-permission provider reads,
   all-or-nothing admission and
   duplicate/uncertain submission behavior. Use the lowest layer that can
   exercise each real contract; mocked browser IPC proves no native delivery.
3. Frontend tests cover picker/camera inputs, preview/removal, progress,
   cancellation, explicit retry, attachment-only send, auth expiry, disabled
   send and preservation/recovery after failure.
4. Native E2E uploads through an authenticated gateway and verifies the agent
   receives a file and image through the real attachment handoff. Browser E2E
   covers the compact mobile UI; native-only gaps are explicitly marked.
5. Capture mobile screenshots for a ready photo chip, progress/error, and the
   composer after removal. Update the remote-control guide and gateway
   reference. Run `npm run verify:ci`, obtain a security-focused local-agent
   review with zero blocking findings, publish an issue-linked PR, and monitor
   all applicable hosted checks on its latest commit. Do not merge.
