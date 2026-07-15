# Terminal Owner Continuity After Clear

## Problem

Clearing or replacing an agent creates a new PTY runtime generation. The new
terminal broker actor correctly starts without an owner and requires every
presentation to register against the replacement generation. The desktop
client previously discarded which presentation owned the prior generation.
When a React rebind raced the broker lifecycle event, it could also update the
replacement actor before re-registering and surface `PresentationNotFound` as
a terminal initialization fatal error.
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

`PresentationNotFound` recovery remains a defensive fallback for a missed or
racy lifecycle notification. It is not the expected clear path.

## Verification

- A presentation that owns generation 1 is re-registered for generation 2,
  completes activation, and sends input using generation 2 and its current
  lease epoch.
- A replacement that already has an owner is not taken over.
- A removed or ineligible previous owner is not restored.
- A paused-generation presentation update and snapshot request issue no stale
  IPC before replacement registration completes.
- Existing explicit click and keyboard activation behavior remains unchanged.
- Native runtime coverage proves input before clear and verifies that the
  replacement presentation remounts without an initialization fatal error.
- Client integration coverage proves the recovered generation and lease accept
  presentation-aware input; browser-only tests cannot prove PTY delivery.
