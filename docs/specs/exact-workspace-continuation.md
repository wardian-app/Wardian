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
