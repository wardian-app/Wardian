# Codex Plugin Pass-Through Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let every Wardian Codex agent use the plugins configured in its own isolated Codex home, without Wardian class allowlists, installs, or global plugin/app disable flags.

**Architecture:** Preserve the per-agent `CODEX_HOME` and non-destructive base-plus-overlay config merge. Remove the Computer Use policy manager entirely. `wardian agent doctor` becomes a generic, read-only Codex home diagnostic that invokes `plugin list --json` through the same provider executable resolution used for launch and reports that source-of-truth result.

**Tech Stack:** Rust, Tauri control socket, Codex provider executable resolution, `serde_json`, Vitest/native test harness.

## Global Constraints

- Do not share mutable `CODEX_HOME` state between agents.
- Do not install, remove, enable, disable, or class-filter plugins.
- Do not add global `--disable plugins` or `--disable apps` arguments.
- Treat a resumed Codex thread's tool list as fixed; diagnostics must state when a fresh session is required after a home config change.

---

### Task 1: Remove the policy gate from home reconciliation and launch

**Files:**
- Delete: `src-tauri/src/utils/codex_policy.rs`
- Modify: `src-tauri/src/utils/mod.rs`, `src-tauri/src/utils/fs.rs`, `src-tauri/src/providers/codex.rs`
- Test: unit tests in `src-tauri/src/utils/fs.rs` and `src-tauri/src/providers/codex.rs`

- [ ] Delete the class selector and automatic plugin install/status code.
- [ ] Keep `sync_codex_agent_home` and its non-destructive config merge.
- [ ] Ensure every Codex spawn omits global plugin/app disable flags.
- [ ] Test that Coder and Electrical Engineer receive identical plugin/app launch treatment.

### Task 2: Make the diagnostic inspect the actual agent home

**Files:**
- Modify: `crates/wardian-core/src/control.rs`, `src-tauri/src/control.rs`, `src-tauri/src/utils/fs.rs`
- Test: `crates/wardian-core/src/control.rs`, `src-tauri/src/utils/fs.rs`

- [ ] Replace policy-shaped plugin output with installed/enabled plugins returned by `codex plugin list --json` for the target home.
- [ ] Run that command through the provider-resolved executable and shell wrapping so Windows npm/PowerShell shims behave like normal Codex launches.
- [ ] Report an inspection failure without modifying plugin state; report a fresh-session requirement only for stale managed-home configuration.
- [ ] Test JSON parsing and diagnostic serialization without credentials or home contents.

### Task 3: Replace Computer Use-specific artifacts with generic coverage and docs

**Files:**
- Delete: `docs/specs/2026-07-19-codex-computer-use-policy.md`, `docs/superpowers/plans/2026-07-19-codex-computer-use-policy.md`, `e2e-native/tests/codex-computer-use-native.test.mjs`
- Modify: `docs/providers.md`, `docs/developer/provider-runtimes.md`

- [ ] Remove class-specific plugin policy claims and Computer Use acceptance language.
- [ ] Document plugin pass-through, agent-home isolation, and fresh-session behavior after a config change.
- [ ] Retain only generic launch/config-isolation tests.

### Task 4: Verify and update the pull request

- [ ] Run focused Rust tests, `cargo clippy -- -D warnings`, `cargo test --lib -- --test-threads=1`, `cargo check`, `npm run lint`, `npm run test`, and docs checks.
- [ ] Push the revision and update issue #677 and PR #678 to describe generic plugin pass-through rather than Computer Use policy.
