# Knowledge Task System (KTS) Milestone 1

* **Status:** Proposed
* **Date:** 2026-03-15
* **Decider:** Architect

> Note: This proposal is partially superseded by [2026-04-17 Evidence First Memory](./2026-04-17-evidence-first-memory.md). KTS remains valid as a promoted knowledge layer, but it is no longer the recommended base memory substrate.

## Context and Problem Statement
Wardian needs a way for agents to store and retrieve "Atoms" of knowledge (tasks, facts, project metadata) that is both human-readable (for Git versioning) and high-performance (for agent indexing). Current session logs are too bloated and unstructured for long-term "Self-Improvement" workflows.

## Proposed Decision
We will implement the **Knowledge Task System (KTS)** as a curated knowledge layer based on the following architecture:

1. **Storage Layer**: Atoms are stored as `.md` files with a YAML frontmatter in a specialized `.wardian/atoms/` directory.
2. **Schema (Atom)**:
    ```yaml
    ---
    id: <UUID>
    type: task | fact | project
    status: pending | completed | rejected
    tags: [arch, pty, core]
    created: <ISO-8601>
    modified: <ISO-8601>
    ---
    # Title of the Atom
    Detailed description or content goes here.
    ```
3. **Rust Atom Parser**: A new module `src-tauri/src/kts/` using `serde_yaml` and `pulldown-cmark` to perform CRUD operations.
4. **Projection Layer**: On startup, the Rust backend will parse all atoms and project them into a **Global SQLite Cache** (using `rusqlite`) for rapid SQL/semantic search without reading all files into memory every time.

Under [2026-04-17 Evidence First Memory](./2026-04-17-evidence-first-memory.md), this projection should consume both promoted atoms and raw evidence, with atoms serving as higher-confidence curated records rather than the first ingestion layer.

## Consequences
* **Positive**: Human-readable, Git-friendly, and extremely fast for agent retrieval.
* **Positive**: Decouples knowledge from individual agent sessions.
* **Negative**: Requires synchronization logic between the filesystem and the SQLite cache.
* **Negative**: Increases the complexity of the Rust backend.
