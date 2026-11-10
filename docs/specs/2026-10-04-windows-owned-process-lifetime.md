# Windows owned process lifetime

## Failure and scope

The app supervisor permits explicit job breakaway to support updater handoff.
An ordinary owned root that relies only on that job can therefore launch a tool
outside app lifetime containment. Codex owner and proxy jobs previously attached
after spawn, leaving a window for a provider to create descendants first.
Other native and headless roots relied on outer containment alone.

This is a reproducible launch-policy counterexample. It does not establish the
cause of previously observed orphan processes or the workspace-save freeze.
Listener socket inheritance in issue #1473 is a separate defect.

## Ownership contract

Preserve the outer app job's updater exception. Give every new provider and
user-terminal PTY a dedicated non-breakaway job through the existing suspended
ConPTY launch path. Require outer app supervision before preparing that launch.

Native broker transports, Codex owners and proxies, headless provider runs and
managed browser engines use an explicit owned-command launcher. It preserves
Tokio arguments, environment, cwd, pipes and cancellation settings. On Windows
it creates the root suspended without breakaway, verifies exact outer-job
membership through the retained child handle, assigns the app-owned descendant
job and any existing caller stop job, then resumes the unique initial thread.
Non-Windows launches delegate to the existing Tokio spawn.

The app-owned job is anonymous, non-inheritable, non-breakaway and retained for
the app lifetime. It does not grant a per-agent quiescence receipt. Existing
per-owner stop jobs retain their own ownership and cleanup semantics. Updater
handoff, Explorer and user-opened external applications do not use this launcher.

The outer job covers abrupt app death between process creation and inner
assignment. A failed outer supervisor prevents managed spawn. Assignment,
thread lookup or resume failure terminates and joins only the retained child
handle. No historical PID, parent-PID sweep or unrelated process is a cleanup
authority.

## Thread identity

Stable Rust and Tokio do not expose the initial thread handle. A Tool Help
snapshot locates candidates for the still-suspended root. Open each candidate
thread, obtain its owner, and compare that owner's process kernel object with
the retained spawn handle using `CompareObjectHandles`. Require exactly one
matching thread. Resume it once and require an initial suspend count of one.
Snapshot IDs select candidates; the retained handles establish authority.

## Evidence and limits

Windows tests abruptly terminate isolated parents and verify retained child
and descendant handles reach the signaled state while a foreign sentinel stays
alive. They cover normal nested-job tool children, rejected descendant breakaway,
the pre-assignment crash window, rejected runtime assignment without execution,
literal arguments, cwd, environment, async stdio and harmless updater escape.
A fake protocol provider also exercises the production native broker bootstrap.

These fixtures exercise OS ownership without credentials or a live desktop.
They do not prove every real provider's sandbox compatibility. A provider that
requires explicit escape from all ancestor jobs must be assessed before this
containment policy is considered ready for that provider.
