# Agent Cloning Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add single-agent Fresh/Profile Clone actions to the agent context menu, backed by a Rust `clone_agent` command.

**Architecture:** The Rust backend owns clone semantics: config sanitization, unique naming, provider session creation, optional profile file copying, state insertion, and update events. The React context menu only exposes the action for one targeted agent and delegates clone mode to `App.tsx`, which invokes the Tauri command and refreshes the roster.

**Tech Stack:** Rust/Tauri commands and unit tests, React/TypeScript context menu wiring, Vitest/Testing Library for UI tests.

---

### Task 1: Backend Clone Helpers

**Files:**
- Modify: `src-tauri/src/commands/agent.rs`

- [ ] **Step 1: Write failing Rust helper tests**

Add tests for:
- `unique_clone_name("Alpha", ["Alpha"]) == "Alpha-copy"`
- `unique_clone_name("Alpha", ["Alpha", "Alpha-copy"]) == "Alpha-copy-2"`
- `sanitize_clone_config` clears `session_id`, `resume_session`, `fresh_provider_session_id`, `codex_cleared_provider_sessions`, and `is_off`, while preserving provider/model/class/folder settings.
- `copy_agent_profile_files` copies only `AGENTS.md` and `.agents/skills`, and excludes `habitat/` and `claude/permission-requests.jsonl`.

Run: `cd src-tauri && cargo test commands::agent::tests::clone_ -- --test-threads=1`
Expected: fail because helpers do not exist.

- [ ] **Step 2: Implement helpers**

Add:
- `CloneAgentMode`
- `CloneAgentRequest`
- `unique_clone_name`
- `sanitize_clone_config`
- `copy_agent_profile_files`
- focused recursive copy/link fallback helpers for profile skills.

- [ ] **Step 3: Verify helper tests pass**

Run: `cd src-tauri && cargo test commands::agent::tests::clone_ -- --test-threads=1`
Expected: pass.

### Task 2: Backend Command

**Files:**
- Modify: `src-tauri/src/commands/agent.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Refactor spawn insertion**

Extract the shared "register this prepared config as a new active agent" portion from `spawn_agent` into a private async helper so both normal spawn and clone use identical persistence/event behavior.

- [ ] **Step 2: Implement `clone_agent`**

Load the source config, generate a unique name, sanitize the clone config, create the new provider/Wardian session id using the same rules as normal spawn, copy profile files for `profile` mode before spawning, register the new agent, emit `agents-updated`, and return the new `AgentConfig`.

- [ ] **Step 3: Register command**

Add `commands::agent::clone_agent` to the Tauri invoke handler in `src-tauri/src/lib.rs`.

- [ ] **Step 4: Verify backend targeted tests**

Run: `cd src-tauri && cargo test commands::agent::tests::clone_ -- --test-threads=1`
Expected: pass.

### Task 3: Context Menu UI

**Files:**
- Modify: `src/components/AgentContextMenu.tsx`
- Modify: `src/layout/watchlist/AgentWatchlist.tsx`
- Modify: `src/views/GridView.tsx`
- Modify: `src/views/App.tsx`
- Modify: `src/layout/watchlist/AgentWatchlist.test.tsx`
- Modify: `src/views/App.test.tsx`

- [ ] **Step 1: Write failing UI tests**

Add tests that:
- A single-agent context menu shows `Clone`.
- Clicking parent `Clone` calls `onClone(agentId, "fresh")`.
- Hovering `Clone` exposes `Fresh Clone` and `Profile Clone`.
- `Profile Clone` calls `onClone(agentId, "profile")`.
- Bulk/team menus do not expose an active clone action.
- `App.tsx` invokes `clone_agent` with `{ req: { source_session_id, mode } }` and refreshes agents.

Run: `npm run test -- AgentWatchlist.test.tsx App.test.tsx`
Expected: fail because clone UI does not exist.

- [ ] **Step 2: Implement UI wiring**

Add `onClone?: (agentId, mode) => MaybePromise` through the context menu callers. Render the clone submenu only when `!isBulk && !isTeam && onClone`. Add the `App.tsx` handler that invokes `clone_agent` then `fetchAgents()`.

- [ ] **Step 3: Verify targeted frontend tests**

Run: `npm run test -- AgentWatchlist.test.tsx App.test.tsx`
Expected: pass.

### Task 4: Final Verification

**Files:**
- No new files expected beyond this plan and the existing spec.

- [ ] **Step 1: Run frontend checks**

Run: `npm run lint`
Expected: pass.

Run: `npm run test`
Expected: pass.

Run: `npm run build`
Expected: pass.

- [ ] **Step 2: Run backend checks**

Run: `cd src-tauri && cargo clippy`
Expected: pass.

Run: `cd src-tauri && cargo test`
Expected: pass.

Run: `cd src-tauri && cargo check`
Expected: pass.

- [ ] **Step 3: Inspect git state**

Run: `git status --short`
Expected: only intentional plan/spec/code/test files changed.

