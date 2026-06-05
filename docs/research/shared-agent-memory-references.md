# Shared Agent Memory References

This document maps public memory, retrieval, and context-management systems to design patterns relevant to Wardian shared memory.

This is not an endorsement, affiliation claim, product evaluation, or implementation commitment. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-31.

Source basis: public repositories, first-party docs, first-party model pages, papers/preprints, and benchmark reports were spot-checked where accessible. Star counts are intentionally omitted because they drift quickly.

## Wardian Target Shape

Wardian's shared memory goal is broader than a vector database and narrower than replaying full provider sessions. The useful target is a local-first memory system that captures two different memory classes:

- **Procedural memory**: evidence-backed traces of what actually happened, especially surprising or corrective events such as user corrections, failed assumptions, recovered errors, workflow outcomes, tool outputs, agent-to-agent handoffs, and decisions made under uncertainty.
- **Semantic memory**: distilled information worth reusing, such as facts, project conventions, durable preferences, known fixes, architectural decisions, class-specific practices, and cross-agent lessons.

Those classes should share provenance and scope rules, but they should not share the same ingestion policy. Procedural memory needs faithful traces and event boundaries. Semantic memory needs extraction, contradiction handling, confidence, review, and promotion.

Wardian should optimize for:

- local-first persistence and inspectability
- efficient recall that avoids loading whole transcripts
- explicit scopes such as global, class, agent, workspace, and workflow run
- backend-mediated writes with audit events
- optional human-readable Markdown projections
- scalable retrieval over many agents and long-running sessions

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [Egregore](https://github.com/egregore-labs/egregore) | Git-backed Markdown handoffs, questions, people, and activity protocols. | Copy the agent-neutral verbs and inspectable record contracts, but do not use Git as Wardian's operational memory store. |
| [caura-memclaw](https://github.com/caura-ai/caura-memclaw) | Governed fleet memory with REST, MCP, scoped recall, trust tiers, audit logs, pgvector, and local embedder options. | Wardian needs scoped memory correctness, per-agent trust/write policy, and auditability as core requirements, not optional enterprise features. |
| [Dolt](https://github.com/dolthub/dolt) | SQL database with Git-style commit, branch, merge, diff, blame, push, and pull semantics for table data. | Versioned memory rows are more useful than versioned files when memory needs queries, access logs, branches, review, and conflict handling. |
| [DoltHub options repo](https://www.dolthub.com/repositories/post-no-preference/options/doc/master) | Public data repository hosted as a Dolt database. | Treat shared datasets as queryable, inspectable data products; Wardian memory exports could be table-shaped and shareable without making Git the runtime store. |
| [Graphify](https://github.com/safishamsi/graphify) | Local-first folder-to-knowledge-graph extraction over code, docs, media, and rationale, with cached graph output. | Build structural indexes over artifacts and transcripts so agents navigate compressed graphs before reading raw evidence. |
| [qmd](https://github.com/tobi/qmd) | Local Markdown search using SQLite, BM25, vector search, query expansion, RRF, and chunk reranking. | Wardian recall should use a hybrid local retrieval stack, not vector search alone. |
| [rtk](https://github.com/rtk-ai/rtk) | CLI proxy and hook system that filters command output before it reaches agent context. | Procedural memory should store full evidence while recalling compact, command-aware summaries with recovery links to raw output. |
| [supermemory](https://github.com/supermemoryai/supermemory) | Memory and context engine with fact extraction, profiles, connectors, container tags, hybrid search, and memory graphs. | Separate memory extraction, context injection, and graph visualization while preserving source metadata and scoping tags. |
| [Chroma Context-1](https://huggingface.co/chromadb/context-1) | Agentic retrieval model that decomposes queries and self-edits context inside a dedicated harness. | Complex recall can become a bounded retrieval subagent, but the harness and budget policy matter as much as the model. |
| [context-hub](https://github.com/andrewyng/context-hub) | Curated docs and skills registry with local annotations, progressive disclosure, source trust, and local/private sources. | Wardian should distinguish "what to know" memories from "how to act" skills, and treat agent-written annotations as untrusted until explicitly included. |
| [hindsight](https://github.com/vectorize-io/hindsight) | Retain, recall, reflect architecture with memory banks, hook integrations, and optional local server. | Wardian can use retain/recall/reflect as verbs, but should preserve raw procedural evidence instead of only storing extracted facts. |
| [Mem0](https://arxiv.org/abs/2504.19413) | Selective long-term memory architecture with extraction, consolidation, retrieval, and graph-enhanced variants. | Selective memory can reduce full-context replay cost and latency, but Wardian should keep extraction separate from evidence capture. |
| [Zep / Graphiti](https://arxiv.org/abs/2501.13956) | Temporal knowledge graph memory with episode provenance, historical relationships, and hybrid graph/vector/full-text retrieval. | Wardian should model validity, supersession, contradiction, and point-in-time recall even if the first implementation uses relational tables. |
| [Letta / MemGPT](https://docs.letta.com/guides/core-concepts/memory/context-hierarchy/) | Memory hierarchy with always-visible memory blocks, files, archival memory, and external retrieval tools. | Wardian needs context priority tiers and shared blocks, but shared writes should remain backend-mediated and policy-gated. |
| [LangGraph / Deep Agents memory](https://docs.langchain.com/oss/javascript/deepagents/memory) | Filesystem-backed long-term memory with scopes, read/write permissions, background consolidation, and semantic/episodic/procedural distinctions. | Wardian can reuse the scoped memory taxonomy while keeping its own SQL event log and Markdown projections. |
| [MIRIX](https://arxiv.org/abs/2507.07957) | Multi-agent memory system with typed memory managers for core, episodic, semantic, procedural, resource, and vault memory. | Wardian should avoid a single undifferentiated note store; resources, traces, facts, and procedures need distinct record types. |
| [MemoryOS](https://arxiv.org/abs/2506.06326) and [MemOS](https://huggingface.co/papers/2507.03724) | Memory-as-managed-resource architectures with short/mid/long tiers, lifecycle operations, metadata, provenance, and versioning. | Wardian should copy lifecycle and scheduling vocabulary, not parameter-memory ambitions or cloud assumptions. |
| [MemX](https://arxiv.org/abs/2603.16171) | Local-first Rust/libSQL memory baseline with explainable retrieval and conservative miss-query suppression. | Wardian recall should be allowed to say "no reliable memory found" rather than forcing a weak match. |
| [MemMachine](https://arxiv.org/abs/2604.04853) | Ground-truth-preserving memory with episodic evidence and adaptive retrieval strategies. | Preserving raw episodes plus span-level evidence is central for Wardian's surprise-driven procedural memory. |
| [Agent Memory as Database](https://arxiv.org/abs/2605.26252) | Database-oriented memory literature arguing long-term memory correctness depends on state trajectory, not isolated records. | Wardian should treat ingestion, revision, forgetting, and retrieval as governed state transitions with tests and audit trails. |

## Reference Notes

### Egregore

Egregore's useful contribution is its runtime-neutral collaboration protocol. Agents work against an inspectable `memory/` repository and use shared verbs such as `sync`, `activity`, `handoff`, `ask`, and `answer`. The Markdown file contracts for handoffs and questions are simple enough for heterogeneous agents to use without sharing a provider runtime.

Wardian should borrow the protocol vocabulary and record shapes, especially for handoffs and asynchronous questions. Wardian should not borrow the assumption that Git is the primary operational store. Git is good for reviewable exports and distributed human diffs; without extra locking, indexing, and audit layers, Git-backed Markdown is weaker for Wardian's live concurrent memory workload than an operational database.

### caura-memclaw

MemClaw frames memory as a fleet problem rather than a single-chatbot problem. Its docs emphasize multi-tenant, multi-agent deployments, scoped memory, cross-agent propagation, trust tiers, audit logs, and token-efficient recall. Its API surface charter separates REST, MCP, and plugin responsibilities rather than making every surface symmetric.

Useful Wardian patterns:

- write, recall, and compound as a durable loop
- explicit agent-scoped credentials or identity-bound writes
- trust-gated operations for cross-agent or global memory
- audit logs for cross-scope reads
- separate agent-facing MCP-like operations from UI/admin operations
- local embedder option for privacy-sensitive deployments

Wardian should adapt these ideas to a local-first backend. For a desktop command center, the first-class control point should be the Rust backend and `wardian` CLI, with provider agents calling narrow commands rather than writing memory files directly.

### Dolt And Versioned Data

Dolt is relevant because it combines SQL access with Git-style version control over table data. Its README describes a database that can fork, clone, branch, merge, push, pull, diff, blame, and commit tables while remaining MySQL-compatible.

For Wardian, the important lesson is not necessarily "use Dolt." The lesson is that memory lineage can be table-shaped. A memory system needs operational database features such as indexes, joins, constraints, and transactions, but it also benefits from reviewable history, branches, diffs, and blame-like provenance.

Potential Wardian translation:

- SQLite remains the default local operational store.
- Every memory mutation writes an append-only event row.
- Semantic memory records have revisions and supersession links.
- Review flows can branch or stage candidate memories before promotion.
- Markdown/Git export is a projection, not the source of truth.

### Graphify

Graphify turns folders of code, docs, PDFs, images, videos, and transcripts into a queryable graph. Its docs describe local tree-sitter extraction for code, local faster-whisper transcription for media, LLM extraction for documents/images/transcripts, NetworkX graph output, confidence labels, community detection, and SHA256 caching. It also distinguishes extracted, inferred, and ambiguous edges.

This maps strongly to Wardian procedural memory. Wardian already has many evidence-rich sources: terminal output, agent transcripts, workflow telemetry, code diffs, PRs, docs, screenshots, and user corrections. A graph layer can make those artifacts navigable without injecting the raw artifacts into every prompt.

Useful Wardian patterns:

- deterministic local extraction before LLM extraction
- confidence labels for extracted versus inferred relationships
- graph reports with "surprising connections" and suggested questions
- content-hash caching so unchanged artifacts are not reprocessed
- graph output as a compact navigation surface before raw evidence

Wardian should avoid relying only on an inferred graph. For memory, graph edges should point back to evidence spans and event records.

### qmd

qmd is a local Markdown search engine with BM25 full-text search, vector semantic search, LLM reranking, MCP integration, local SQLite storage, and local models through node-llama-cpp. Its search pipeline includes a BM25 probe, query expansion, typed lexical/vector/HYDE searches, RRF fusion, chunk selection, reranking on chunks rather than whole bodies, and deduplication.

This is a useful reference for Wardian recall. Agent memory retrieval should not be a single vector lookup. Some memories are found by exact identifiers, class names, file paths, error strings, request IDs, workflow IDs, dates, or agent names. Others need semantic retrieval. A reliable recall path combines structured filters, lexical search, vector search, and reranking over small chunks.

Useful Wardian patterns:

- SQLite-backed local index
- BM25 or FTS for exact/error-string recall
- optional local vector index for semantic recall
- query expansion only when cheap lexical evidence is weak
- rerank chunks, not whole transcripts
- MCP/CLI output formats designed for agent workflows

### rtk

rtk is not a memory system, but it is directly relevant to procedural memory efficiency. It proxies shell commands and returns filtered command-aware output to reduce tokens while preserving raw execution in the actual shell path. Its docs describe smart filtering, grouping, truncation, deduplication, hook-based command rewriting, project-local and user-global filters, token-savings tracking, and recovery paths to full output.

Wardian should apply this pattern to memory recall:

- Store full procedural evidence in artifacts or transcript spans.
- Recall compact summaries by default.
- Preserve recovery links to exact raw evidence.
- Use command-aware filters for terminal-heavy evidence.
- Track token savings and parse failures to improve filters.

This is especially important for "surprise" capture. A failed test log may contain 5,000 lines, but the durable procedural memory should point to the raw log while surfacing the handful of lines that explain the surprise.

### supermemory

Supermemory presents a broad memory/context engine: fact extraction from conversations, user profiles, hybrid search, connectors, multimodal processing, project/container tags, and graph visualization. Its integrations show both input processors that fetch and inject memories before model calls and output processors that save conversations afterward.

Useful Wardian patterns:

- separate input-side recall from output-side retention
- container tags as a flexible scoping model
- custom IDs for joining memories to external records
- metadata filters for search
- profile-style summaries for stable context
- graph views over documents and memory entries

Wardian should be more conservative about automatic saving than a general memory API. Agent output should become semantic memory only through backend policy: explicit user request, workflow rule, surprise detector, or promotion review.

### Chroma Context-1

Context-1 is a 20B agentic search model trained to retrieve support documents for complex multi-hop queries. Its model card describes query decomposition, parallel tool calling, self-editing context, and a required agent harness that manages tool execution, budgets, pruning, and deduplication.

For Wardian, the useful pattern is the harness boundary. Complex recall can be delegated to a retrieval subagent, but the subagent should operate inside a strict budget with explicit tools, scoped indexes, deduplication, and citations back to memory records.

Wardian should not make a large agentic retrieval model a baseline requirement. The local-first baseline should be deterministic filters, SQLite FTS, optional embeddings, and bounded reranking. Agentic recall can be an advanced layer for difficult multi-hop queries.

### Context Hub

Context Hub separates curated "docs" from "skills," supports local/private sources, uses a local cache, and lets agents attach annotations to docs. Its docs explicitly treat annotations as untrusted user-mutable input that is not included by default.

This maps cleanly to Wardian's split between semantic memory and skills:

- Reference knowledge answers "what to know."
- Skills and class prompts answer "how to act."
- Agent annotations are useful but should not automatically become trusted instructions.
- Progressive disclosure keeps large references out of prompt context until needed.
- Source trust labels help humans and agents calibrate official, maintainer, community, and local knowledge.

Wardian should use the same trust posture for semantic memory: a memory note can be available without being injected. The retrieval result should carry source, scope, confidence, and trust state so the prompt builder can decide how to label it.

### Hindsight

Hindsight's public docs organize memory around retain, recall, and reflect. It stores documents, conversations, and raw content by analyzing them into structured memories, supports memory banks, configurable retain/reflect missions, hooks for Codex, and webhook events for memory operations.

Useful Wardian patterns:

- explicit retain, recall, and reflect verbs
- memory banks as scope containers
- bank templates for repeatable agent or project memory configuration
- hook integration points for session start, prompt submit, and stop
- event delivery tied transactionally to memory operations
- dynamic bank IDs for agent/project scoping

Wardian should diverge on one point: procedural memory should explicitly preserve raw evidence spans alongside extracted facts. Hindsight emphasizes extracted and structured memory representations; Wardian needs the actual conversation trace or terminal evidence to remain inspectable so later agents can audit surprise, causality, and extraction quality.

## Long-Term Memory Literature

The long-term memory systems reviewed online converge on a few patterns. They do not treat memory as a passive vector index. They manage memory as a lifecycle: capture evidence, extract candidates, revise claims, forget or expire stale material, retrieve with temporal and scope constraints, and evaluate by query type.

### Selective Extraction And Consolidation

Mem0 is useful as a production-oriented reference because its paper argues against full-context replay. The paper reports higher accuracy than compared memory baselines on LOCOMO while reducing latency and token cost versus full-context approaches. The architectural pattern is selective extraction from conversation, consolidation of related memories, retrieval of relevant memories, and optional graph enrichment for relationships.

Wardian should copy the selectivity, not the assumption that extracted memory is enough. Extraction-on-write is lossy: it can omit the surprising detail, overstate a preference, or miss the tool output that made a lesson true. Wardian should retain raw procedural evidence first, then let extraction create candidate semantic records.

### Temporal Graph Memory

Zep and Graphiti are important because they make time a first-class memory dimension. Graphiti models entities, facts, relationships, validity windows, and raw episodes as provenance. Retrieval combines semantic, keyword, graph, and temporal constraints.

Wardian does not need a graph database as its first storage engine to learn from this. A relational schema can still carry:

- `valid_from` and `valid_to`
- `observed_at` and `superseded_at`
- `derived_from_evidence_id`
- contradiction and supersession relations
- point-in-time query filters
- graph-like traversal over `memory_relations`

This matters for long-term use because stale facts are often worse than missing facts. The system needs to answer "what did we believe then?", "what is active now?", and "which evidence superseded that claim?"

### Memory Hierarchy And Shared Blocks

Letta and MemGPT remain useful references for memory hierarchy. Their architecture separates always-visible core memory from out-of-context recall and archival memory. Current Letta docs also distinguish memory blocks, files, archival memory, external RAG, shared memory blocks, read-only policy blocks, and concurrent update caveats.

Wardian should adapt this hierarchy:

- **Core context**: small, high-confidence, always-visible preferences or project facts.
- **Recall memory**: searchable procedural traces and conversation/workflow history.
- **Archival memory**: lower-priority semantic records and external references.
- **Skill/procedure memory**: instructions for how agents should act.
- **Shared blocks**: curated class/global/workspace context with explicit ownership.

The key difference is authority. Wardian should not let provider agents freely rewrite global or class memory blocks. Agents can append evidence or propose candidates, while the Rust backend enforces permissions, concurrency, promotion policy, and audit rows.

### Filesystem Memory And Scoped Namespaces

LangGraph and Deep Agents show a pragmatic filesystem-backed pattern: memory can be scoped by namespace, loaded on demand, marked read-only or writable, updated during a conversation or consolidated in the background, and split into semantic, episodic, and procedural forms. This maps closely to Wardian's existing preference for inspectable disk state and skills-as-files.

The Wardian translation should be SQL plus filesystem projection:

- SQL event rows and indexes remain authoritative.
- Markdown files expose selected memory to humans and agents.
- Skills remain procedures, not generic facts.
- Background consolidation may propose memory candidates, but should not silently promote cross-scope claims.
- Read-only policy/procedure projections should be clearly labeled and protected from agent writes.

### Typed Memory Managers

MIRIX and related systems push against flat memory by using multiple memory types and dedicated managers. MIRIX's taxonomy includes core, episodic, semantic, procedural, resource, and vault memory, and its paper reports evaluations across text-only long conversation and multimodal screenshot settings.

Wardian's version should be narrower and local-first, but the typed separation is valuable:

- **Episodic/procedural traces**: what happened and how the agent solved or failed.
- **Semantic records**: reusable facts or conventions.
- **Procedural rules**: reviewed instructions or skills.
- **Resource memory**: files, logs, screenshots, diffs, reports, and external links.
- **Vault/protected memory**: secrets-adjacent or sensitive evidence that should not be recalled across scopes by default.

Long-term memory becomes safer when each type has its own retention, retrieval, promotion, and redaction rules.

### Memory As Managed Resource

MemoryOS and MemOS are useful as lifecycle references. MemoryOS frames memory around short-term, mid-term, and long-term stores with update, retrieval, and generation modules. MemOS frames memory as a managed system resource with units that carry content, metadata, provenance, versioning, and lifecycle operations such as composition, migration, and fusion.

Wardian should not adopt the more ambitious parameter-memory or activation-memory parts as a baseline. The useful piece is governance language: memory objects should have owners, versions, freshness, retention state, confidence, provenance, access policy, and lifecycle transitions.

### Local-First And Ground-Truth-Preserving Baselines

MemX is relevant because it emphasizes local-first implementation, Rust/libSQL, explainable retrieval, full-text indexing, hybrid retrieval, and conservative rejection when no reliable memory is found. MemMachine is relevant because it argues for preserving episodic ground truth while layering adaptive retrieval over sentence or span-level evidence.

Both support Wardian's current direction:

- local embedded database first
- exact lexical retrieval for identifiers and errors
- semantic retrieval where useful
- evidence spans as first-class records
- low-confidence rejection
- retrieval strategies that change by query type

The practical requirement is that recall can return "no reliable memory found." A memory system that always returns something will eventually inject stale or irrelevant context into agents.

### Database-Oriented Memory Correctness

Recent database-oriented memory literature argues that long-term memory correctness is not just record correctness. It is trajectory correctness: how the memory state changes through ingestion, revision, forgetting, and retrieval. This matches Wardian's needs better than a pure vector-store framing.

Wardian should evaluate memory by state transitions:

- Did this event deserve capture?
- Was the right candidate extracted?
- Was it scoped narrowly by default?
- Did promotion preserve evidence?
- Did a later contradiction supersede it?
- Was stale memory excluded from prompt context?
- Was cross-scope access logged?

This is the practical answer to the SQL-versus-Git question. SQL is not inherently sufficient, but it gives Wardian the substrate for transactions, indexes, joins, constraints, audit rows, and lifecycle state. Git and Markdown remain excellent review/export surfaces.

## Wardian Design Pressures

### SQL First, Markdown Projection Second

Git-backed Markdown is useful for portability and review, but it is not sufficient as the standalone operational substrate for Wardian memory. Wardian needs fast local queries, concurrent agent access, access logs, scope checks, structured provenance, and transactionally attached evidence.

Recommended source-of-truth shape:

- `memory_records`: stable semantic records, procedural records, decisions, lessons, handoffs, and questions
- `memory_events`: append-only create, read, update, supersede, promote, reject, and access events
- `memory_scopes`: global, class, agent, workspace, workflow, and run bindings
- `memory_evidence`: transcript span, terminal output, workflow artifact, file diff, screenshot, or external URL references
- `memory_relations`: supersedes, contradicts, supports, derived-from, related-to, same-incident-as
- `memory_indexes`: FTS/vector/graph index state and freshness metadata
- `memory_lifecycle`: candidate, active, superseded, rejected, expired, redacted, and deleted states

Markdown should be generated for human review, export, docs, and Git sync. It should not be the only place Wardian records access or mutation behavior.

SQLite also has operational constraints: it supports many readers but a single writer at a time. Wardian should make memory writes backend-mediated, use WAL mode and busy timeouts where appropriate, and queue or serialize write transactions through the Rust backend. Artifact writes and database event rows should have a defined commit order so an evidence reference never points to a missing artifact and a stored artifact is either indexed or marked pending.

### Procedural Memory Capture

Procedural memory should start from events Wardian already controls:

- agent prompts and replies
- `wardian ask` and `wardian reply`
- terminal output and provider-adapted transcripts
- workflow node inputs, outputs, errors, and telemetry
- test/build/lint command summaries
- user corrections and explicit "remember this" requests
- reviewer findings and blocked states

Wardian should not retain every turn equally. A surprise detector should mark candidate traces when something violates expectation or changes future behavior:

- user correction of an agent assumption
- failed command followed by a fix
- test failure with a non-obvious resolution
- reviewer rejection or contradiction
- repeated error across agents
- workflow branch that changed because of runtime evidence
- agent handoff that alters next-step procedure
- explicit user promotion

The procedural record should include the actual trace span, a compact summary, involved agents, timestamps, artifacts, and the reason it was captured.

Capture must also include a redaction and retention boundary. Terminal output, transcripts, screenshots, diffs, provider logs, and external artifacts may contain secrets or personal data. Wardian should redact before promotion or indexing where possible, keep raw evidence under explicit retention and delete policy, and require opt-in or policy-gated behavior for cross-scope recall of sensitive evidence.

### Semantic Memory Promotion

Semantic memory should be promoted from procedural evidence, not written as free-floating claims. A promoted record should carry:

- statement
- scope
- source evidence
- confidence
- authoring agent or user
- extraction method
- status: candidate, active, superseded, rejected, expired
- related records and contradictions

Automatic extraction can create candidate memories. Active cross-agent memory should require policy: user approval, trusted workflow, high-trust agent, or scoped class/global rules.

The write path should be three-stage:

1. **Evidence capture** records the trace, artifact, event boundary, actors, and surprise reason.
2. **Candidate extraction** proposes semantic facts, procedural lessons, contradictions, or handoff records.
3. **Reviewed activation** promotes selected candidates into active scoped memory.

Background consolidation can run between sessions or workflow runs, but it should propose rather than silently activate cross-agent or global memories.

### Scoping Model

Wardian should make scope explicit at both write and recall time:

- **Global**: applies across Wardian.
- **Class**: applies to agent roles such as Coder, Reviewer, Architect, or Researcher.
- **Agent**: applies to one durable agent identity.
- **Workspace**: applies to a repo or project path.
- **Workflow**: applies to a workflow definition or recurring run.
- **Run**: applies only to one workflow run or task instance.

Default writes should be narrow. Cross-scope reads should be logged. Cross-scope writes should be trust-gated or reviewed.

Workspace scope needs a durable identity model. Repo paths can move, worktrees can share history, cloned agents can operate from different absolute paths, and branches can temporarily represent different tasks. Wardian should canonicalize workspace identity from stable project metadata where available, not only from the current filesystem path.

### Retrieval Stack

The local-first baseline should combine:

- structured filters over scope, record type, timestamp, agent, workspace, workflow, and status
- temporal filters and point-in-time recall
- SQLite FTS for exact and lexical recall
- optional local embeddings for semantic recall
- content-hash-based reindexing
- graph relations for navigation and contradiction review
- supersession and contradiction handling before prompt injection
- reranking over small chunks or summaries
- low-confidence rejection when no reliable memory is found
- prompt budgets and recovery links to raw evidence

Agentic retrieval can sit above this stack as an optional advanced path for hard multi-hop queries. It should be bounded by scope, budget, and citation requirements. Retrieval should be evaluated by query class: exact identifier, temporal question, contradiction check, multi-hop reasoning, workflow outcome, user preference, and procedure lookup.

### UI And CLI Surface

Wardian should expose memory through surfaces that match the work:

- CLI verbs for agents: retain, recall, promote, handoff, ask, answer, annotate, list, show.
- UI review queue for candidate semantic memories and cross-scope promotions.
- Agent roster indicators for pending handoffs, questions, and stale memory candidates.
- Workflow nodes for scoped recall and explicit promotion.
- Evidence viewer for raw trace spans and filtered summaries.
- Markdown export for review and external sync.
- Evaluation views for stale memories, contradictions, low-confidence misses, and cross-scope reads.

### Evaluation Targets

Public benchmarks such as LOCOMO and LongMemEval are useful for vocabulary and stress cases, but Wardian needs native evaluations that match its agent habitat:

- user correction captured as evidence and promoted into a scoped rule
- terminal failure summarized compactly while preserving raw log evidence
- repeated failed assumption across agents detected as a procedural candidate
- `wardian ask` handoff recalled by the receiving agent with source links
- workflow artifact remembered for the workflow/run scope, not global scope
- worktree-specific fact excluded from a different branch or workspace
- superseded project convention not injected after a later correction
- sensitive transcript span redacted before cross-scope recall
- retrieval correctly returns no result for unrelated queries

### Things To Avoid

- Treating provider session history as durable memory.
- Letting agents write global memory files directly.
- Injecting all available memory into every session.
- Collapsing procedural traces and semantic facts into one undifferentiated note type.
- Using vector similarity as the only recall mechanism.
- Trusting agent-written annotations as instructions by default.
- Treating every extracted fact as active memory before review or policy activation.
- Forcing recall to return a weak match instead of a reliable miss.
- Making cloud services, hosted graphs, or large retrieval models required for the local baseline.

## Candidate Wardian Vocabulary

- **Evidence**: raw or minimally transformed artifact from a conversation, terminal, workflow, file, or external source.
- **Trace**: bounded procedural sequence with actors, timestamps, inputs, outputs, and surprise reason.
- **Memory**: promoted reusable semantic or procedural record with scope and provenance.
- **Candidate**: extracted or proposed memory awaiting policy or user approval.
- **Promotion**: explicit transition from evidence/candidate to active memory.
- **Scope**: where a memory may be read and where it may be written.
- **Recall**: scoped retrieval that returns compact context plus evidence links.
- **Reflect**: synthesis over recalled memories, usually producing candidate memories or decisions.
- **Annotation**: untrusted note attached to a record, artifact, doc, or skill.
- **Lifecycle**: governed state transition over capture, extraction, activation, revision, forgetting, redaction, and deletion.
- **Supersession**: explicit replacement of an older memory by a newer evidence-backed record.
- **Miss**: a deliberate retrieval result meaning no reliable memory was found within scope and budget.

## Initial Recommendation

For Wardian, the recommended architecture is SQL-first local memory with Markdown projections:

1. Keep SQLite or an equivalent embedded relational store as the operational source of truth.
2. Store procedural evidence spans separately from promoted semantic records.
3. Use explicit scopes and audit every cross-scope read/write.
4. Promote semantic memory through evidence capture, candidate extraction, and reviewed activation.
5. Model temporal validity, supersession, contradictions, retention, and redaction from the beginning.
6. Build hybrid retrieval over structured filters, FTS, optional vectors, graph links, chunk reranking, and low-confidence misses.
7. Export Markdown for transparency and Git review, but do not require Git for live memory operation.

This keeps Wardian aligned with its local command-center identity: inspectable on disk, governed by the Rust backend, efficient for many agents, and honest about the difference between what happened and what the system believes is worth remembering.
