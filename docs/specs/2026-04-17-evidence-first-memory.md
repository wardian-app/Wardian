# Evidence-First Memory

- **Status:** Placeholder
- **Date:** 2026-04-17

## Context and Problem Statement

Wardian needs a memory model that avoids using provider-native session replay as long-term memory. Provider sessions are useful runtime continuity, but they are expensive, provider-specific, and difficult to inspect. Wardian's memory system should instead preserve evidence, index it for retrieval, and build prompt context selectively.

This placeholder records one immediate design constraint from the session-persistence work. A fuller memory architecture will replace this document later.

## Proposed Decision

### Continuity Is Not Memory

Provider-native session IDs, PTY state, provider homes, approval hooks, and provider logs are runtime continuity. They should not be the primary memory substrate.

Wardian memory should be based on:

- raw evidence artifacts
- workflow run outputs
- transcripts or excerpts with provenance
- promoted knowledge records
- scoped retrieval into prompts

### Scoped Memory For Inherited Workflow Runs

Workflow mode affects memory access:

| Workflow run mode | Read scope | Default write target |
| --- | --- | --- |
| `ephemeral` | common and class scopes | workflow run artifacts |
| `inherit_fresh` | common, class, and source-agent scopes | workflow run artifacts |
| `inherit_resume` | source agent runtime and provider session | source agent continuity, plus workflow artifacts |

`inherit_fresh` is the important case. It may read source-agent scoped memories so the run behaves like the selected agent's profile, but it must not automatically write back to that source agent's durable memory.

Default write behavior:

```text
inherit_fresh during run:
  read source agent scoped memory
  write workflow run artifacts
  do not mutate source agent memory

after run:
  preserve workflow output/evidence
  discard or archive provider session state as non-resumable
  require explicit promotion to source-agent memory
```

### Promotion Must Be Explicit

Future memory features should support explicit promotion paths:

- promote workflow artifact to source-agent memory
- attach evidence to an agent
- append a provenance-backed memory record
- propose memory updates for user review

Automatic source-agent memory writes from `inherit_fresh` should be avoided in the initial implementation. If added later, they should use a backend memory API with locking, append-only records, provenance, and conflict handling rather than allowing providers to write shared memory files directly.

### Runtime Directories Are Not Durable Memory

A workflow-run runtime directory may be retained after completion for auditability, but it is an artifact container, not a resumable agent home. Fresh workflow runs should not write provider-discovered session IDs back to the source agent's `resume_session`.

## Consequences

- **Positive**: Workflow runs can use an agent's scoped context without silently changing that agent.
- **Positive**: Memory becomes inspectable and provenance-backed instead of hidden inside provider sessions.
- **Positive**: Token growth is controlled because prompt context is selected, not replayed wholesale.
- **Negative**: Users must explicitly promote useful workflow outputs into durable agent memory.
- **Negative**: The future memory system needs a real API for promotion, locking, provenance, and retrieval.
