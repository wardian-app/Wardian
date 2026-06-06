# Codex Windows Sandbox Projection

* **Status:** Implemented
* **Date:** 2026-05-12

## Context and Problem Statement

Wardian creates per-agent Codex homes under `<wardian-home>/agents/<session-id>/habitat/.codex` and temporary bootstrap homes under `<wardian-home>/provider-bootstrap/codex/<workspace-key>/.codex`.

On Windows, Codex's elevated sandbox stores reusable support artifacts in the user's Codex home. When Wardian projects a fresh Codex home without those artifacts, Codex can treat the projected home as a new sandbox environment and run setup again. That setup may require Windows elevation and can rotate or regenerate credentials.

The fix must not share provider session state. Wardian still relies on per-agent Codex logs, `sessions/**`, `history.jsonl`, `session_index.jsonl`, and SQLite files for resume and telemetry boundaries.

## Decision

Wardian projects Windows sandbox support separately from Codex session state:

- Project `.sandbox-secrets` from the user's Codex home into each Wardian-created Codex home.
- Project `.sandbox-bin` from the user's Codex home into each Wardian-created Codex home.
- Copy `.sandbox/setup_marker.json` when present.
- Keep `.sandbox` itself local to the agent or bootstrap home.
- Keep `.sandbox/sandbox.log`, `.sandbox/setup_error.json`, Codex sessions, history, index files, logs, and SQLite state per-agent.

Bootstrap migration leaves `.sandbox`, `.sandbox-bin`, and `.sandbox-secrets` in the reusable bootstrap home instead of moving them into the final agent home. If a first-run bootstrap creates sandbox support before the user's Codex home has it, migration projects those bootstrap-generated support artifacts into the final agent home before moving session logs.

## Consequences

- **Positive:** Fresh Wardian Codex homes can observe existing Windows sandbox setup without sharing provider session state.
- **Positive:** Codex resume and Wardian log lookup still read from the agent habitat because sessions and indexes remain per-agent.
- **Positive:** Sandbox diagnostic files stay scoped to the home that produced them.
- **Negative:** The projection depends on Codex keeping the elevated sandbox credential/helper layout under `.sandbox-secrets` and `.sandbox-bin`.

## Verification

- Unit tests cover Windows sandbox projection and bootstrap migration.
- Native verification must include a Windows run that spawns Codex through Wardian and confirms sandbox support is projected without merging session logs or requiring a repeated sandbox setup.
