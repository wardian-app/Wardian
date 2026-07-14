# Entity-Oriented Agent Semantics

- **Status:** Proposed
- **Date:** 2026-07-14

## Sources

- [The Fabric and the Brain](https://protocolized.summerofprotocols.com/p/the-fabric-and-the-brain)
  motivates the idea that an agent's legible behavior emerges through its
  protocols and relationships, rather than through a cosmetic persona.
- [Malleable Software](https://www.inkandswitch.com/essay/malleable-software/)
  motivates a gentle slope from use to creation, composable tools over shared
  data, and the ability for local groups to evolve their environment.

## Context

An agent class or a system prompt can make different model sessions sound
different without making them meaningfully different operational entities. The
same provider session can be prompted to be a coder, reviewer, or researcher,
while retaining the same effective powers, boundaries, and failure modes.

Wardian already gives agents durable identity fields, relationship topology,
workflow positions, and an accumulating record of interaction and runtime
evidence. This spec names the model those surfaces are moving toward and sets
limits on what Wardian may claim about it.

## Decision

Wardian should treat a mature agent as a durable, situated operational entity.
An entity is not defined by a personality prompt. It is defined by its stable
identity, its position among other entities, the effects it is actually able to
cause, the policies that govern its participation in Wardian-controlled flows,
and the evidence accumulated across its sessions.

The five aspects are:

1. **Identity** — a stable entity identity, separate from provider session IDs,
   terminals, and individual workflow runs. Identity makes an agent's history
   attributable across changing sessions and providers.
2. **Relationships** — durable, inspectable links to workspaces, teams,
   watchlists, agent topology, workflow roles, handoff paths, and approval
   relationships. An entity is situated; it is not an isolated object with a
   decorative label.
3. **History** — attributable runtime evidence: conversation logs, interactions,
   lifecycle transitions, tool and approval observations, workflow events,
   artifacts, and resource telemetry. Logs are raw source material, not memory
   or an entity definition. Curated memory remains evidence-backed and is
   retrieved selectively.
4. **Authority** — the effects an entity is permitted to cause. Authority is
   meaningful only at the boundary that can enforce it. It includes both
   Wardian-owned effects, such as workflow transitions and message routing, and
   execution effects, such as filesystem or shell access, when an identified
   provider or host runtime enforces them.
5. **Disposition** — stable operating behavior within an entity's available
   choices: how it enters work, what evidence it requires before completion,
   when it hands work off or pauses, and how it responds to failure. Disposition
   is primarily runtime and workflow policy, not a tone-of-voice instruction.

Together these aspects make an entity legible enough to compose, observe, and
revise over time. They do not imply that a model is explainable, sentient, or
reliably obedient to prose instructions.

## Truthful Enforcement Boundary

Wardian must not present an advisory instruction as authority. Every control
shown by the product must identify its enforcement boundary:

| Boundary | Meaning |
| --- | --- |
| Wardian-enforced | Wardian controls the relevant action, such as a workflow gate, routing decision, lifecycle action, or structured completion. |
| Provider-enforced | A provider CLI or API applies the setting; the available semantics vary by provider and version. |
| Host-enforced | The operating system or an explicit isolation runtime constrains the process or its resources. |
| Observed | Wardian can detect or report the behavior, but cannot prevent it. |
| Advisory | Prompt or class guidance that may influence behavior but provides no enforcement guarantee. |

In particular, owning a PTY and parsing provider events does not make Wardian an
inline tool-call authority. A raw shell or provider-owned tool channel can
produce broad effects outside Wardian's control. Worktree assignment is useful
Git isolation, but is not by itself filesystem security isolation.

## Scope and Canonical Records

This decision does not introduce a new entity database or replace existing
records. Current agent configuration, provider sessions, workflow runs,
interaction records, queue evidence, and provider telemetry keep their current
owners. The entity is initially a conceptual and queryable projection across
those records.

Human-authored definitions should remain small and inspectable on disk. They
may describe class intent, Wardian-owned workflow policies, and references to
the evidence that supports them. High-volume telemetry, conversations, and
event history must remain structured runtime evidence rather than being copied
into Markdown or automatically replayed into model context.

## Malleability Interface

Entity-oriented semantics do not introduce a monolithic agent-definition form.
They give a common address to the existing artifacts through which a user can
adapt a Habitat. A Library asset, workflow, Garden surface, or evidence view
must name the entity or entities it concerns, the scope it changes or observes,
and its canonical owner.

| Artifact | Tailoring effect | Canonical owner |
| --- | --- | --- |
| Prompt | Reusable invocation; does not alter the entity. | Library prompt file |
| Skill | Reusable procedure or context, deployable at a chosen scope. | Library skill and deployment record |
| Class | Reusable blueprint of instructions, defaults, and deployed skills. | Class definition and Library deployment record |
| Workflow | Typed relationship and process semantics among entities. | Workflow blueprint and run record |
| Memory and evidence | Attributable knowledge and observations, promoted selectively. | Evidence and memory stores |
| Garden, Dashboard, Queue, and Graph | Lenses over entities and their relationships. | Their respective layout or projection records |

The tailoring slope has many small moves, not one jump from settings to
programming. This is the ordered frame for Wardian's user-facing documentation:
introduce the next action only after its preceding, simpler forms of adaptation
are understandable.

| Rung | Tailoring action | Wardian form |
| --- | --- | --- |
| 1 | Use an agent | Send a direct instruction; inspect its live session. |
| 2 | Observe and understand | Read transcript, Queue evidence, logs, Graph, resource telemetry. |
| 3 | Frame attention | Create a watchlist/Cohort, saved Site, dashboard lens, or Garden scene. |
| 4 | Reuse a known move | Run a starred Library prompt on selected agents. |
| 5 | Save a local variation | Turn that one-off instruction into a prompt. |
| 6 | Give a procedure a name | Turn a repeated method into a skill, with a concise contract and artifacts. |
| 7 | Change its scope | Deploy the skill to one agent, a class, or global context. |
| 8 | Assemble a reusable agent shape | Edit or create a class; choose its instructions, deployed skills, and provider defaults. |
| 9 | Compose entities | Bind roles, teams, workspaces, skills, and evidence into a workflow or project habitat. |
| 10 | Specify interaction behavior | Add typed handoffs, approval gates, retries, evidence requirements, and escalation points. |
| 11 | Promote outcomes | Curate proven evidence into memory, a prompt, skill, workflow, or project context. |
| 12 | Adapt a tool or lens | Create a small entity-oriented dashboard, report, Garden view, or extension over shared canonical data. |
| 13 | Share, vary, and maintain | Publish or deploy a Library asset, make a local variant, compare it, and selectively adopt changes. |
| 14 | Author new behavior | Build a plugin, surface, provider adapter, or eventually managed runtime feature. |

Every rung should use the least expressive mechanism that captures the desired
change. A prompt is preferable to a skill when a reusable invocation is enough;
a skill is preferable to a workflow when no multi-entity protocol is needed;
and a registered extension is a last resort rather than the next step after a
setting. This keeps the path from operator to habitat-maker gradual and keeps
customizations reviewable.

Garden-local notes, spatial proximity, and planning edges remain interpretive.
They become operational relationships only through an explicit canonical action
that creates or changes a workflow binding, deployment, team/workstream record,
or another owning record.

## Operationalization

The least invasive, most generalizable path is to improve Wardian's truthful
description and control of boundaries it already owns, before attempting to
mediate arbitrary provider tool calls.

### 1. Establish a provider capability and enforcement matrix

Define an adapter-owned capability matrix for each provider and launch mode.
It should say what Wardian can configure, observe, or enforce for that provider,
including sandbox settings, approval behavior, tool restrictions, provider
hooks, structured tool-event visibility, workspace/worktree binding, and
headless versus interactive execution.

The matrix must record both the semantic feature and its enforcement boundary.
It is not a synthetic cross-provider permission model: unavailable or
non-equivalent behavior remains unavailable or non-equivalent in the UI and
API. Existing provider-specific settings should be projected through this
matrix, retaining their provenance rather than being relabeled as universal
Wardian permissions.

### 2. Make Wardian-owned disposition executable

Use the workflow engine, structured asks/replies, Queue, and routing surfaces
for policies Wardian can actually enforce. Examples include:

- required evidence before a node or structured request is complete;
- eligible approvers and separation between producer and approver;
- explicit handoff recipients and completion formats;
- role assignment, lifecycle ownership, cancellation, and retry behavior;
- pause or escalation when a known workflow condition is met.

These are substantive differences between entities even when their providers
have identical tool access. They should be represented as typed workflow and
interaction semantics, not appended to class prompts.

### 3. Build observation before scoring

Project attributable evidence into per-entity, time-bounded views: work
accepted and completed, queue wait, handoffs, approval outcomes, retries,
failures, resource use, and links to representative runs. These views must show
their time range, sample size, and evidence drill-downs. Wardian should not
derive a single opaque quality or trust score.

This extends the existing Queue, Graph, logs, and telemetry surfaces instead of
duplicating them. The Dashboard becomes a longer-term assessment lens; Queue
remains a work and evidence surface, and Graph remains a relationship surface.

### 4. Defer execution authority until there is an enforcement point

Cross-provider execution authority requires one of two explicit architectures:

1. a managed execution mode in which providers submit structured tool requests
   through a Wardian-controlled broker; or
2. an isolation backend that constrains the process at the host boundary.

Both are significant projects. The former requires provider/API support for
structured interception or a different managed agent runtime. The latter
requires platform adapters because Windows, macOS, and Linux do not expose one
portable sandbox primitive with identical semantics. Terminal-output parsing,
PATH wrappers, or a generic shell allowlist are not adequate substitutes for an
effect boundary and must not be presented as security enforcement.

Until one of these architectures exists, execution settings remain
provider-enforced, host-enforced, observed, or advisory according to the
capability matrix.

## Consequences

- Agent class remains valuable for reusable context and intent, but class text
  alone does not define an operational entity.
- Wardian can make meaningful entity differences real now through its own
  orchestration semantics and evidence model.
- Provider parity is not a product requirement. Honest capability differences
  are preferable to a uniform UI that overstates control.
- A future managed execution mode can add stronger authority without changing
  the conceptual model, because the enforcement boundary is already explicit.
- The Library is an in-place authoring surface for this slope: it lets a user
  promote a discovered adaptation without leaving the Habitat for a separate
  plugin project.
- Markdown-as-truth remains focused on compact, reviewable human decisions;
  runtime evidence remains bounded, attributable, and queryable rather than
  becoming prompt bloat.

## Non-Goals

- Adding a personality field or a long-lived persona prompt.
- Treating conversation transcripts as long-term memory by default.
- Creating a universal provider-neutral tool permission abstraction before it
  has a real enforcement mechanism.
- Adding a new workflow language solely to express entity semantics.
- Claiming that worktree assignment, terminal ownership, or event parsing alone
  provides execution sandboxing.
