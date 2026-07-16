# Provider Session Secret Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent untrusted provider initialization events or poisoned persisted state from replacing Wardian/provider session UUIDs with API keys, and remove resume identifiers from logs and launch diagnostics.

**Architecture:** Structured init events remain status/timestamp telemetry but lose authority to mutate `resume_session`; provider-specific filesystem/database discovery remains authoritative. A focused identity module detects values equal to credential-bearing environment variables, clears poisoned persisted resume state during preparation, and rejects unsafe interactive/headless launches before side effects.

**Tech Stack:** Rust, Tauri, `portable-pty`, serde, Node.js native E2E, PowerShell verification.

## Global Constraints

- Link implementation and PR work to GitHub issue #671.
- Never log, persist, display in errors, or pass to a provider any value identified as a credential.
- Do not rely on identifier shape alone; credential values can be UUID-shaped.
- Preserve Wardian's stable `session_id` and existing provider-specific trusted discovery paths.
- Keep provider event parsers intact; change only the authority granted to parsed init identifiers.
- Use an isolated `WARDIAN_HOME` for native tests and do not touch production Wardian state.
- Follow the full frontend/backend, documentation, secrets, and git-scope checks in `AGENTS.md`.

---

### Task 1: Credential-aware session identity boundary

**Files:**
- Create: `src-tauri/src/manager/session_identity.rs`
- Modify: `src-tauri/src/manager/mod.rs:1-25`

**Interfaces:**
- Produces: `clear_credential_resume_session(config: &mut AgentConfig) -> bool`.
- Produces: `validate_session_values_for_launch(wardian_session_id: &str, resume_session: Option<&str>) -> Result<(), String>`.
- Keeps credential matching pure in tests; no test mutates process-global environment variables.

- [ ] **Step 1: Write failing pure tests in the new module**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    #[test]
    fn uuid_shaped_api_key_is_a_credential_value() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        assert!(value_matches_credentials(
            secret,
            env(&[("OPENAI_API_KEY", secret)])
        ));
    }

    #[test]
    fn wardian_session_environment_is_not_a_credential() {
        let session = "00000000-0000-4000-8000-0000000000aa";
        assert!(!value_matches_credentials(
            session,
            env(&[("WARDIAN_SESSION_ID", session), ("TERM", session)])
        ));
    }

    #[test]
    fn unsafe_launch_error_does_not_echo_the_credential() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let error = validate_session_values_with_environment(
            "wardian-session",
            Some(secret),
            env(&[("ANTHROPIC_API_KEY", secret)]),
        )
        .expect_err("credential resume must fail closed");
        assert!(!error.contains(secret));
        assert!(error.contains("credential environment value"));
    }

    #[test]
    fn poisoned_resume_is_cleared_without_changing_wardian_uuid() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let mut config = AgentConfig {
            session_id: "wardian-session".into(),
            resume_session: Some(secret.into()),
            ..Default::default()
        };
        assert!(clear_credential_resume_session_with_environment(
            &mut config,
            env(&[("GEMINI_API_KEY", secret)])
        ));
        assert_eq!(config.session_id, "wardian-session");
        assert_eq!(config.resume_session, None);
    }
}
```

- [ ] **Step 2: Verify RED**

```powershell
cd src-tauri
cargo test manager::session_identity -- --test-threads=1
```

Expected: compilation fails because the module and functions do not exist.

- [ ] **Step 3: Implement the identity boundary and re-export it**

Create `session_identity.rs`:

```rust
use std::ffi::{OsStr, OsString};
use wardian_core::models::AgentConfig;

fn credential_env_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    name == "API_KEY"
        || name.ends_with("_API_KEY")
        || name == "TOKEN"
        || name.ends_with("_TOKEN")
        || name == "SECRET"
        || name.ends_with("_SECRET")
        || name.contains("_SECRET_")
        || name == "PASSWORD"
        || name.ends_with("_PASSWORD")
}

fn value_matches_credentials(
    candidate: &str,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> bool {
    let candidate = candidate.trim();
    !candidate.is_empty()
        && environment.into_iter().any(|(name, value)| {
            credential_env_name(&name) && value.to_string_lossy() == candidate
        })
}

fn validate_session_values_with_environment(
    wardian_session_id: &str,
    resume_session: Option<&str>,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<(), String> {
    let environment = environment.into_iter().collect::<Vec<_>>();
    if value_matches_credentials(wardian_session_id, environment.clone())
        || resume_session
            .is_some_and(|value| value_matches_credentials(value, environment.clone()))
    {
        return Err(
            "Refusing provider launch because a session identifier matches a credential environment value."
                .to_string(),
        );
    }
    Ok(())
}

fn clear_credential_resume_session_with_environment(
    config: &mut AgentConfig,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> bool {
    let matches = config
        .resume_session
        .as_deref()
        .is_some_and(|value| value_matches_credentials(value, environment));
    if matches {
        config.resume_session = None;
    }
    matches
}

pub(crate) fn validate_session_values_for_launch(
    wardian_session_id: &str,
    resume_session: Option<&str>,
) -> Result<(), String> {
    validate_session_values_with_environment(
        wardian_session_id,
        resume_session,
        std::env::vars_os(),
    )
}

pub(crate) fn clear_credential_resume_session(config: &mut AgentConfig) -> bool {
    clear_credential_resume_session_with_environment(config, std::env::vars_os())
}
```

Add to `manager/mod.rs`:

```rust
pub(crate) mod session_identity;
pub(crate) use session_identity::{
    clear_credential_resume_session, validate_session_values_for_launch,
};
```

- [ ] **Step 4: Verify GREEN and commit**

```powershell
cd src-tauri
cargo test manager::session_identity -- --test-threads=1
cd ..
git add src-tauri/src/manager/session_identity.rs src-tauri/src/manager/mod.rs
git commit -m "fix(security): reject credential session identities"
```

Expected: focused tests pass; the commit contains only the identity boundary.

### Task 2: Native regression for harness-owned init identity

**Files:**
- Create: `e2e-native/tests/provider-session-secret-native.test.mjs`

**Interfaces:**
- Proves the real Tauri/PTY path processes structured mock output before assertions.
- Proves a harness init identifier cannot enter live state, persisted state, or Wardian debug logs.

- [ ] **Step 1: Create the isolated failing native test**

```javascript
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const SYNTHETIC_SECRET = "00000000-0000-4000-8000-0000000000aa";

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);
  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

async function waitForStructuredMockOutput(driver, sessionId) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const output = await invokeTauri(driver, "read_agent_pty", {
      sessionId,
      options: { max_bytes: 65536, peek: true },
    });
    if (String(output ?? "").includes("action_required")) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail("Timed out waiting for structured mock output");
}

test("provider init identifier cannot poison persisted resume state", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);

  const previousSession = process.env.WARDIAN_MOCK_SESSION_ID;
  process.env.WARDIAN_MOCK_SESSION_ID = SYNTHETIC_SECRET;
  let session;
  try {
    session = await startNativeSession(harness);
  } finally {
    if (previousSession === undefined) delete process.env.WARDIAN_MOCK_SESSION_ID;
    else process.env.WARDIAN_MOCK_SESSION_ID = previousSession;
  }
  t.after(async () => session.close());
  await waitForAppShell(session.driver, 20000);

  const agent = await invokeTauri(session.driver, "spawn_agent", {
    req: {
      sessionName: `SecretBoundary-${process.pid}-${Date.now()}`,
      agentClass: "TestClass",
      folder: harness.repoRoot,
      resumeSession: null,
      isOff: false,
      configOverride: {
        provider: "mock",
        provider_config: { type: "mock", scenario: "action_needed", delay_ms: 5 },
      },
    },
  });

  await waitForStructuredMockOutput(session.driver, agent.session_id);
  const agents = await invokeTauri(session.driver, "list_agents");
  const live = agents.find((candidate) => candidate.session_id === agent.session_id);
  assert.ok(live);
  assert.notEqual(live.resume_session, SYNTHETIC_SECRET);

  const state = fs.readFileSync(path.join(harness.isolatedHome, "settings", "state.json"), "utf8");
  const debug = fs.readFileSync(path.join(harness.isolatedHome, "wardian_debug.log"), "utf8");
  assert.equal(state.includes(SYNTHETIC_SECRET), false);
  assert.equal(debug.includes(SYNTHETIC_SECRET), false);
});
```

- [ ] **Step 2: Verify RED and retain the failing evidence**

```powershell
npm run test:e2e:native:fast -- e2e-native/tests/provider-session-secret-native.test.mjs
```

Expected: FAIL because current `spawn.rs` copies the synthetic identifier into `resume_session` and logs it.

### Task 3: Remove init authority and redact interactive launch logs

**Files:**
- Modify: `src-tauri/src/manager/spawn.rs:203-220,314-535,730-975`
- Modify: `src-tauri/src/manager/spawn_tests.rs`
- Modify: `src-tauri/src/commands/agent.rs:1980-2030`

**Interfaces:**
- Consumes: `validate_session_values_for_launch` from Task 1.
- Produces: init handling that records timestamps/status only.
- Preserves: Codex adoption through projected rollout/index discovery.

- [ ] **Step 1: Guard interactive spawn before side effects**

At the start of `manager::spawn_agent`, before provider resolution and `resolve_cwd`:

```rust
super::validate_session_values_for_launch(
    &config.session_id,
    config.resume_session.as_deref(),
)?;
```

- [ ] **Step 2: Centralize timestamp-only init handling**

Delete `capture_codex_init_resume_session`. Add:

```rust
pub(super) fn capture_init_timestamp(
    event: &AgentEvent,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
) {
    let AgentEvent::Init { timestamp, .. } = event else { return };
    let Some(timestamp) = timestamp else { return };
    let mut current = init_timestamp.lock().unwrap();
    if current.is_none() {
        *current = Some(timestamp.clone());
    }
}
```

Call it from the Claude line path, general line path, and streaming JSON path. Remove every init-triggered assignment to `config.resume_session`, init-triggered save, and raw init identifier log. Leave event parsing and status policy unchanged.

- [ ] **Step 3: Replace obsolete spawn tests**

```rust
use super::spawn::capture_init_timestamp;
use std::sync::{Arc, Mutex};
use wardian_core::models::AgentEvent;

#[test]
fn init_event_only_captures_timestamp() {
    let timestamp = Arc::new(Mutex::new(None));
    capture_init_timestamp(
        &AgentEvent::Init {
            session_id: "00000000-0000-4000-8000-0000000000aa".into(),
            timestamp: Some("2026-07-16T12:00:00Z".into()),
        },
        &timestamp,
    );
    assert_eq!(
        timestamp.lock().unwrap().as_deref(),
        Some("2026-07-16T12:00:00Z")
    );
}
```

- [ ] **Step 4: Replace raw interactive and Codex adoption logs**

```rust
log_debug(&format!(
    "[Wardian] PTY spawn: provider={} exe={} arg_count={} cwd={}",
    config.provider,
    launch_spec.executable,
    launch_spec.args.len(),
    provider_cwd.display()
));
log_debug(&format!(
    "[Wardian] Spawning {} agent. Session: {}, Resume: {}, Restored: {}",
    provider.name(),
    config.session_id,
    config.resume_session.as_deref().is_some_and(|value| !value.trim().is_empty()),
    is_restored
));
```

In `commands/agent.rs`:

```rust
manager::log_debug(&format!(
    "[WARDIAN] Adopted discovered Codex session for Wardian session {}",
    session_id
));
```

- [ ] **Step 5: Verify unit and native GREEN, then commit**

```powershell
cd src-tauri
cargo test manager::spawn_tests manager::session_identity -- --test-threads=1
cd ..
npm run test:e2e:native:fast -- e2e-native/tests/provider-session-secret-native.test.mjs
git add src-tauri/src/manager/spawn.rs src-tauri/src/manager/spawn_tests.rs src-tauri/src/commands/agent.rs e2e-native/tests/provider-session-secret-native.test.mjs
git commit -m "fix(runtime): distrust provider init session ids"
```

Expected: unit and native tests pass; the commit contains the regression and authority removal.

### Task 4: Recover poisoned state and harden headless/bootstrap paths

**Files:**
- Modify: `src-tauri/src/commands/agent.rs:1840-1950`
- Modify: `src-tauri/src/manager/headless.rs:258-390,650-800`
- Test: existing `src-tauri/src/commands/agent.rs` test module

**Interfaces:**
- Consumes: both Task 1 identity functions.
- Produces: persisted recovery before provider-specific fallback.
- Produces: headless/bootstrap failures that never echo credentials or provider stderr.

- [ ] **Step 1: Wire poisoned-state recovery before provider rules**

At the start of `prepare_resume_config`, immediately after `config.is_off = false`, insert:

```rust
if manager::clear_credential_resume_session(config) {
    manager::log_debug(
        "[WARDIAN] Cleared an unsafe resume identifier matching a credential environment value.",
    );
}
```

At the start of `prepare_restored_config_for_spawn`, clear a credential match before the `is_off` early return:

```rust
if config.is_off && manager::clear_credential_resume_session(config) {
    manager::log_debug(
        "[WARDIAN] Cleared an unsafe resume identifier matching a credential environment value.",
    );
}
if config.is_off {
    return Ok(());
}
prepare_resume_config(config)
```

This composes the Task 1 tested credential clear with the existing tested provider rules: Claude/Gemini fall back to their Wardian-assigned manual session UUID, Codex accepts only projected local evidence, OpenCode requires `ses_...`, and Antigravity does not fall back to the Wardian UUID.

- [ ] **Step 2: Run existing provider recovery tests after wiring**

```powershell
cd src-tauri
cargo test manual_session_provider_resume -- --test-threads=1
cargo test codex_resume_ -- --test-threads=1
cargo test opencode_resume_ -- --test-threads=1
```

Expected: existing provider fallback and evidence rules remain green after unsafe-state clearing is inserted.

- [ ] **Step 3: Guard and redact headless launch**

At the top of `run_headless_with_options`:

```rust
super::validate_session_values_for_launch(
    options.wardian_session_id,
    options.resume_session,
)?;
```

Replace raw argument logging:

```rust
log_debug(&format!(
    "[Wardian] run_headless launch: provider={} exe={} arg_count={} resume={}",
    provider_name,
    launch_spec.executable,
    launch_spec.args.len(),
    resume_session.is_some_and(|value| !value.trim().is_empty())
));
```

- [ ] **Step 4: Harden legacy bootstrap diagnostics and return path**

Before using/returning `session_id_res`, validate its candidate. Replace raw arguments, session ID, `Option<String>`, and stderr logs with:

```rust
log_debug(&format!(
    "[WARDIAN-DEBUG] obtain_session_id launch: exe={} arg_count={} cwd={}",
    launch_spec.executable,
    launch_spec.args.len(),
    command_cwd.display()
));
if !stderr_output.trim().is_empty() {
    log_debug(&format!(
        "[WARDIAN-DEBUG] obtain_session_id provider stderr bytes={}",
        stderr_output.len()
    ));
}
if let Some(candidate) = session_id_res.as_deref() {
    super::validate_session_values_for_launch(candidate, Some(candidate))?;
}
log_debug(&format!(
    "[WARDIAN-DEBUG] obtain_session_id completed: found_session_id={}",
    session_id_res.is_some()
));
```

Return a generic error instead of raw provider stderr when no session ID is found:

```rust
Err(format!(
    "Provider {} failed during session initialization.",
    provider_name
))
```

- [ ] **Step 5: Verify recovery/headless suites and commit**

```powershell
cd src-tauri
cargo test manager::session_identity -- --test-threads=1
cargo test manual_session_provider_resume -- --test-threads=1
cargo test codex_resume_ -- --test-threads=1
cargo test manager::headless -- --test-threads=1
cargo test commands::agent -- --test-threads=1
cd ..
git add src-tauri/src/commands/agent.rs src-tauri/src/manager/headless.rs
git commit -m "fix(runtime): recover poisoned resume state"
```

Expected: all focused tests pass; no error or log includes the synthetic value.

### Task 5: Documentation and full verification

**Files:**
- Modify: `docs/guide/provider-readiness.md`
- Already created: `docs/superpowers/specs/2026-07-16-provider-session-secret-boundary-design.md`
- Already created: `docs/superpowers/plans/2026-07-16-provider-session-secret-boundary.md`

**Interfaces:**
- Produces user-facing credential/session separation guidance.
- Produces verification evidence for issue #671 and the eventual PR.

- [ ] **Step 1: Add credential/session safety guidance**

```markdown
### Credential and session identity safety

Wardian inherits the environment needed by provider CLIs, but provider API keys
are not Wardian session identifiers. Structured provider output cannot replace
Wardian's stable agent UUID or its trusted resume identity. If persisted resume
state matches a credential environment value, Wardian clears it before recovery
and refuses any remaining unsafe launch without printing the value.

Authenticate providers in a normal terminal and keep API keys in the provider's
documented credential mechanism. Do not paste an API key into Wardian's Resume
Session field.
```

- [ ] **Step 2: Run formatting and focused secret scans**

```powershell
cargo fmt --all -- --check
git diff --check
rg -n "Session ID mapped|Resume ID:|Found session_id:|Returning session_id:" src-tauri/src
rg -n "00000000-0000-4000-8000-0000000000aa" --glob '!docs/superpowers/**' --glob '!e2e-native/tests/provider-session-secret-native.test.mjs'
```

Expected: formatting/diff checks pass; obsolete logs are absent; the sentinel exists only in explicit test/design artifacts.

- [ ] **Step 3: Run the full backend checklist**

```powershell
cd src-tauri
cargo clippy --all-targets --all-features -- -D warnings
cargo test -- --test-threads=1
cargo check --all-targets --all-features
cd ..
```

Expected: all commands exit 0.

- [ ] **Step 4: Run the full frontend checklist**

```powershell
npm run lint
npm run test
npm run build
```

Expected: all commands exit 0. No screenshot is required because no frontend or visual behavior changes.

- [ ] **Step 5: Re-run native security verification**

```powershell
npm run test:e2e:native:fast -- e2e-native/tests/provider-session-secret-native.test.mjs
```

Expected: PASS with no synthetic identifier in state or Wardian debug logs.

- [ ] **Step 6: Commit docs and audit scope/secrets**

```powershell
git add docs/guide/provider-readiness.md docs/superpowers/plans/2026-07-16-provider-session-secret-boundary.md
git commit -m "docs: explain provider credential session safety"
git status --short --branch
git diff --name-status origin/main...HEAD
git log --oneline origin/main..HEAD
```

Expected: only issue #671 design/plan/docs, focused Rust changes, and the native regression are tracked; no `.env`, credential, temporary native home, or unrelated file is present.
