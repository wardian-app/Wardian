# Exact continuation after workspace assignment

Workspace reassignment preserves a provider conversation rather than implicitly
starting a fresh conversation. This requires a durable identity-bound contract:
an exact provider UUID alone is insufficient if missing-history recovery can
start a new conversation with that same UUID.

The agent roster's `AgentConfig.exact_resume` records the required provider and
conversation identity. Assignment sets it on the staged configuration, including
Off assignments, and publication persists it with the new workspace. Old roster
files omit it and retain ordinary startup recovery semantics. Native session
specifications already carry the roster configuration, so delayed dispatch and
restore retain the requirement without a worktree-specific launch path.

Read-only `AgentProvider.validate_exact_resume` preflight runs before existing
runtime disposal, target reservation or shutdown holds. Each qualified adapter
must use this seam to reject unsupported options and unavailable materialized
history. It repeats the check at native launch before crossing the provider
boundary. Adapter-owned verified file bindings are revalidated before the child
can write. Static adapter qualification remains independent of actual runtime
shutdown authority.

An exact requirement rejects provider or conversation substitution. Claude and
Pi native missing-history reconciliation returns an error instead of changing
`resume_session` into a fresh identity. Ordinary unmarked first-initialization
recovery remains unchanged. Explicit New Session and configured Fresh resume
clear the prior requirement as part of the intentional conversation boundary;
workspace assignment still forces continuation while preserving the configured
policy for subsequent ordinary resumes.

The roster is the canonical owner of this agent-scoped requirement; native
session specs are runtime views. The inspectable saved agent configuration carries
it across process restarts. This record is a continuity contract, not a credential
or authority to control another process. No provider is newly qualified by storing
it, and adapters remain disabled until actual identity, destination execution and
retained-history qualification passes.

Ordinary Resume has a separate read-only `validate_resume_launch` seam before
owner disposal. Its default enforces any durable exact requirement. Adapters
that require a stricter launcher for every materialized Resume must validate
unmarked existing agents here as well. Preparation stages configuration without
publishing it, including PendingPi recovery; configured Fresh policy cannot skip
validation when a pending identity has acquired complete history. Genuinely
pending initialization retains the provider's fresh launch route.

Native-broker exact continuation has its own transport preflight. Interactive
qualification cannot imply RPC parity. Pi's stock RPC launch retains the saved
header cwd, so exact-required RPC fails before provider startup until a supported
destination override exists. Ordinary unmarked RPC recovery retains its existing
behavior; the SDK interactive bridge consumes the assigned destination.

## Retained source evidence

The shared lifecycle stages a typed source receipt before stopping the previous
writer. It carries that receipt through archive drain, roster persistence,
replacement publication, delayed Resume, and native dispatch. Each adapter
revalidates the retained receipt after drain and immediately before launch;
failure cannot substitute a newly discovered source for an existing receipt.

Claude and Pi retain append-only transcript identity and a confirmed byte-prefix
digest. Claude records its configuration selector. An explicit selector pins the
child to the canonical captured root; an absent selector preserves the native
default configuration lookup rather than setting a transcript directory as an
override. Rotating a workspace projection cannot select another transcript.
Watchers and telemetry consume the same captured source.
Antigravity requires independently owned, stable projection paths matching all
persisted provider workspace fields. A receipt cannot grant projection ownership.
OpenCode records its actual materialized store and immutable completed history
rows; unfinished provider work can append without changing confirmed records.

Physical file identities are serialized as decimal strings to preserve their
full width across Rust and TypeScript. Legacy identity-only records can acquire
source evidence during preparation, but launch revalidation requires retained
evidence once captured. Explicit New Session removes the old continuation intent.
Adapter support and focused tests do not imply native provider qualification.
Only observed same-identity, retained-context, actual destination-tool execution
can open an active handover capability. Unsupported native transports fail before
launch; Pi stock RPC remains separate from the interactive SDK route.
