# Terminal Owner Continuity After Clear

## Problem

Clearing or replacing an agent creates a new PTY runtime generation. The new
terminal broker actor correctly starts without an owner and requires every
presentation to register against the replacement generation. The desktop
client previously discarded which presentation owned the prior generation.
When a React rebind raced the broker lifecycle event, it could also update the
replacement actor before re-registering and surface `PresentationNotFound` as
a terminal initialization fatal error.
The ordered event subscription can also observe the replacement snapshot
before Tauri delivers `runtime_replaced`. In that ordering, the lifecycle
notice has the same generation as the client's current snapshot. Treating it
as informational leaves the replacement registry ownerless even though output
continues normally.
The terminal could therefore resume rendering while its still-focused xterm
sent input with an ownerless lease, making typing appear permanently broken
until another explicit activation gesture happened to run.

## Decision

`TerminalSessionClient` treats the current generation's `runtime_paused`
lifecycle notification as the start of replacement, before clear removes the
old presentation registry. It remembers the owner and suppresses presentation
updates and snapshot requests against that stale generation. When the broker
announces the replacement generation, the client first re-registers
presentations, applies replacement snapshots, and restores the desktop event
subscription. It may then run the normal two-phase activation protocol for the
previous owner when all of these remain true:

- the same presentation is still registered;
- it is visible and mounted;
- it remains interactive; and
- the replacement runtime is still ownerless.

This is continuity of an existing user choice, not implicit ownership from
focus, mount, or registration. Wardian must never choose a different
presentation merely because it re-registered first. A hidden, suspended,
read-only, removed, or superseded owner remains unowned until a user explicitly
activates an eligible presentation.

The desktop client tracks continuity only for presentations registered in that
client. A remote takeover changes the broker's live owner, but it does not
replace the desktop client's remembered local owner with a remote presentation
ID that the client cannot re-register. If clear replaces the runtime while a
remote presentation owns it, the last eligible local owner is restored after
the replacement unless another presentation has already claimed the new
generation. This preserves remote control during the existing generation while
preventing the replacement desktop terminal from becoming permanently
ownerless.

`PresentationNotFound` recovery remains a defensive fallback for a missed or
racy lifecycle notification. It is not the expected clear path.

A same-generation `runtime_replaced` notification still initiates recovery
when that replacement generation has not already been recovered. This makes
snapshot-first and lifecycle-first delivery equivalent while preventing a
duplicate notification from repeatedly rebuilding the registry.

Viewport acknowledgements are generation-scoped too. Recovery invalidates the
previous generation's last-reported geometry and, after ownership is restored,
fits the existing renderer and commits its current columns and rows through the
normal owner resize protocol.

## Verification

- A presentation that owns generation 1 is re-registered for generation 2,
  completes activation, and sends input using generation 2 and its current
  lease epoch.
- A replacement that already has an owner is not taken over.
- A remote owner is honored while its generation is live, and a replacement
  generation restores the last eligible local owner rather than attempting to
  activate the remote presentation ID through the desktop client.
- A removed or ineligible previous owner is not restored.
- A paused-generation presentation update and snapshot request issue no stale
  IPC before replacement registration completes.
- Existing explicit click and keyboard activation behavior remains unchanged.
- Native runtime coverage proves input before and after clear, verifies that
  the same presentation owns generation 2, and compares its reported columns
  and rows with the recovered renderer geometry.
- Client integration coverage proves the recovered generation and lease accept
  presentation-aware input; browser-only tests cannot prove PTY delivery.
