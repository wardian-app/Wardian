# Implementation Plan: Agent Status and Resume (v2)

This plan ensures that all AI providers (Claude, Codex, OpenCode) handle status tracking and session resumption correctly, following our newly implemented identity and SQLite state architecture.

## 1. Provider Alignment

### 1.1. Claude Code
- **Status:** Currently uses `claude_status_from_log`.
- **Resume:** Uses `--resume <id>`. 
- **Requirement:** Ensure that when Wardian restarts, it correctly passes the stored `resume_session` (which matches the Claude JSONL filename) to the `--resume` flag.
- **Action:** Update `providers/claude.rs` to strictly map `resume_session` to the `--resume` flag during restoration.

### 1.2. Codex
- **Status:** TUI-based. Uses `codex resume <ID>`.
- **Requirement:** Verify `manager::latest_codex_session_index_entry` correctly extracts the ID for resumption.
- **Action:** Add explicit `/status` parsing if needed, or ensure the TUI output for session IDs is correctly captured.

### 1.3. OpenCode
- **Status:** Uses `/status` in TUI. Uses `--session <ID>` for resume.
- **Requirement:** Ensure the extracted `ses_xxx` ID from logs is correctly passed to `--session`.
- **Action:** Refine `opencode_extract_created_session_id` in `manager.rs` to ensure it captures the ID even if the process was interrupted.

## 2. SQLite Integration

-   **Deduplication:** Ensure `update_agent_status` is called by all provider parsers.
-   **Forensic Verification:** Implement the check for `WARDIAN_SESSION_ID` in `reconcile_headless_agents` (Done in Phase 3, verify during testing).

## 3. UI Refinements

-   **Placeholder:** Update `SpawnAgentPanel.tsx` placeholder to `@handle` style (Done).
-   **Headless Indicator:** Ensure the UI clearly shows "Headless" status when an agent is recovered but not interactive.

## 4. Verification Tasks

1.  **Test Claude Resume:** Spawn Claude -> Kill app -> Restart -> Verify it resumes the same session ID.
2.  **Test OpenCode Headless:** Spawn OpenCode -> Force kill Wardian (keep agent alive) -> Restart Wardian -> Verify status is "Headless".
3.  **Test Duplicate Identity:** Try to create two agents with the same handle -> Verify error message.

## 5. Timeline & Phasing

- **Task 1:** Surgical provider flag alignment (Claude/OpenCode).
- **Task 2:** "Headless" UI state visibility.
- **Task 3:** Final E2E Smoke Test.
