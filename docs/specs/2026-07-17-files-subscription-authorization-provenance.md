# Files Subscription Authorization Provenance

- **Status:** Approved
- **Date:** 2026-07-17
- **Primary issue:** [#392](https://github.com/wardian-app/Wardian/issues/392)
- **Extends:** `docs/specs/2026-07-16-files-artifact-review-surface.md`

## Context

Files presentations are canonical-resource keyed so multiple panes can share one
descriptor, watcher, and revision stream. The same canonical file can still be
opened through different requested paths: for example, one pane through a
workspace junction and another through the direct directory. Canonical resource
deduplication must not erase that distinction because each requested path has
independent authorization provenance.

Keeping one `AuthorizedPath` on the shared resource lets the first or most
recent open become an accidental authority for every subscriber. If that path
is an alias which is later removed or retargeted, it can make a valid direct
subscriber unavailable. Conversely, replacing it with the direct path can let
the alias subscriber read through provenance it never established.

## Decision

Each file subscription retains both its access claim and the exact
`AuthorizedPath` created for its requested pathname. The canonical resource
entry retains only shared state: watcher, descriptor, revision token, monotonic
revision, and subscriber map.

Reads and renderer-ticket issuance validate the calling subscription's own
claim and requested pathname. They do not borrow authorization from another
subscriber. Agent access is rechecked against the current primary and
additional directories. Picker access remains exact; after relaunch a durable
backend path record may mint a new live capability, but grant deduplication by
canonical target never widens either subscription's retained pathname.

Refresh uses the same live-claim boundary before it reads content. In the
desktop runtime, agent configuration is resolved from the AppHandle's managed
`AppState`; a removed agent or changed root cannot drive a descriptor or hash
revision. Picker candidates must still have their exact backend capability.
Revoked candidates fall through without scanning, and a later valid candidate
may refresh the shared resource.

Refresh tries active subscription authorizations in deterministic requested-
path and subscription-ID order. A valid candidate may refresh the one shared
descriptor and revision. Editor-style atomic replacement is reauthorized only
when that candidate's original pathname still resolves to the same canonical
target under the same root. An alias removal or retarget invalidates that
candidate but does not prevent a later direct candidate from refreshing the
resource. The resource publishes a shared unavailable revision only when every
active candidate fails.

If subscriptions change while a refresh is scanning, Wardian discards that
application and schedules a fresh pass. Closing a subscription removes only its
claim and authorization provenance; the canonical watcher closes after the last
subscriber. Joining an existing resource and closing a subscription while
others remain both schedule debounced reconciliation. This recovers an
alias-only unavailable descriptor when a valid direct subscriber joins, and
marks the resource unavailable after its last valid candidate closes, without
depending on a filesystem event.

Picker capabilities keep `in_flight_uses` and `active_subscriptions` in the
same grant record and under the same mutex used for eviction. A successful open
atomically transitions one in-flight use into one active subscription. Close
removes the subscription before decrementing that active count. Eviction is
allowed only when both counters are zero, eliminating stale cross-lock
membership snapshots.

## Required Invariants

1. One canonical file has at most one active watcher and revision stream.
2. Every active subscription has exactly one independently validated requested
   pathname and claim.
3. Alias removal or retarget rejects alias reads and tickets without revoking a
   valid direct subscription.
4. A valid direct subscriber can recover and publish an atomic replacement even
   when an earlier deterministic alias candidate fails.
5. Concurrent first opens retain both authorization paths regardless of which
   open installs the shared entry.
6. Exact picker capabilities never authorize siblings, parents, or another
   subscription's requested pathname.
7. A candidate whose agent or picker claim is no longer live never reaches the
   descriptor scanner or publishes a new content hash.
8. Grant eviction cannot remove a capability with an in-flight open or live
   subscription.

## Verification

Cross-platform Rust regressions use directory symlinks on Unix and junctions on
Windows. They cover alias removal for text reads, picker-alias retarget for PDF
tickets, direct recovery after atomic replacement, subscription-local close,
and a barrier-controlled concurrent first-open race. The pre-existing direct
picker atomic-replacement regression remains the baseline for single-
subscription behavior.
