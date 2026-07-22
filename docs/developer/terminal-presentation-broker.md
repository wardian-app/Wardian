# Terminal Presentation Broker

One agent runtime can appear in Agents, one or more Agent Session
surfaces, and authenticated remote clients at the same time. The Rust terminal
session broker makes those views independent presentations of one PTY without
allowing them to fight over input or geometry.

## Core Invariants

For each live terminal session:

1. At most one presentation owns input and canonical PTY geometry.
2. Every presentation consumes the same ordered output stream.
3. Mount, focus, restore, viewport observation, and visibility changes never
   acquire ownership implicitly.
4. Mirrors never resize the PTY.
5. Split, move, hide, suspend, and close affect presentations, not the runtime.
6. Desktop and remote clients use the same generation, lease, snapshot,
   sequence, and geometry rules.

Structured prompt delivery is a separate backend-authorized control path. The
terminal lease applies to terminal keystrokes and binary input; it must not gate
Inbox, Chat, Commands, mailbox, workflow, or broadcast delivery.

## Session, Presentation, and Consumer

The broker actor is created with the PTY runtime and survives with zero
presentations. It owns canonical geometry, a VT parser, bounded replay, the
current runtime generation and lease epoch, presentation records, and feed
consumers.

A presentation registration includes:

- `presentation_id` and `session_id`;
- `client_kind`: `desktop` or `remote`;
- non-authoritative `desired_geometry`;
- `visibility` and `render_state`;
- requested `interactive` or `read_only` capability;
- the client's observed lease epoch.

IDs must be non-empty, contain no whitespace, and stay within 512 UTF-8 bytes.
One session accepts at most 64 desktop presentations and three remote
presentations. The broker trusts the local desktop identity; a remote
presentation must match its authenticated attachment identity. Requested
interaction can only reduce the capability allowed by that identity.

Presentations and feed consumers are deliberately separate. One desktop
consumer fans a session feed out to all local renderers. Each authenticated
remote socket has its own cursor. Hidden presentations do not receive private
queues.

## Generation, Epoch, and Sequences

These counters solve different stale-client problems:

| Value | Changes when | Rejects |
|---|---|---|
| `runtime_generation` | the PTY runtime is created or replaced | input, resize, cursors, or activation for an older runtime |
| `lease_epoch` | ownership is revoked or a transfer begins | input, resize, or activation based on stale ownership |
| `stream_sequence` | an ordered output, geometry, ownership, or lifecycle event is emitted | missing or reordered feed application |
| `geometry_sequence` | an owner proposes a new resize | reordered resize requests from the same owner |
| `interaction_sequence` | a presentation participates in an interaction/activation | nondeterministic fallback ownership based on client clocks |

Resume, clear, or PTY replacement increments the runtime generation and lease
epoch, revokes owners and pending activation, resets parser/replay state, and
requires fresh presentation synchronization. A reused session ID therefore
cannot let a stale renderer write to a replacement PTY.

The desktop client treats `runtime_paused` as the beginning of that transition.
It stops issuing presentation updates and snapshot requests to the paused
generation, remembers the current owner, and waits for the replacement
generation before re-registering. A `PresentationNotFound` retry is only a
fallback for missed or racy lifecycle delivery, not the normal clear flow.

Lifecycle delivery can race the ordered event subscription. If a replacement
snapshot advances the client generation before `runtime_replaced` arrives, the
same-generation lifecycle notice still rebuilds the presentation registry
once. The recovered owner then re-reports its renderer geometry because
viewport acknowledgements from the previous generation cannot size the new
PTY.

After synchronization, a client may restore the exact presentation that owned
the immediately preceding generation through the ordinary two-phase activation
protocol. Restoration is allowed only while that presentation is still
visible, mounted, interactive, registered, and the replacement remains
ownerless. This preserves an existing user ownership choice without making
mount, focus, or registration implicit activation signals, and without letting
registration order select a new owner.

## Ordered Stream and Recovery

PTY reads and broker state changes are serialized by the per-session actor.
Every event carries `runtime_generation` and a monotonic `sequence`. Replay is
bounded to 4,096 events and 1 MiB of raw output; the older bound wins.

Consumers pull batches after a cursor. The broker clamps each batch to 256
events and 256 KiB, and clients acknowledge the highest applied sequence for
lag diagnostics. Acknowledgements do not pin retention. Desktop wakeups are
coalesced over 16 ms so bursty output does not create one UI event per PTY read.

A snapshot contains canonical geometry, generation, a unique ID, a
`sequence_barrier`, VT terminal state, the visible grid, and up to 1,000 lines
of scrollback. Its serialized payload is capped at 2 MiB. Apply it as follows:

1. Replace the local renderer/parser state with the snapshot.
2. Set the cursor to `sequence_barrier`.
3. Ignore events at or below the barrier.
4. Apply only consecutive later events.

An expired replay cursor, missing sequence, generation change, or terminated
runtime is a structured batch result. Gap and generation-change results include
a recovery snapshot and new barrier. Slow clients resynchronize instead of
growing an unbounded backlog.

## Explicit Ownership Transfer

Activation is a two-phase protocol:

1. Begin with `presentation_id`, `runtime_generation`, and the observed
   `lease_epoch`.
2. The broker verifies the presentation is visible, mounted, interactive,
   synchronized, and backed by a live runtime. It advances the epoch, records a
   pending activation, removes the active owner for the transfer window, and
   returns an activation ID plus snapshot/barrier.
3. After applying the snapshot, acknowledge the exact activation ID,
   generation, and epoch. The broker applies at most one current desired
   geometry and publishes the new owner.

Input and resize are rejected while a transfer is pending. If acknowledgement
does not arrive within five seconds, the broker rolls back to the previous
owner only when that presentation is still eligible; otherwise the PTY remains
ownerless. Duplicate begin and acknowledgement calls for the same transfer are
idempotent. Stale identity, generation, epoch, activation, or geometry requests
return a lease decision without disconnecting an otherwise healthy client.

When an established owner loses its presentation, the broker may promote the
eligible presentation with the highest server-owned interaction sequence. A
candidate must be visible, mounted, interactive, synchronized, and connected to
a live runtime. If there is no candidate, canonical geometry remains unchanged
until the next explicit activation.

Suspending a current owner marks it as requiring resynchronization and disables
input. Remount uses an owner-resync begin/snapshot/ack protocol. It retains the
same generation and epoch only if no competing activation has won in the
meantime.

## Geometry and Fitting

`report_terminal_presentation_viewport` records a presentation's latest desired
size without resizing the PTY. Only an active lease holder can send a
presentation-aware resize with a strictly increasing `geometry_sequence`.
Native PTY resize, parser resize, canonical geometry update, and the geometry
event are one serialized commit.

Geometry limits are:

| Client | Columns | Rows |
|---|---:|---:|
| Desktop | 20..500 | 8..200 |
| Remote | 20..240 | 8..80 |

Mirrors render the owner's canonical grid. They keep natural scale when it
fits, reduce local scale only to the configured readability floor, pan if it
still cannot fit, and letterbox when the viewport is larger. The default floor
is 0.75 and cannot be configured below 0.5. A phone, narrow split, or remote
viewport therefore never degrades the desktop PTY by imposing
smallest-client-wins geometry.

### Alternate-screen application ownership

The broker owns PTY transport, presentation leases, canonical geometry,
snapshots, and ordered output. A terminal application still owns the protocol
state it negotiates inside that transport. When an application such as OpenCode
enters the alternate buffer and enables xterm mouse tracking, xterm forwards
click, drag, hover, and wheel reports to the application instead of treating
them as Wardian or xterm scrollback gestures. Remote touch input adapts vertical
travel into the same wheel protocol only while that alternate-screen mouse mode
is active.

Snapshots and replay preserve alternate-buffer, mouse-mode, and synchronized-
output controls. Synchronized-output boundaries are renderer instructions, not
broker events: xterm holds intermediate row refreshes until the application
closes the frame. Normal-buffer presentations may use rendered-row geometry to
fill a host, while alternate buffers use xterm's measured cell grid so
application-owned layout is not inferred from stale row DOM.

Text and binary input share a dedicated FIFO in the desktop session client.
This preserves the byte order emitted by xterm when rapid keyboard and mouse
events would otherwise create concurrent Tauri IPC requests. The input FIFO is
separate from snapshot, event-drain, and geometry serialization so a renderer
refresh cannot delay interactive input or create a resize backlog. The final
presentation drains that FIFO before unregistering and releasing the session
client, and marks itself closing before that drain begins. Already accepted
input completes in order, while late renderer callbacks cannot append work to a
client being released or be overtaken by a replacement presentation.

## Desktop and Remote Consistency

`TerminalSessionClient` owns one ordered desktop subscription per session and
fans snapshots and events to independent xterms. Each surface/card has a stable
presentation ID and its own renderer, local scale, pan, and viewport state.

The authenticated remote WebSocket registers a remote presentation and
consumer, then uses the same activation, input, resize, snapshot, events, ack,
and owner-resync DTOs. Remote authentication and attachment identity remain
mandatory, and existing socket backpressure and rate limits still apply.
Non-owner or stale requests are nonfatal protocol results, not reasons to close
the socket.

## Renderer Budgets

The desktop process permits at most 24 mounted xterm renderers and 12 WebGL
contexts. The pools are independent deterministic LRUs:

- touching a visible or interacted presentation keeps it warm;
- WebGL eviction falls back to xterm's DOM renderer;
- xterm eviction keeps the logical presentation registered, marks it for
  synchronization, and restores it from a broker snapshot when needed;
- an Agents presentation may be hidden while its budgeted xterm remains
  mounted; hidden still disables input and reveal, but does not itself discard
  renderer state;
- an ordinary component unmount has a 30-second disposal grace so moves and
  zoom changes can reuse the renderer.

Renderer residency and presentation visibility are deliberately independent.
The Agents surface keeps its current bounded resident set across short tab
switches. A presentation outside that set is suspended and consumes no xterm
budget. Hiding a resident presentation updates the broker to `hidden` while
leaving its xterm mounted, so returning to Agents does not cause a simultaneous
registration, snapshot, and renderer-construction burst.

### First-paint reveal barrier

A new or restored xterm is not visible as soon as its DOM node exists. Its
current reveal generation must complete this transaction first:

1. confirm connected, non-zero host bounds and physical intersection;
2. select WebGL or settle on the complete DOM fallback;
3. fit the local xterm to the measured host;
4. apply and await the broker snapshot or recovery write;
5. verify that the host, backend, proposed grid, and actual grid still agree;
6. reveal the host atomically.

Registration's pre-snapshot hook keeps backend selection and the first fit
inside the client's serialized registration transaction. Suspending,
replacing, hiding, or changing the renderer invalidates the reveal generation,
so stale font, snapshot, or intersection continuations cannot reveal a newer
renderer. Resize observation schedules at most one animation-frame fit and
does no work when measured pixels are unchanged. Timer expiry is never used as
evidence that terminal geometry has settled.

Graph and Garden have a separate 30-second heavy-child grace through their
surface render policy. Neither grace weakens broker ownership or changes PTY
geometry.

## Presentation Close Versus Runtime Lifecycle

Unregistering a presentation removes only that view. If it was the owner, the
broker advances the epoch and applies the fallback rule. If it was the final
presentation, the runtime, parser, replay ring, and canonical geometry remain
alive.

Runtime commands are explicit and backend-owned:

- pause retains broker state but makes runtime input and activation unavailable;
- resume/replacement establishes a new generation and requires resync;
- kill terminates the broker/runtime and notifies consumers;
- none of these are implied by closing a tab, group, workbench, remote socket,
  or renderer.

Persisted workbench tabs are not pruned when a runtime disappears. They restore
as unavailable placeholders and can recover when the resource returns.

## Compatibility Boundary

Legacy raw desktop input/resize and remote v1 paths are temporary adapters to
the same broker. They must not introduce a second owner or geometry writer. New
workbench code uses presentation-aware commands carrying session,
presentation, generation, epoch, and geometry sequence.

Provider readiness and structured interaction delivery are separate from the
presentation lease. Do not use renderer focus, terminal repaint text, or a
fixed delay as evidence that a provider is ready for a structured message.

## Verification Boundaries

- Rust unit tests prove identity, limits, activation races and timeouts, stale
  lease rejection, fallback, resync, replay gaps, bounded snapshots, generation
  changes, geometry ordering, and shutdown.
- Frontend unit/integration tests prove desktop fan-out, cursor recovery,
  renderer budgets, mirror fitting, presentation state updates, and explicit
  activation UI.
- Browser E2E can prove presentation controls and mirror labels using mocked
  data, but cannot prove PTY resize, IPC, or native stream ordering.
- Native E2E proves desktop owner/mirror races, stable geometry, gap-free
  output, close-without-kill, safe-mode continuity, and authenticated
  desktop-to-remote-to-desktop transfer.
- Real-provider native E2E is required only for claims about a specific CLI's
  prompt, TUI redraw, resume, or provider-specific behavior.

Run the focused native workbench package after rebuilding native assets:

```bash
npm run tauri -- build --debug --no-bundle
npm run test:e2e:native:workbench
```

The same commands apply in PowerShell. Always run with the harness's isolated
`WARDIAN_HOME`; never point a test at a production Wardian home.
