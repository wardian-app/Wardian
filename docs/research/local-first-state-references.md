# Local-First State References

This document maps public local-first state, sync, and embedded-database systems to design patterns relevant to Wardian's disk-inspectable state, live desktop UI, CLI interoperability, and future multi-device or multi-agent sync.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, or documentation where available.

## Design Axes

- **Local authority**: whether the application can continue to read and write useful state without a server.
- **Sync primitive**: whether the system syncs events, CRDT operations, row changes, database snapshots, or application-specific mutations.
- **Conflict model**: whether conflicts are impossible by construction, resolved automatically, rebased, rejected by a backend, or surfaced to humans.
- **Observable truth**: whether durable state can be inspected through files, SQLite, event logs, Markdown, or structured records.
- **Reactive UI**: whether local writes update the UI synchronously before remote propagation.
- **Agent compatibility**: whether agents can safely read, write, replay, or repair state without depending on a hidden cloud service.
- **Schema evolution**: whether on-disk state has explicit versions, migrations, and compatibility boundaries.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [LiveStore](https://github.com/livestorejs/livestore) | Reactive SQLite state layer with event-sourced sync. | Strong reference for separating local materialized state from replayable events. |
| [PowerSync](https://powersync.com/) | Backend-database-to-local-SQLite sync engine. | Useful for local UI responsiveness with explicit backend write acceptance. |
| [Triplit](https://github.com/aspen-cloud/triplit) | Full-stack syncing database with client/server real-time sync. | Query-scoped sync and property-level conflict handling are relevant to roster/workflow views. |
| [Jazz](https://github.com/garden-co/jazz) | Distributed local-first relational database. | Strong reference for relational semantics plus history, permissions, and sync. |
| [SQLite Sync](https://github.com/sqliteai/sqlite-sync) | CRDT-based offline-first sync extension for SQLite. | Directly relevant to Wardian because it mentions AI agent memory and Markdown knowledge-base sync. |
| [Automerge](https://github.com/automerge/automerge) | CRDT document model and sync. | Useful where Wardian needs collaborative document/workflow editing. |
| [Yjs](https://github.com/yjs/yjs) | Mature shared-data CRDT ecosystem. | Useful for graph or editor collaboration if Wardian later adds multi-user live editing. |
| [ElectricSQL](https://github.com/electric-sql/electric) | Postgres-to-local reactive sync. | Relevant if Wardian ever mirrors local state from a team/shared service. |

## Reference Profiles

### LiveStore

**Source basis:** Public repo and docs checked.

**What it includes:** LiveStore is a client-centric state-management framework built around reactive embedded SQLite and built-in sync. Its public design uses event sourcing: local events are persisted, synced, and materialized into SQLite tables used by the app.

**Distinctive components:**

- Reactive SQLite as the application data layer.
- Event log separate from materialized state.
- Pull-before-push sync and local event rebase.
- Synchronous UI updates from local materialized queries.
- Explicit design discussion around compaction, conflicts, and partitioning.

**Wardian relevance:** Wardian already has multiple truths: app state, CLI-readable state, Markdown/docs, workflow files, PTY streams, and provider telemetry. LiveStore suggests a useful split: store append-only events for replay and audit, then materialize fast local Habitat views.

### PowerSync

**Source basis:** Public site checked.

**What it includes:** PowerSync keeps a local in-app SQLite database updated from a backend database, with offline reads/writes and upload queues for local mutations. Writes can be applied locally immediately and then sent to a backend API that accepts or rejects them according to application logic.

**Distinctive components:**

- Local SQLite as the app-facing database.
- Automatic partial sync from backend database to client.
- Local write queue for offline use.
- Backend-controlled write validation.
- SDK orientation across client platforms.

**Wardian relevance:** PowerSync is useful where Wardian needs fast local interaction but eventual authoritative validation. For example, workflow edits, agent roster changes, and skill activation changes could become immediate local events while still allowing a later validator to accept, reject, or repair them.

### Triplit

**Source basis:** Public repo checked.

**What it includes:** Triplit is a full-stack syncing database that runs across server and client. It syncs data between server and browser in real time, supports pluggable storage such as IndexedDB, SQLite, and Durable Objects, and handles incremental updates and conflict resolution.

**Distinctive components:**

- TypeScript-first client/server data layer.
- Real-time query sync.
- Incremental update propagation.
- Property-level conflict handling.
- Framework adapters for common frontend stacks.

**Wardian relevance:** Wardian's Habitat views are query-heavy: "which agents are active?", "which workflows are stuck?", "which skills are enabled?", "which runs changed?". Triplit is a useful reference for making those views query-scoped and reactive instead of rebuilding a whole global state object.

### Jazz

**Source basis:** Public repo summary and architecture material checked.

**What it includes:** Jazz is a distributed local-first database with relational semantics, TypeScript client layers, Rust core work, storage/sync abstractions, permissions, schema concepts, history, and replayable state.

**Distinctive components:**

- Relational model with local-first distribution.
- Row histories and reserved metadata columns.
- Query-scoped sync.
- Schema files, permissions, and migrations.
- Reactive subscriptions over local state.
- Architecture docs that explicitly separate current state, history, sync, and durability.

**Wardian relevance:** Jazz is a strong conceptual reference for Wardian because Wardian needs both inspectable local truth and structured relational views. Agent sessions, workflow runs, skills, classes, workspaces, and events are relational enough that a durable table-first model may stay clearer than ad hoc JSON blobs.

### SQLite Sync

**Source basis:** Public repo checked.

**What it includes:** SQLite Sync is an offline-first sync extension for SQLite using CRDTs. It syncs with SQLite Cloud, PostgreSQL, and Supabase and explicitly calls out AI agent memory, Markdown knowledge bases, and distributed pipelines as use cases.

**Distinctive components:**

- SQLite extension rather than a full app framework.
- CRDT-based conflict-free sync.
- Local writes with later merge.
- AI-agent memory and Markdown sync positioning.
- Block-level last-writer-wins behavior intended to preserve different document sections.

**Wardian relevance:** This is directly relevant to Wardian's "Markdown-as-Truth" and multi-agent habitat framing. Even if Wardian does not adopt SQLite Sync, its problem statement is close: many agents and surfaces may edit shared local knowledge, and the product needs a strategy for merging without hiding what happened.

### Automerge

**Source basis:** Public repo checked at a high level.

**What it includes:** Automerge is a CRDT library for building local-first applications where users or processes can independently edit shared documents and merge changes later.

**Distinctive components:**

- JSON-like document model.
- Conflict-free merge.
- Offline editing.
- Sync protocol and storage ecosystem.
- Collaboration-first posture.

**Wardian relevance:** Automerge is most relevant to Wardian workflow definitions, notes, and agent-authored documents. It is less directly relevant to high-volume telemetry, where event logs or SQLite tables are likely a better fit.

### Yjs

**Source basis:** Public repo checked at a high level.

**What it includes:** Yjs is a mature CRDT ecosystem used for collaborative editing, shared data structures, and real-time synced application state.

**Distinctive components:**

- Shared maps, arrays, text, and XML-like structures.
- Provider ecosystem for WebSocket, WebRTC, IndexedDB, and other transports.
- Strong editor integration ecosystem.
- Awareness/presence patterns.

**Wardian relevance:** Yjs matters if Wardian adds true multi-user graph editing, collaborative workflow authoring, or live shared notes. It should not be the default for core agent telemetry unless Wardian needs collaborative document semantics.

### ElectricSQL

**Source basis:** Public repo/site checked at a high level.

**What it includes:** ElectricSQL is a local-first sync system oriented around Postgres and local reactive clients. It is useful as part of the broader local-first database landscape even though Wardian's near-term posture is local desktop state rather than cloud-backed team sync.

**Distinctive components:**

- Postgres-backed sync.
- Shape/query-driven local replication.
- Local reactive reads.
- Web app integration posture.

**Wardian relevance:** ElectricSQL becomes more relevant if Wardian adds team/workspace sync, remote dashboards, or shared organizational state. For the near-term local-first app, event logs and embedded SQLite are probably more immediately actionable.

## Wardian Positioning

Wardian should avoid choosing between "files as truth" and "database as truth" too early. The stronger pattern is layered:

```text
filesystem artifacts + event log -> normalized local database -> reactive UI and CLI views
```

That keeps agent-readable files and Markdown visible while allowing the app to query status, dependencies, run history, and activation state efficiently.

Near-term implications:

- Treat important state changes as replayable events, not only mutable rows.
- Keep stable file formats for workflows, skills, and specs.
- Use SQLite/materialized views for fast Habitat queries.
- Version all durable state formats explicitly.
- Separate local acceptance from later validation or remote sync.
- Make merge/conflict events visible to humans and agents.
