# Windows Agent Process and Control Reliability

## Status

Accepted

## Context

Wardian launches interactive providers through PTYs and supervises their process trees from the Rust backend. On Windows, some provider CLIs start through `.cmd` shims. During clear, resume, or worktree reassignment, the shim process can exit before Wardian runs `taskkill /PID <pid> /T /F`. A failed `taskkill` for an already-exited wrapper must not be logged as a cleanup failure, and Wardian must still look for session-marked descendants that may have outlived the wrapper.

Separately, the CLI used the same `app_not_running` error for connection failures and control endpoint timeouts. Under heavy agent load, reads can still succeed while mutating control requests time out. Reporting that state as "app not running" hides the real overloaded-control condition.

Slow telemetry passes contributed to the overload risk on Windows because each active agent triggered another full process scan to rediscover session-marked process roots. With many active agents, that made process sampling scale with agent count instead of with one process table pass.

## Decision

Windows cleanup treats an already-gone PID as a successful cleanup outcome. `taskkill` failures are classified with a post-failure process-existence check, so empty output from a vanished wrapper does not produce a noisy failure. Stale-session cleanup also performs a bounded follow-up scan for the same `WARDIAN_SESSION_ID`, giving newly visible descendants a chance to be reaped after the wrapper disappears.

Telemetry now discovers Windows session process roots from the process marker data already collected during the current process-table pass. The metrics pass and app metrics calculation reuse those roots instead of calling a fresh process scan once per agent.

CLI control endpoint timeouts now return `control_endpoint_timeout` with a retry/load hint and a distinct exit code. Missing or refused endpoints still return `app_not_running`.

## Consequences

- Normal Windows cleanup remains explicit and fast, while stale `.cmd` wrapper races no longer look like failed cleanup.
- Orphaned provider descendants with `WARDIAN_SESSION_ID` markers are more likely to be reaped during clear, resume, and worktree reassignment.
- Telemetry process sampling scales better with large rosters because Windows session-root discovery is single-pass per metrics tick.
- Automation can distinguish an absent Wardian app from a live but overloaded control endpoint.

Native runtime E2E remains the required layer for proving real ConPTY behavior. Unit tests cover error classification, stale-PID classification, and single-pass session-root discovery.
