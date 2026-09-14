# Pi interactive content receipts

Issue: #1244. Scope: ordinary stock Pi TUI launches managed by Wardian.

## Problem and boundary

Pi can retain a fresh session's user message in memory until the first assistant
message completes. Reading the transcript cannot prove acceptance within the
existing ten-second receipt deadline. Spinner activity, terminal echo, `input`,
and `agent_start` alone do not prove that Pi accepted the submitted content.

Wardian observes supported extension `message_start` events whose native role is
`user`. A receipt requires exact normalized submitted UTF-8 content (SHA-256 plus
byte length), a record beginning after the pre-write file boundary, the expected
provider session and launch, and publication under the current terminal runtime
generation. One managed prompt is pending per launch, within the existing
per-agent delivery serialization. There is no new provider request-ID carrier.
Identical concurrent manual input remains an operational ambiguity, as with the
other native content receipts. A user extension is trusted code in the provider
process, not an adversary isolated by this mechanism.

Only nonempty plain messages qualify. Slash and shell commands fail the managed
receipt preflight. Extension-handled input without a native user event cannot
acknowledge anything. A transformed native message has a different digest and
cannot acknowledge the original. Multimodal native messages remain activity but
cannot satisfy a plain-text receipt. Queued inputs qualify only when the agent
loop actually consumes them; queue insertion alone is insufficient.

## Ownership and ordering

The launcher creates an exclusive per-launch directory below the agent's Pi
state directory, writes an extension with an immutable descriptor, and passes
its absolute path as a separate `--extension` argv value. It does not alter global
Pi configuration or disable user extensions. The extension opens only the
precreated append stream and retains its descriptor. Unix directory/file modes
are 0700/0600; Windows uses the owned home directory's inherited permissions.
This is process ownership, not a new filesystem security boundary.

`session_start` in TUI mode advertises the expected session identity and stream
version. A missing handshake refuses managed input before payload writes.
`agent_start` and `agent_end` delimit activity; only user `message_start` publishes
a receipt. These hooks append synchronously before returning to Pi. Each record
contains launch/session identity, an extension instance ID, sequence, kind and
optional content digest/length. Prompt bodies and model metadata are absent.
Records are limited to 2 KiB and the launch stream to 256 KiB, without rotation.
Reload, changed identity, sequence gaps, invalid records, replacement, truncation
and capacity failure stop acceptance. Unsupported or failed extensions cannot
silently fall back to log-based acceptance.

The existing Pi watcher reads complete records and retains partial tails. Ticket
arming captures the current on-disk length, including any partial record, so an
old unread record cannot become new evidence. Duplicate sequence records are
ignored. Input writes retain the exact captured terminal generation. The broker
holds its runtime map read lock across generation validation and `turn_started`
publication, reusing the atomic publication boundary from the OpenCode receipt.
Only after successful publication does the matching pending ticket become
accepted. Each native user event increments activity once; delayed JSONL user
records remain available to transcript/raw-event consumers but do not count or
publish another start. JSONL still owns transcript projection and completion.

The child handle retains receipt ownership through cancellation. A retained stop
worker kills and polls only that child, joins the receipt watcher, then removes
only its known files and empty directory. Uncertain exit or changed file identity
retains files. New launches use distinct directories; the weak registry does not
keep old launches alive. No live appender's files are intentionally removed.

## Failure and acceptance limits

The existing ten-second post-submit deadline is unchanged. Timeout or failure
after a write means uncertain submission, never replay. Receipt is acceptance,
not completion, successful model output, or durable transcript persistence.
Provider startup/footer readiness is a separate dependency; this change does not
select menus or alter startup detection. Headless Pi retains its existing path.

Deterministic extension tests cover ordering, transformed/handled inputs, queued
and identical inputs, identity/reload/capacity failures, non-TUI mode and actual
file append with Unicode paths. Rust tests cover pre-write boundaries, partial
records, matching, replacement generations, delayed-log de-duplication, pending
cancellation and owned cleanup. Parent verification must additionally exercise
the compiled delivery path and real fresh/resumed Pi TUI acceptance; callback
unit tests alone do not establish native compatibility.
