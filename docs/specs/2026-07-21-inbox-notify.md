# Inbox and Notify Control Plane

## Decision

Rename the user-facing Queue surface to **Inbox**. Keep **Mailbox** for agent-directed delivery, so the two terms describe different directions of communication. Existing persisted `queue` workbench surfaces migrate to `inbox` when loaded; legacy completion files retain their internal compatibility names for this slice.

Inbox combines four sources: legacy completion projections, provider action-needed evidence, durable agent `notify` records, and native workflow Approval-node projections. It does not treat arbitrary terminal output as a final completion summary. When Wardian lacks a canonical final result, it shows `Work finished — no summary supplied`.

## Notify

`wardian notify update` creates a durable, concise user-facing update. It is for material results, limitations, and changes to the user's next decision, not routine progress.

`wardian notify approval` creates a durable manual approval request. It must include a title, body, proposed action, risk, two to five explicit choices, and an expiry. It requires a live managed-agent origin, allows one unresolved request per agent, and is limited to irreversible, external, security-sensitive, materially costly actions, or an approval explicitly requested by the user. Expiry and client timeout mean **do not proceed**.

Manual approval resolution is recorded as a child interaction reply and returned by `notify --wait`; it never impersonates a user as an agent and never sends arbitrary approval text into a provider terminal.

## Workflows

Workflow Approval nodes are already native engine gates. Inbox reads their durable awaiting state and calls the existing workflow approve/reject transition directly. The workflow engine remains the source of truth; workflow approval is not re-encoded as an agent notification.

## Verification

- Workbench persistence tests cover legacy `queue` surface migration.
- Interaction-state and control tests cover durable notification creation, one-open-approval policy, resolution, and expiry.
- Inbox view/store tests cover update and approval rendering plus workflow Approval-node projection.
