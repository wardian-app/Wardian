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
