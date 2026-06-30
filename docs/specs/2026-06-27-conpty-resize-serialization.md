# ConPTY resize serialization

- **Status:** Implemented
- **Date:** 2026-06-27
- **Area:** Windows PTY runtime, agent terminal resize handling, user terminal resize handling.

## Context and Problem Statement

Wardian hit a Windows `Wardian.exe` crash during startup restore after many agent terminals were recreated and resized. Windows Error Reporting showed the first Wardian failure as heap corruption (`0xc0000374`) in `ntdll.dll`; the visible illegal-instruction dialog (`0xc000001d`) mapped to a deliberate fail-fast `ud2` trap after `int 29h`. The Wardian debug log around the crash showed many restored terminal sessions issuing `resize_agent_terminal` calls while OpenCode and other PTY children were active.

Wardian uses the vendored `portable-pty` Windows ConPTY implementation. Each `ConPtyMasterPty` serializes calls for one PTY handle, but Wardian previously spawned independent blocking resize tasks for each terminal session. That allowed multiple `ResizePseudoConsole` calls to enter the loaded ConPTY implementation at the same time across different PTY handles. Concurrent same-session resize requests could also race past the `pty_sizes` dedup check before the first resize recorded the authoritative size.

## Decision

Resize coordination stays in the backend. Frontend fitting and ResizeObserver behavior remain unchanged so drag-resize responsiveness is not degraded.

Wardian now maintains:

- A per-session async resize lock. Each `resize_pty` call rechecks `pty_sizes` after waiting for an in-flight resize, so duplicate same-size requests are skipped instead of reaching the PTY master.
- A process-wide native PTY resize lock. Agent PTY and standalone user-terminal resizes both take this lock only around the blocking `MasterPty::resize` call, preventing parallel native ConPTY resize calls across sessions.

Distinct size reports are still applied in order. The frontend can continue emitting responsive size updates, while the backend removes stale duplicate work and native ConPTY reentrancy.

## Consequences

- **Positive:** Removes the crash-prone pattern of parallel `ResizePseudoConsole` calls across restored terminal sessions.
- **Positive:** Preserves smooth frontend resize behavior because client-side fit scheduling is unchanged and distinct backend size reports are not debounced away.
- **Positive:** Reduces redundant ConPTY redraw churn by rechecking duplicate same-session sizes after an in-flight resize finishes.
- **Negative:** Native PTY resize calls are process-wide serialized. This is intentional because the native operation is short and the frontend already coalesces high-frequency ResizeObserver fits.

## Verification

Focused regression coverage:

- `concurrent_duplicate_resize_rechecks_recorded_size_after_in_flight_resize`
- `concurrent_resizes_across_sessions_do_not_overlap_native_resize_calls`

Both tests use a probe `MasterPty` and do not require real provider processes.
