# Agent memory

Wardian agents can save durable preferences, decisions, corrections, lessons,
and useful active project checkpoints. Memory is available when the agent starts a new
provider process, even when the provider changes.

Direct memory is disabled by default while the feature matures. Enable **Agent
memory** under **Settings > Agent Runtime** to opt in. It does not consume a
second model call.

When disabled, new provider processes receive no startup recall, direct-retention
instructions, or managed memory capability. Existing records remain stored and
available for inspection, and changing the setting applies to newly launched or
restarted provider processes.

The optional Memory Consolidation automation is disabled until you configure and
run it or bind it to a session-close invoker.

You do not need to say "remember this." At the end of an ordinary task, an agent
checks for clear preferences, project conventions, decisions, corrections,
lessons, and active checkpoints that materially help resume work. It saves a
small number of useful records when the evidence is clear. Routine progress
reports and task journals belong in task or conversation records. Brief or explicitly
one-response-only instructions are not saved merely because they appeared in a
conversation. The check happens before the agent's final answer; it does not
require loading an optional consolidation automation or making another model
call.

## Save and inspect memory

```bash
wardian memory save "Prefer concise technical handoffs" \
  --evidence "The user explicitly requested concise handoffs." \
  --scope agent

wardian memory save "Release validation is awaiting a real-macOS run" \
  --evidence "The latest review left native macOS acceptance pending." \
  --kind current

wardian memory list
wardian memory recall
```

PowerShell:

```powershell
wardian memory save "Prefer concise technical handoffs" `
  --evidence "The user explicitly requested concise handoffs." `
  --scope agent
```

The default kind is Stable. Use Current (`--kind current`) for an active
checkpoint worth carrying to the next session. Update it when state changes and
remove it when the work resolves. Workspace scope is the default. Use agent
scope only for a preference or working convention that should follow the agent
between projects. Every save requires an evidence excerpt. Optional `--source`
values preserve a link for deeper inspection without coupling retention.

Use `show`, `history`, `update`, and `remove` with the returned memory ID. The
full ID and any unique prefix are accepted, including the eight-character IDs
shown in startup-injected memory instructions. If a prefix matches more than
one memory, use a longer prefix. Update and remove preserve audit history.

Inside a Wardian-managed terminal, memory commands are restricted to that
agent's own records and authenticated with a launch-scoped capability. Run them
only from that managed context. The desktop currently exposes cross-agent memory
inspection; managed terminals cannot use it to change another agent's records.
An uncredentialed operator shell fails closed.

The capability expires when Wardian terminates, replaces, or reclaims that
provider runtime. A PTY reader or broker error alone does not revoke a still-live
runtime's capability. Concurrent interactive and automation processes receive
independent capabilities.

## What appears in chat

Successful actions appear as `Memory saved · This agent`, `Memory updated · This
agent`, or `Memory removed · This agent`. A provider process that receives memory
shows a collapsed `Memory loaded` row. Expand it to inspect the exact context.
No row appears when there was nothing to load.

## Operator maintenance in Garden

When memory records accumulate outdated checkpoints, misclassified kinds, or
redundant historical notes, an operator can review and apply batch corrections
through the Garden desktop interface.

Managed agents may draft a local JSON plan file, but only a human desktop
operator can preview, confirm, and apply maintenance. Plans are reviewable
proposals rather than authority. Managed CLI tools cannot modify memory across
agents, and the desktop backend enforces native confirmation even if a caller
bypasses the frontend.

### Opening the maintenance surface

1. Navigate to **Garden** in the desktop application.
2. Select an agent to open its interior panel.
3. In the **Memory** region, click **Maintain memory…**.

The dialog binds to that exact agent owner (`<agent-name> · owner <agent-id>`).
Plans targeting any other agent are rejected on import.

### Plan structure and constraints

Maintenance plans use schema version 1 and contain 1 to 100 operations.
The imported file must be at most 1 MiB.

Key constraints:
- `plan_id`, `agent_id`, and `idempotency_key` are required strings.
- Text and evidence excerpts are capped at 8,192 characters each.
- Each operation accepts at most 64 sources, and each source locator is capped at 4,096 characters.
- Scopes must use explicit tagged objects: `{ "kind": "agent" }` or `{ "kind": "workspace", "path": "<absolute-workspace-path>" }`. Relative workspace paths are rejected by platform-aware validation.
- An existing memory ID may appear in at most one source operation, though a `revise` target may also absorb records via `retire_into`.

Supported operations:
- `revise`: Updates text, kind (`stable` or `current`), scope, evidence excerpt, or additional sources for an existing `memory_id` matching an `expected_revision_id`. Existing sources are preserved and combined with normalized additions.
- `create`: Allocates a new record using a plan-local `client_key`. This allows splitting mixed records without generating synthetic IDs beforehand.
- `retire`: Marks a record as removed (`status = "removed"`) using its `memory_id`, `expected_revision_id`, and an explicit `reason`. Preserves revision history and evidence for auditing.
- `retire_into`: Retires a record into another surviving `target_memory_id` or `target_client_key`, transferring and deduplicating source locators into the recipient.

### Example plan file

Save the plan as a `.json` file, such as `maintenance-plan.json`:

```json
{
  "schema_version": 1,
  "plan_id": "plan-2026-09-24-cleanup",
  "agent_id": "<agent-id>",
  "idempotency_key": "cleanup-batch-001",
  "operations": [
    {
      "op": "revise",
      "memory_id": "<memory-uuid>",
      "expected_revision_id": "<revision-uuid>",
      "text": "Production build requires Rust 1.80+ and Node 22.",
      "kind": "stable",
      "scope": {
        "kind": "workspace",
        "path": "<absolute-workspace-path>"
      },
      "evidence_excerpt": "Verified in repository toolchain manifest.",
      "add_sources": [
        {
          "source_type": "artifact",
          "locator": "rust-toolchain.toml",
          "primary": true
        }
      ]
    },
    {
      "op": "create",
      "client_key": "new-test-rule",
      "text": "Run native E2E tests with npm run test:e2e:native:fast.",
      "kind": "current",
      "scope": {
        "kind": "agent"
      },
      "evidence_excerpt": "Updated in test running instructions.",
      "sources": []
    },
    {
      "op": "retire",
      "memory_id": "<stale-memory-uuid>",
      "expected_revision_id": "<stale-revision-uuid>",
      "reason": "Superseded by automated nightly validation."
    }
  ]
}
```

Preparing a plan via shell:

POSIX:

```bash
cat << 'EOF' > maintenance-plan.json
{
  "schema_version": 1,
  "plan_id": "plan-cleanup-01",
  "agent_id": "<agent-id>",
  "idempotency_key": "cleanup-01",
  "operations": [
    {
      "op": "retire",
      "memory_id": "<memory-uuid>",
      "expected_revision_id": "<revision-uuid>",
      "reason": "Outdated test instruction."
    }
  ]
}
EOF
```

PowerShell:

```powershell
@'
{
  "schema_version": 1,
  "plan_id": "plan-cleanup-01",
  "agent_id": "<agent-id>",
  "idempotency_key": "cleanup-01",
  "operations": [
    {
      "op": "retire",
      "memory_id": "<memory-uuid>",
      "expected_revision_id": "<revision-uuid>",
      "reason": "Outdated test instruction."
    }
  ]
}
'@ | Set-Content -Path maintenance-plan.json -Encoding utf8
```

### Import and preview

Click the file selector in the maintenance modal to import `maintenance-plan.json`.
Duplicate `kind` or `path` keys in a scope are rejected as malformed before preview.

Wardian validates the schema and executes a read-only preview against `memory.db`:
- Computes a canonical `preview_digest` binding the plan ID, owner, operations, and expected revisions.
- Displays operation counts and detailed Before / After summaries for every entry.
- Renders source additions and absorbed memory IDs. Retirements are distinguished from active consolidations.

### Handling revision conflicts

If an active record was edited, superseded, or retired after the plan was generated, the preview highlights a conflict:
- **`revision_changed`**: The record's current revision ID differs from `expected_revision_id`.
- **`target_missing`** or **`already_retired`**: Referenced memories are not present or have already been removed.

When one or more conflicts are detected:
- The **Apply reviewed plan…** button is disabled.
- The operator must inspect the conflict details, adjust the plan file to match the latest revisions shown in Garden's memory and history view, and re-import or re-preview.

### Native confirmation and atomic apply

When no conflicts exist, click **Apply reviewed plan…**.

1. **Native confirmation dialog**: The host OS displays a native modal confirmation detailing the owner ID, operation count, and preview digest prefix (`sha256:` plus 12 hexadecimal characters). If declined or dismissed, the operation aborts with zero changes.
2. **Atomic transaction**: The backend opens an immediate SQLite transaction, re-verifying the preview digest and expected revisions. If any concurrent modification occurred, the transaction rolls back cleanly.
3. **Receipt display**: Upon success, a `MaintenanceReceipt` is stored and rendered in the modal. The receipt records the applied timestamp, plan ID, owner ID, idempotency key, preview digest, and allocated IDs for newly created or revised records.

### Idempotency and failure recovery

- **Replay**: Re-applying a plan with an identical `idempotency_key` and contents returns the existing receipt without duplicate writes or additional native prompts.
- **Uncertain response**: If an application crash or desktop IPC interruption occurs while awaiting the apply response, reopen the dialog, import the plan, and click **Check apply receipt**. The backend queries the stored receipts by owner and idempotency key to confirm whether the batch was committed.
- **Recovery after conflict**: Any failed apply clears the cached preview, requiring a fresh preview before another apply can be attempted.

## Optional consolidation

The Library includes the editable `Memory Consolidation` automation sample. Assign
its `curator` role to a provider or agent. A temporary-provider assignment may
include `model` and `effort`; Wardian uses that exact selection and the user's
provider quota without a hidden fallback.

Create a session-close invoker disabled, inspect it, then enable it:

```bash
wardian automation session-close add \
  --blueprint memory-consolidation \
  --name "Consolidate this agent" \
  --agent <agent-name-or-id> \
  --boundary clear \
  --require-archive \
  --assignments '{"curator":{"target_type":"temporary_provider","provider":"codex","model":"<model-id>","effort":"low"}}'

wardian automation session-close list
wardian automation session-close enable <invoker-id>
```

Use model IDs and effort levels returned by
`wardian agent models --provider <provider> --refresh`. Conversation logging must be enabled for automatic
archive consolidation. Direct saving and startup recall remain available when
logging or consolidation is disabled.
