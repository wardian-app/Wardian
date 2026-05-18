# Codex Provider Support

* **Status:** Implemented
* **Date:** 2026-03-27
* **Decider:** Architect

## Context and Problem Statement
Wardian already has a multi-provider backend, but the current implementation only supports Gemini and Claude. Codex support is required for local agent sessions, headless execution, and telemetry without expanding the workflow-engine merge surface.

Codex also differs from Gemini and Claude in one important way: it uses `AGENTS.md` directly for repository instructions instead of a provider-specific markdown file.
Wardian also stores shared instructions and deployed skills outside the active repository tree under `~/.wardian/`, which Codex does not discover natively from `--add-dir` roots.

## Decision
We will add Codex as a first-class provider in the Rust backend and expose it in the agent configuration UI.

### 1. Provider Integration
- Add a `CodexProvider` implementation under `src-tauri/src/providers/`.
- Resolve `codex` from `ProviderFactory`.
- Support:
  - interactive resume via `codex resume <session_id>`
  - headless bootstrap via `codex exec --json`
  - headless follow-up runs via `codex exec resume <session_id> --json`

### 2. Instruction Model
- `AGENTS.md` remains the canonical instruction source for every class.
- Gemini and Claude continue to use provider-specific stub files.
- Codex does not introduce a `CODEX.md` stub because the CLI reads `AGENTS.md` natively.
- For providers that need Wardian-managed context in scope, Wardian creates a neutral per-session `habitat` directory.
- The habitat contains a generated root `AGENTS.md`, a linked `workspace/` junction to the real project, and a merged `.agents/skills/` tree sourced from `common`, `classes/<role>`, and `agents/<session_id>`.

### 3. Codex Native Projection
- Codex receives a projected `CODEX_HOME` inside the habitat rather than reading Wardian context directly from `--add-dir`.
- The projected Codex home preserves only shared native Codex profile files from the user's real Codex home: `auth.json`, `config.toml`, and `cap_sid`.
- Per-session Codex state remains local to the Wardian agent home, including `history.jsonl`, `session_index.jsonl`, `sessions/**`, provider logs, and SQLite state.
- On Windows, Codex elevated sandbox support is projected narrowly: `.sandbox-secrets` and `.sandbox-bin` are shared, and `.sandbox/setup_marker.json` is copied. The `.sandbox` runtime directory itself remains local so sandbox logs and setup errors are not merged across agents.
- The projected `skills/` directory overlays Wardian’s merged skill set on top of the user’s existing Codex skills and system skills.

### 4. Telemetry
- Codex session files are discovered under `~/.codex/sessions/YYYY/MM/DD/*.jsonl`.
- Query counts and init timestamps are derived from Codex session JSONL when available.

### 5. Workflow Scope
- Workflow-engine refactoring is explicitly out of scope for this branch.
- Only minimal provider-aware changes needed to avoid blocking Codex support should be considered elsewhere.

## Consequences
- **Positive**: Wardian can spawn and manage Codex-backed agents from the same provider architecture as Gemini and Claude.
- **Positive**: Codex follows the existing markdown-as-truth model cleanly because it consumes `AGENTS.md` directly.
- **Positive**: Habitat projection lets Wardian expose external instructions and skills to Codex without patching the Codex CLI.
- **Positive**: The branch avoids unnecessary workflow-engine conflicts.
- **Negative**: Codex support now depends on a projected home layout that must stay aligned with Codex’s on-disk conventions.
