# Codex Computer Use Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable Computer Use only for Electrical Engineer and Mechanical Engineer Codex agents while preserving individual Codex state and ordinary approval controls.

**Architecture:** One Wardian-owned Codex policy resolver supplies the home reconciler, interactive launch flags, and CLI diagnostics. Each agent keeps its habitat `CODEX_HOME`; configuration is a marker-owned merge of a Wardian base and preserved local overlay.

**Tech Stack:** Rust, Tauri, `toml_edit`, `serde`, `clap`, Tokio control socket, native WebDriver E2E.

## Global Constraints

- Retain a distinct `CODEX_HOME` for every Wardian agent; never use one shared mutable Codex home.
- Preserve sessions, histories, SQLite files and sidecars, memories, goals, trust, and unowned agent/project configuration.
- Default-deny plugins. This slice allowlists only `computer-use@openai-bundled` for Electrical Engineer and Mechanical Engineer.
- Preserve Codex sandbox, approval prompts, Computer Use confirmations, and interrupt controls.
- An existing session must report `restart_required` after a managed policy/config change.

---

### Task 1: Define class-scoped Codex plugin policy

**Files:**
- Create: `src-tauri/src/utils/codex_policy.rs`
- Modify: `src-tauri/src/utils/mod.rs`
- Modify: `src-tauri/src/utils/shell.rs`
- Test: `src-tauri/src/utils/codex_policy.rs`

**Interfaces:**
- Consumes: `CodexRuntimePolicy` and `AgentConfig.agent_class`.
- Produces: `resolve_codex_plugin_policy(class_name, runtime) -> CodexPluginPolicy`, `launch_feature_disables()`, and `fingerprint()`.

- [ ] **Step 1: Write failing resolver tests**

```rust
#[test]
fn electrical_engineer_allows_computer_use_without_feature_disables() {
    let policy = resolve_codex_plugin_policy("Electrical Engineer", &CodexRuntimePolicy::default());
    assert_eq!(policy.allowed_plugins, vec![AllowedCodexPlugin::computer_use()]);
    assert!(policy.launch_feature_disables().is_empty());
}

#[test]
fn coder_is_default_deny() {
    let policy = resolve_codex_plugin_policy("Coder", &CodexRuntimePolicy::default());
    assert!(policy.allowed_plugins.is_empty());
    assert_eq!(policy.launch_feature_disables(), vec!["plugins", "apps"]);
}
```

- [ ] **Step 2: Run the tests and confirm failure**

Run: `cd src-tauri && cargo test codex_policy --lib`

Expected: FAIL because the module and resolver do not exist.

- [ ] **Step 3: Implement the policy contract**

```rust
pub const COMPUTER_USE_PLUGIN: &str = "computer-use@openai-bundled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedCodexPlugin {
    pub selector: String,
    pub requires_apps: bool,
}

pub fn resolve_codex_plugin_policy(class_name: &str, _runtime: &CodexRuntimePolicy) -> CodexPluginPolicy {
    let allowed_plugins = match class_name.trim() {
        "Electrical Engineer" | "Mechanical Engineer" => vec![AllowedCodexPlugin::computer_use()],
        _ => Vec::new(),
    };
    CodexPluginPolicy { allowed_plugins }
}
```

Implement `requires_apps`, `launch_feature_disables`, and deterministic `fingerprint`. Extend `CodexRuntimePolicyOverrides` with a serde-defaulted `class_plugin_allowlists: BTreeMap<String, Vec<AllowedCodexPlugin>>`, so policy is Wardian-owned and future explicit class overrides do not require a global home edit. Re-export the new module from `utils/mod.rs`.

- [ ] **Step 4: Verify and commit**

Run: `cd src-tauri && cargo fmt --check && cargo test codex_policy --lib`

Expected: PASS.

Commit: `git add src-tauri/src/utils/codex_policy.rs src-tauri/src/utils/mod.rs src-tauri/src/utils/shell.rs && git commit -m "feat(codex): add class-scoped plugin policy"`

### Task 2: Reconcile managed base and per-agent Codex home

**Files:**
- Modify: `src-tauri/src/utils/fs.rs`
- Modify: `src-tauri/src/manager/headless.rs`
- Modify: `src-tauri/src/manager/codex.rs`
- Test: `src-tauri/src/utils/fs.rs`
- Test: `src-tauri/src/manager/codex.rs`

**Interfaces:**
- Consumes: Wardian home, target agent home, workspace, skills, and `CodexPluginPolicy`.
- Produces: `reconcile_codex_agent_home(...) -> Result<CodexHomeReconciliation, String>` containing the fingerprint and plugin statuses.

- [ ] **Step 1: Write failing preservation tests**

```rust
#[test]
fn reconcile_merges_base_without_overwriting_agent_state() {
    std::fs::write(base.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
    std::fs::write(home.join("config.toml"), "[projects.\"/agent\"]\ntrust_level = \"trusted\"\n").unwrap();
    std::fs::write(home.join("history.jsonl"), "agent history").unwrap();
    std::fs::write(home.join("state_5.sqlite"), "agent sqlite").unwrap();
    reconcile_codex_agent_home(&base, &home, &skills, &policy, workspace).unwrap();
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("model = \"gpt-5\""));
    assert!(config.contains("trust_level = \"trusted\""));
    assert_eq!(std::fs::read_to_string(home.join("history.jsonl")).unwrap(), "agent history");
}
```

- [ ] **Step 2: Run the tests and confirm failure**

Run: `cd src-tauri && cargo test codex_home_projection --lib`

Expected: FAIL because the projector copies the user global `config.toml`.

- [ ] **Step 3: Implement marker-owned reconciliation**

Stop including `config.toml` in `CODEX_SHARED_HOME_FILES`; retain only authentication/bootstrap files (`auth.json`, `cap_sid`). Store the Wardian base at `<WARDIAN_HOME>/codex/base/config.toml`. Use `toml_edit::DocumentMut` to update only the `[wardian]` metadata, managed marketplace/MCP/plugin declarations, and policy fingerprint; preserve all other tables, especially `[projects]`.

For an allowlisted but missing plugin, run the discovered Codex binary with `CODEX_HOME=<target-agent-home>` and `plugin add computer-use@openai-bundled --json`. Parse only selector, state, version, and path. Never invoke installation for unallowlisted agents. Persist non-secret install failures in `wardian-plugin-status.json` for diagnostics. Run this reconciler for bootstrap homes and re-run it after bootstrap migration into the final home.

- [ ] **Step 4: Add isolation regression tests**

Assert that two reconciled homes keep distinct history, sessions, and SQLite files; do not allow hardlink identity with a global state database. Replace the current stale-config test with one that proves absent base input does not delete an existing agent overlay.

- [ ] **Step 5: Verify and commit**

Run: `cd src-tauri && cargo test codex_home --lib && cargo test migrate_codex_bootstrap_home --lib`

Expected: PASS.

Commit: `git add src-tauri/src/utils/fs.rs src-tauri/src/manager/headless.rs src-tauri/src/manager/codex.rs && git commit -m "fix(codex): reconcile managed home policy per agent"`

### Task 3: Derive interactive launch flags from the policy

**Files:**
- Modify: `src-tauri/src/providers/codex.rs`
- Modify: `src-tauri/src/manager/spawn.rs`
- Modify: `src-tauri/src/commands/agent.rs`
- Test: `src-tauri/src/providers/codex.rs`
- Test: `src-tauri/src/commands/agent.rs`

**Interfaces:**
- Consumes: `resolve_codex_plugin_policy(&config.agent_class, &runtime_policy)`.
- Produces: provider args containing `--no-alt-screen`, ordinary sandbox/approval flags, and only the feature disables returned by the resolved policy.

- [ ] **Step 1: Write failing launch-argument tests**

```rust
#[test]
fn electrical_engineer_does_not_disable_plugins_or_apps() {
    let args = provider.get_spawn_args(&AgentConfig {
        provider: "codex".into(),
        agent_class: "Electrical Engineer".into(),
        ..Default::default()
    }, false);
    assert!(!args.windows(2).any(|pair| pair == ["--disable", "plugins"]));
    assert!(!args.windows(2).any(|pair| pair == ["--disable", "apps"]));
    assert!(args.contains(&"--ask-for-approval".to_string()));
}

#[test]
fn coder_keeps_both_feature_disables() {
    let args = provider.get_spawn_args(&AgentConfig { provider: "codex".into(), agent_class: "Coder".into(), ..Default::default() }, false);
    assert!(args.windows(2).any(|pair| pair == ["--disable", "plugins"]));
    assert!(args.windows(2).any(|pair| pair == ["--disable", "apps"]));
}
```

- [ ] **Step 2: Run the tests and confirm failure**

Run: `cd src-tauri && cargo test spawn_args --lib`

Expected: FAIL because the Codex provider always appends both disables.

- [ ] **Step 3: Replace unconditional disables**

```rust
let plugin_policy = resolve_codex_plugin_policy(&config.agent_class, runtime_policy);
for feature in plugin_policy.launch_feature_disables() {
    args.push("--disable".into());
    args.push(feature.into());
}
```

After successful Codex interactive spawn, record the policy fingerprint that was used. Ensure `build_agent_cli_command_with_shells` invokes the same provider argument builder so copied external commands match embedded launches. Do not change existing `--sandbox`, `--ask-for-approval`, `--no-alt-screen`, or full-auto behavior.

- [ ] **Step 4: Verify and commit**

Run: `cd src-tauri && cargo test spawn_args --lib && cargo test full_agent_command_builds_copyable_resume_for_each_non_gemini_provider --lib`

Expected: PASS; Coder retains both disables and Electrical Engineer has neither.

Commit: `git add src-tauri/src/providers/codex.rs src-tauri/src/manager/spawn.rs src-tauri/src/commands/agent.rs && git commit -m "fix(codex): honor class plugin policy at launch"`

### Task 4: Add `wardian agent doctor` diagnostics

**Files:**
- Modify: `crates/wardian-core/src/control.rs`
- Modify: `src-tauri/src/control.rs`
- Modify: `crates/wardian-cli/src/args.rs`
- Modify: `crates/wardian-cli/src/live.rs`
- Modify: `crates/wardian-cli/src/main.rs`
- Test: `crates/wardian-core/src/control.rs`
- Test: `crates/wardian-cli/src/args.rs`
- Test: `crates/wardian-cli/tests/agent_cli.rs`

**Interfaces:**
- Consumes: `ControlRequest::AgentDoctor { target }`, live `AgentConfig`, and a non-secret `codex_agent_diagnostic(&AgentConfig)` helper.
- Produces: `AgentDoctorResponse { schema, agent, applicable, codex_home, allowed_plugins, plugins, launch_flags, restart_required, reasons }`.

- [ ] **Step 1: Write failing command and control tests**

```rust
#[test]
fn parses_agent_doctor_target() {
    let cli = Cli::try_parse_from(["wardian", "agent", "doctor", "ee-1"]).unwrap();
    assert!(matches!(cli.command, Command::Agent(AgentArgs { command: Some(AgentCommand::Doctor { target }), .. }) if target == "ee-1"));
}

#[test]
fn agent_doctor_request_serializes_target() {
    let json = serde_json::to_string(&ControlRequest::AgentDoctor { target: "ee-1".into() }).unwrap();
    assert!(json.contains(r#"\"command\":\"agent_doctor\""#));
}
```

- [ ] **Step 2: Run the tests and confirm failure**

Run: `cargo test -p wardian-core agent_doctor && cargo test -p wardian-cli parses_agent_doctor_target`

Expected: FAIL because no diagnostic request or CLI subcommand exists.

- [ ] **Step 3: Implement the read-only diagnostic route**

Add `AgentDoctor` to the core request schema with `AgentDoctorResponse`, `CodexPluginDiagnostic`, and `CodexDiagnosticReason`. The Tauri dispatcher resolves exactly one agent, rejects broad selectors, and reads only the agent habitat marker and plugin-status data. Emit exact reason values: `not_applicable`, `not_allowlisted`, `not_installed`, `plugins_feature_disabled`, `apps_feature_disabled`, `installer_failed`, and `restart_required`.

Add `AgentCommand::Doctor { target }`, `live::agent_doctor`, and `handle_agent_doctor`. Return formatted JSON, never raw config/auth/session content.

- [ ] **Step 4: Add response behavior tests**

Make a mock control endpoint in `crates/wardian-cli/tests/agent_cli.rs` assert that `wardian agent doctor coder-a1` sends `{"command":"agent_doctor","target":"coder-a1"}`. Add control tests for a Coder `not_allowlisted` response and an Electrical Engineer response whose different active/current fingerprints set `restart_required`.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p wardian-core agent_doctor && cargo test -p wardian-cli agent_doctor`

Expected: PASS.

Commit: `git add crates/wardian-core/src/control.rs src-tauri/src/control.rs crates/wardian-cli/src/args.rs crates/wardian-cli/src/live.rs crates/wardian-cli/src/main.rs crates/wardian-cli/tests/agent_cli.rs && git commit -m "feat(cli): diagnose effective Codex plugin policy"`

### Task 5: Document and prove the native provider boundary

**Files:**
- Modify: `docs/providers.md`
- Modify: `docs/developer/provider-runtimes.md`
- Create: `e2e-native/tests/codex-computer-use-native.test.mjs`
- Modify: `package.json` only if the native test runner needs explicit registration

**Interfaces:**
- Consumes: `WARDIAN_E2E_REAL_CODEX_COMPUTER_USE=1`, isolated `WARDIAN_HOME`, configured local Codex CLI, and the `agent doctor` response.
- Produces: opt-in native proof that an eligible class exposes Computer Use and Coder does not, without controlling an app.

- [ ] **Step 1: Write skipped-by-default native coverage**

```js
test('real Codex Computer Use policy is class-scoped', async ({ app }) => {
  test.skip(process.env.WARDIAN_E2E_REAL_CODEX_COMPUTER_USE !== '1',
    '@real-provider-only requires a local Codex Computer Use surface');
  const electrical = await spawnAgent(app, { provider: 'codex', className: 'Electrical Engineer' });
  const coder = await spawnAgent(app, { provider: 'codex', className: 'Coder' });
  const electricalDoctor = await runWardian(['agent', 'doctor', electrical.name]);
  const coderDoctor = await runWardian(['agent', 'doctor', coder.name]);
  assert.equal(electricalDoctor.plugins.find((item) => item.selector === 'computer-use@openai-bundled')?.installed, true);
  assert.equal(electricalDoctor.launch_flags.includes('--disable plugins'), false);
  assert.equal(coderDoctor.allowed_plugins.length, 0);
  assert.equal(coderDoctor.launch_flags.includes('--disable plugins'), true);
});
```

Then send the eligible session a harmless capability prompt that requests only the Computer Use skill description or `list_apps` availability. Assert the transcript contains no `launch_app`, `click`, `type_text`, `press_key`, or other app-control method.

- [ ] **Step 2: Verify default skip behavior**

Run: `npm run test:e2e:native:fast -- e2e-native/tests/codex-computer-use-native.test.mjs`

Expected: PASS with the real-provider body skipped unless the opt-in variable is set.

- [ ] **Step 3: Update cross-platform provider documentation**

Document class-scoped default-deny plugins, habitat-state preservation, conditional feature disables, restart-required semantics, and the diagnostic command.

```bash
wardian agent doctor <agent-name-or-uuid>
```

```powershell
wardian agent doctor <agent-name-or-uuid>
```

Explain that a new session is required after a policy change and diagnostics intentionally exclude credentials and session data.

- [ ] **Step 4: Run final verification**

Run:

```bash
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo check
cd .. && npm run lint && npm run test
npm run test:e2e:native:fast -- e2e-native/tests/codex-computer-use-native.test.mjs
```

PowerShell:

```powershell
Push-Location src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo check
Pop-Location
npm run lint
npm run test
npm run test:e2e:native:fast -- e2e-native/tests/codex-computer-use-native.test.mjs
```

Expected: all Rust/frontend checks pass; the native Computer Use test is safely skipped without explicit real-provider opt-in.

- [ ] **Step 5: Commit docs and native coverage**

Commit: `git add docs/providers.md docs/developer/provider-runtimes.md e2e-native/tests/codex-computer-use-native.test.mjs package.json && git commit -m "test(codex): cover scoped Computer Use capability"`

## Plan Self-Review

- Spec coverage: Tasks 1-3 implement default-deny policy, isolated base/overlay reconciliation, agent-local installation, conditional launch flags, and restart fingerprints. Task 4 provides the requested diagnostic surface. Task 5 documents behavior and adds the opt-in harmless capability test.
- Placeholder scan: each task names its files, interfaces, test, expected result, implementation direction, and commit.
- Type consistency: `CodexPluginPolicy` feeds reconciliation, launch arguments, fingerprints, and `AgentDoctorResponse`; `ControlRequest::AgentDoctor` is consistently named in the core schema, Tauri dispatcher, and CLI.
