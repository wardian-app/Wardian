# Verified Windows Claude Process Stop

Status: accepted
Date: 2026-10-02
Scope: Newly launched interactive Claude PTY runtimes on Windows

## Decision

Create a dedicated non-breakaway, kill-on-close job before launching Claude.
The vendored PTY builder owns a duplicated job handle. Windows creates the PTY
child with its main thread suspended, assigns it to the job, then resumes it.
Assignment or resume failure terminates the suspended child and fails launch.
This closes the interval in which a running child could create descendants
before post-launch assignment. The existing app-lifetime supervisor remains.

Fresh resume and clear retain the exact owner of their newly created prior
provider hold. Before termination they set and read back a zero active-process
limit to reject new job members, enumerate the job, and retain each member's
process handle after checking membership. They stop the contained job and poll
those handles, the direct child, and job accounting without holding the roster
lock. Release that acquisition only after every captured handle signals exit,
the direct child exited, and the job has zero active processes. Job accounting
alone is insufficient: it can reach zero before process handles signal exit.
The ordinary termination and Drop paths use the contained job instead of
reconstructing a tree from a possibly reused PID.

## Failure and compatibility boundaries

Cancellation, a five-second join timeout, failed termination/query/wait, missing
child handles, or additional uncontained background processes produces no exit
receipt. Failed creation fencing, incomplete enumeration, or inability to open
and verify a member handle also retains the hold. This is a bounded receipt for
captured members; it does not enumerate historical process objects that had
already left the job before the snapshot. The old conversation remains fenced
when observation fails. Persistence failure is returned
to the lifecycle caller; the exact hold remains available for recovery.

Legacy/post-launch jobs, other providers, and other operating systems retain
the prior verified-repair policy. No existing hold is removed at startup or
solely because a PID is missing. A fresh conversation can still start under the
existing lifecycle exclusion while an uncertain old conversation stays fenced.

## Verification

Windows backend tests launch an isolated ConPTY shell and a real descendant,
verify job membership, stop through retained handles, and observe descendant
exit and immediate listener-port reuse while an unrelated process stays alive.
A sealed live job rejects a new suspended PTY member before execution, proving
the zero process limit is accepted and enforced. An invalid job handle prevents
the suspended child from executing its startup command. A lifecycle test
verifies release of only the exact hold acquisition and preservation of a
later acquisition and a hold with missing legacy runtime handles.

## Platform contract

Nested job membership blocks descendant breakaway when the immediate job does
not permit it, including when an outer job permits breakaway. See Microsoft's
[nested jobs documentation](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs).
