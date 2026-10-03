# Desktop Home Ownership

Status: accepted
Date: 2026-10-02
Scope: Interactive desktop processes sharing a Wardian home

## Decision

Each desktop process acquires an exclusive OS file lock at
`<wardian-home>/runtime/desktop-owner.lock` before migration, recovery, provider
restore, or server startup. It retains the open lock through the event loop and
shutdown. A second cooperating desktop exits with a diagnostic instead of
restoring competing providers or attempting to bind the same remote port.

The lock pathname is permanent. It is never deleted to recover a crash; OS
process termination releases the lock. Lock-file existence and persisted PIDs
are not ownership evidence. Distinct homes have distinct locks. CLI and
headless processes continue to use conversation leases rather than this
desktop-only lock.

## Compatibility and recovery

An older desktop that predates this protocol does not acquire the lock. Quit
legacy instances before starting a version using this protocol. The lock does
not terminate another process, steal its socket, or clear an uncertain provider
hold. Ordinary quit and update handoff must finish before a replacement starts.

Prior-provider holds need separate verified exit evidence. In particular,
joining a shell or requesting a tree kill does not prove every writer exited.
The desktop lock adds exclusion for cooperating app versions; it does not
replace provider containment or conversation acquisition checks.

## Verification

A subprocess regression holds one home's lock while another process attempts
startup, verifies that a different home remains independent, then deliberately
exits a child without running destructors and verifies reacquisition. This
tests OS exclusion and crash release without real provider or user processes.
