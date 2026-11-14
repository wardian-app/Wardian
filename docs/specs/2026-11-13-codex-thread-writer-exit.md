# Codex thread writer exit

## Decision

A terminal process tree and its thread writer are separate ownership domains.
Codex's ordinary daemon discovery remains enabled. Terminal job exit alone
therefore cannot release an old provider session's hold.

The retained stop fence owns the exact native generation gate and shared owner
through terminal-tree shutdown. After the contained terminal exits, it requires
supported unload evidence from the captured private app-server. A complete empty
`thread/loaded/list` response qualifies; if the expected thread remains loaded,
the worker may issue `thread/unsubscribe` once and await an empty response set
within a bounded deadline. Foreign identities, pagination, missing protocol
support, and uncertain responses retain the hold.

The owner must then pass the same whole-tree join used by contained PTY
runtimes: seal creation, retain member handles, terminate the job, and observe
the direct child, every retained member, and an empty job. The existing Windows
suspended launch establishes this containment before the owner's first
instruction. Other platforms retain holds until equivalent ownership evidence
exists.

Only this stop path constructs `CodexThreadExit`, with private agent,
generation, and thread fields. A compile-fail test rejects construction by
callers. Release validates that receipt against the complete held acquisition
under both lease locks. The current lifecycle transition may keep its
reservation; another active writer or a changed acquisition prevents release.

## Cancellation and recovery

Runtime capture transfers cleanup to a retained worker before the caller's next
await. Cancellation does not discard the native owner or its creation gate.
Unverified tree exit keeps the fence and handles available for explicit retry.
A lease write failure retains an already verified receipt without replaying
unsubscribe. Cleanup that succeeds without writer evidence releases the runtime
fence and permits a fresh session, while the old session remains held.

Unknown owners, global-daemon writers, and legacy holds remain conservative.
Startup logs and the saved-session Inbox diagnostic explain the retained hold;
the existing acquisition-specific operator recovery path remains available.
Neither a restart nor a reboot excludes a new writer reopening the old thread.

## Evidence

Controlled Windows children and supported app-server response fixtures cover
owned-thread release, an out-of-job writer, missing unload evidence, failed
owner verification, cancellation, and persistence retry. These checks do not
substitute for opt-in real-provider acceptance.
