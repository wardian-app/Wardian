# Provider Session Provenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Wardian agent UUIDs separate from exact provider conversation IDs, reject secret-valued or conflicting identity evidence, and remove every heuristic resume fallback.

**Architecture:** A provider-specific identity policy validates typed evidence before it can mutate `resume_session`. Claude and Gemini use Wardian-supplied provider UUIDs; Codex and OpenCode bootstrap through their machine-readable APIs and bind the exact returned ID; Antigravity accepts only a workspace mapping proven to have changed during the current bootstrap. Resume, restore, clone, and clear fail when the exact provider ID is unavailable.

**Tech Stack:** Rust, Tauri, `portable-pty`, serde JSON, provider JSONL output, Node.js native E2E.

## Global Constraints

- `session_id` remains a Wardian-owned UUID and is never replaced by a provider ID.
- `resume_session` changes only from the provider-specific authoritative source defined in the approved design.
- Never use filesystem recency, `latest`, `--continue`, the Wardian UUID, or another session's metadata as a resume fallback.
- Missing, malformed, secret-valued, or conflicting identity fails without mutating persisted state.
- Errors and debug logs never include candidate identifiers, raw provider arguments, stderr, or credentials.
- Tests precede production changes and each focused test is observed failing for the intended reason.
- Native tests use an isolated `WARDIAN_HOME`; real-provider checks remain opt-in.

---

### Task 1: Provider-specific identity policy

**Files:**
- Modify: `src-tauri/src/manager/session_identity.rs`
- Modify: `src-tauri/src/manager/mod.rs`

**Interfaces:**
- Produces `ProviderIdentityOutcome::{Confirmed, Captured}`.
- Produces `apply_provider_identity_with_environment(provider, config, candidate, environment) -> Result<ProviderIdentityOutcome, String>`.
- Produces `apply_provider_identity(provider, config, candidate) -> Result<ProviderIdentityOutcome, String>`.
- Produces `validate_session_values_for_launch(wardian_session_id, resume_session) -> Result<(), String>`.

- [ ] **Step 1: Replace the clearing test with failing provenance tests**

Add tests proving:

```rust
#[test]
fn claude_init_confirms_but_cannot_replace_expected_id() {
    let mut config = config("claude", Some("expected"), None);
    assert_eq!(
        apply_provider_identity_with_environment("claude", &mut config, "expected", vec![]),
        Ok(ProviderIdentityOutcome::Confirmed),
    );
    assert!(apply_provider_identity_with_environment("claude", &mut config, "different", vec![]).is_err());
    assert_eq!(config.resume_session.as_deref(), Some("expected"));
}

#[test]
fn codex_fresh_captures_only_a_uuid() {
    let mut config = config("codex", None, None);
    let id = "019db2f3-22de-7861-8bc6-1b86db1686db";
    assert_eq!(
        apply_provider_identity_with_environment("codex", &mut config, id, vec![]),
        Ok(ProviderIdentityOutcome::Captured),
    );
    assert_eq!(config.resume_session.as_deref(), Some(id));
    let before = config.resume_session.clone();
    assert!(apply_provider_identity_with_environment("codex", &mut config, "not-a-uuid", vec![]).is_err());
    assert_eq!(config.resume_session, before);
}

#[test]
fn secret_candidate_is_rejected_without_mutation_or_echo() {
    let secret = "00000000-0000-4000-8000-0000000000aa";
    let mut config = config("codex", None, None);
    let error = apply_provider_identity_with_environment(
        "codex",
        &mut config,
        secret,
        env(&[("OPENAI_API_KEY", secret)]),
    ).expect_err("secret identity must fail");
    assert!(!error.contains(secret));
    assert_eq!(config.resume_session, None);
}
```

- [ ] **Step 2: Run `cargo test manager::session_identity -- --test-threads=1` and confirm the new tests fail because the policy API is absent.**
- [ ] **Step 3: Implement the minimal policy using exact equality for Claude/Gemini, UUID parsing for Codex, `ses_` validation for OpenCode, non-empty exact IDs for Antigravity, and no mutation before all checks pass. Remove `clear_credential_resume_session`.**
- [ ] **Step 4: Re-run the focused tests and commit `fix(security): enforce provider session provenance`.**

### Task 2: Exact bootstrap identity extraction

**Files:**
- Modify: `src-tauri/src/manager/headless.rs`
- Modify: `src-tauri/src/providers/antigravity.rs`
- Test: inline Rust test modules in both files

**Interfaces:**
- Codex consumes only parsed `thread.started.thread_id`.
- OpenCode consumes only the `sessionID` present in the current `opencode run --format json` output.
- Antigravity consumes `conversation_for_workspace` only when the post-run value differs from the pre-run value.

- [ ] **Step 1: Add failing tests for the pure extractors**

```rust
#[test]
fn opencode_bootstrap_requires_a_ses_id_from_current_output() {
    assert_eq!(bootstrap_output_session_id("opencode", r#"{"sessionID":"ses_exact"}"#), Some("ses_exact".into()));
    assert_eq!(bootstrap_output_session_id("opencode", r#"{"type":"text"}"#), None);
}

#[test]
fn antigravity_bootstrap_requires_changed_workspace_mapping() {
    assert_eq!(changed_workspace_conversation(Some("old"), Some("new")), Some("new".into()));
    assert_eq!(changed_workspace_conversation(Some("same"), Some("same")), None);
    assert_eq!(changed_workspace_conversation(None, None), None);
}
```

- [ ] **Step 2: Run the focused tests and confirm missing extractor failures.**
- [ ] **Step 3: Restore Codex/OpenCode parsing in `obtain_session_id`, add the Antigravity pre/post workspace baseline, validate the result through the identity policy, and return an error when the exact result is absent. Remove all `wardian_session_id` and `latest_conversation_id` substitutions from headless JSON results.**
- [ ] **Step 4: Run `cargo test manager::headless providers::antigravity -- --test-threads=1` and commit `fix(providers): bind exact bootstrap session ids`.**

### Task 3: Separate Wardian and provider identities in lifecycle commands

**Files:**
- Modify: `src-tauri/src/commands/agent.rs`
- Test: inline tests in `src-tauri/src/commands/agent.rs`

**Interfaces:**
- Fresh spawn/clone always creates a Wardian UUID first.
- Claude/Gemini create a distinct `fresh_provider_session_id` and persist it only after successful spawn.
- Codex/OpenCode/Antigravity call `obtain_session_id` before the interactive process and launch that process with the exact returned `resume_session`.
- Resume/restore requires an existing exact `resume_session`; `Fresh` explicitly starts a new provider conversation.

- [ ] **Step 1: Add failing lifecycle tests**

```rust
#[test]
fn resume_without_exact_provider_id_fails_without_mutation() {
    let mut config = AgentConfig { provider: "codex".into(), session_id: uuid().into(), resume_session: None, is_off: true, ..Default::default() };
    let before = config.clone();
    assert!(prepare_resume_config(&mut config).is_err());
    assert_eq!(config.resume_session, before.resume_session);
    assert_eq!(config.is_off, before.is_off);
}

#[test]
fn fresh_manual_provider_uses_a_distinct_provider_uuid() {
    let config = fresh_spawn_config("gemini", "wardian-uuid").expect("fresh config");
    assert_ne!(config.fresh_provider_session_id.as_deref(), Some(config.session_id.as_str()));
}
```

- [ ] **Step 2: Run the focused command tests and confirm they fail under the fallback behavior.**
- [ ] **Step 3: Refactor spawn and clone to construct the Wardian config before bootstrap, keep the Wardian UUID stable, and carry provider identity separately. Make clear bootstrap Codex/OpenCode/Antigravity and propagate errors instead of logging-and-continuing.**
- [ ] **Step 4: Delete Codex index adoption, OpenCode pause log capture, and every `resume_session = session_id` fallback. Run `cargo test commands::agent -- --test-threads=1` and commit `fix(agents): require exact provider resume identity`.**

### Task 4: Validate provider events without generic mutation

**Files:**
- Modify: `src-tauri/src/manager/spawn.rs`
- Modify: `src-tauri/src/manager/spawn_tests.rs`

**Interfaces:**
- `handle_provider_init_event(provider, event, config, timestamp) -> Result<ProviderIdentityOutcome, String>` validates identity and captures timestamps only after successful validation.
- A rejected event leaves configuration and timestamp unchanged and marks the runtime unavailable without logging the value.

- [ ] **Step 1: Add failing tests for matching confirmation, conflicting Claude/Gemini init, matching resumed Codex init, and malformed fresh Codex init.**
- [ ] **Step 2: Run `cargo test manager::spawn_tests -- --test-threads=1` and confirm failures reflect the missing handler.**
- [ ] **Step 3: Implement the handler once and call it from every PTY parsing path. Remove duplicated direct `resume_session` writes and raw-identifier logs.**
- [ ] **Step 4: Run focused spawn and provider parser tests and commit `fix(runtime): validate provider init identity`.**

### Task 5: Remove heuristic identity discovery and update evidence

**Files:**
- Modify: `src-tauri/src/manager/opencode.rs`
- Modify: `src-tauri/src/manager/telemetry.rs`
- Modify: `docs/guide/provider-readiness.md`
- Delete: `e2e-native/tests/provider-session-secret-native.test.mjs`

**Interfaces:**
- Telemetry may locate logs only from an already-bound exact provider ID.
- Telemetry never writes or substitutes `resume_session`.

- [ ] **Step 1: Add failing tests that unscoped OpenCode log extraction and Antigravity latest-directory lookup cannot produce identity.**
- [ ] **Step 2: Remove unscoped/recency identity helpers and their call sites; retain exact-ID log lookup only.**
- [ ] **Step 3: Replace the synthetic native test with provider-faithful unit fixtures and document explicit failure/remediation without claiming automatic recovery.**
- [ ] **Step 4: Run focused telemetry/OpenCode tests and commit `fix(providers): remove heuristic resume discovery`.**

### Task 6: Verification and review

**Files:**
- Verify all branch changes.

- [ ] **Step 1: Run `cargo fmt --check`, `cargo clippy`, `cargo test`, and `cargo check` in `src-tauri`.**
- [ ] **Step 2: Run `npm run lint`, `npm run test`, and `npm run build`.**
- [ ] **Step 3: Run targeted native tests that do not require real provider credentials, plus opt-in real-provider tests only when credentials and harness support are available.**
- [ ] **Step 4: Run `git diff --check`, inspect `git status`, search changed files for credential literals and raw argument logging, and verify branch scope against `origin/main`.**
- [ ] **Step 5: Perform a final code review against the approved spec and address findings before reporting completion.**
