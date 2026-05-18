# OpenCode Provider Support

* **Status:** Implemented
* **Date:** 2026-04-03
* **Decider:** Architect

## Context and Problem Statement

Wardian already supports multiple agent CLIs, but OpenCode was still treated as a planned provider. That left a real gap in local orchestration, especially for free debug runs and for teams that want a provider which natively understands `AGENTS.md`.

OpenCode also differs from Gemini, Claude, and Codex in an important way: it can run directly in the real project workspace while separately accepting extra instruction files and skill roots through runtime configuration. That means Wardian does not need to project a fake workspace just to expose class-scoped context.

## Decision

We will add OpenCode as a first-class provider in the Rust backend and expose it in the agent configuration UI.

### 1. Provider Integration

- Add an `OpenCodeProvider` implementation under `src-tauri/src/providers/`.
- Resolve `opencode` from `ProviderFactory`.
- Support:
  - interactive sessions via the top-level `opencode` command
  - session bootstrap and headless runs via `opencode run --format json`
  - resume via `--session <session_id>`

### 2. Working Root and Instruction Model

- OpenCode runs in the real target workspace.
- `AGENTS.md` remains the canonical instruction file for OpenCode.
- Wardian does not create a projected workspace for OpenCode.
- Instead, Wardian injects runtime configuration through `OPENCODE_CONFIG_CONTENT`.

### 3. Skill and Class Scoping

- Wardian resolves include roots from:
  - `~/.wardian/common`
  - `~/.wardian/classes/<class_name>`
  - `~/.wardian/agents/<session_id>`
  - any user-added include directories
- For each existing root, Wardian injects:
  - `AGENTS.md` paths into `instructions`
  - `.agents/skills` paths into `skills.paths`
- This preserves Wardian's per-class and per-agent scoping without mutating the user's repository tree.

### 4. Session Identity

- OpenCode session IDs are discovered from JSON event output rather than assigned up front.
- Wardian extracts `sessionID` from `step_start` events during bootstrap and headless runs.
- Resume uses the discovered provider session ID directly through `--session`.

### 5. UI Surface

- OpenCode appears anywhere the provider can be selected for agent creation or inspection.
- Advanced settings expose an OpenCode-specific `agent` override field.
- OpenCode-specific session and skill behavior stays backend-owned; the UI only edits declarative config.

## Consequences

- **Positive**: Wardian can spawn and resume OpenCode-backed agents without a projected workspace model.
- **Positive**: OpenCode can consume Wardian class instructions and skills while still trusting the real repository path.
- **Positive**: Headless runs and workflow execution use the same provider-specific session and JSON parsing rules as interactive bootstrap.
- **Negative**: OpenCode telemetry enrichment is lighter than Codex or Claude today because its session metadata is not exposed as one stable per-session log file.
- **Negative**: Real interactive provider verification still depends on local OpenCode installation and auth outside CI.
