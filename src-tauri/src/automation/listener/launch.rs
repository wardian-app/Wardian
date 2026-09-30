//! The launch path shared by every listener variant.
//!
//! A listener fire becomes an ordinary durable automation run. What the three
//! variants differ in is only *when* they fire and *what payload* they
//! contribute; everything after that — blueprint resolution, idempotent
//! claiming, provider and workspace resolution, overlap, and runtime
//! write-back — is this module's job, so a fix here reaches all three.

use crate::automation::runs;
use fs2::FileExt;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::engine::{
    store::{append_event, read_checkpoint, write_checkpoint},
    Event, EventKind, RunState, RunStatus,
};
use wardian_core::listeners::{
    self, AutomationListener, ListenerRejection, ListenerRuntime, OverlapPolicy,
};
use wardian_core::models::InvocationKind;

/// One listener fire, ready to become a run.
#[derive(Debug, Clone)]
pub struct ListenerFire {
    /// Stable identity of the *event*, not of the fire attempt. Two deliveries
    /// of the same webhook, or two replays of one debounced burst, share this
    /// value and therefore share a run.
    pub event_identity: String,
    /// Fields this variant contributes to `trigger.output`.
    pub payload: Map<String, Value>,
}

/// What happened to a fire, so callers can record it honestly instead of
/// assuming every fire produced a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    Started(String),
    /// The same event already has a run; a retry is not a second run.
    AlreadyClaimed,
    /// `skip` overlap, with a run already active. The event was deliberately
    /// dropped by policy, not lost.
    Skipped,
    /// `coalesce` overlap; this fire replaced any earlier pending one. It is
    /// held in memory only, so it is not durable.
    Coalesced,
    /// The rate ceiling tripped and the listener auto-disabled.
    RateLimited,
    /// The fire could not become a run: an unresolvable blueprint, a claim that
    /// could not be taken, or a run that could not be prepared. Kept distinct
    /// from `Skipped` so a caller cannot report a failure as an accepted event.
    Failed(String),
}

/// Deterministic run id for one listener event.
///
/// Deriving it from the event rather than generating a fresh id is what makes
/// a retried delivery idempotent: the second attempt resolves to a run
/// directory that already exists and is refused by the claim.
pub fn listener_run_id(listener_id: &str, event_identity: &str) -> String {
    let digest = Sha256::digest(format!("{listener_id}\0{event_identity}").as_bytes());
    format!("listener-{digest:x}")
}

/// Claim a deterministic run directory, or report that it is already taken.
///
/// Returns the held lock so the caller keeps exclusivity until the run's
/// durable state exists. A partially created directory from a crashed earlier
/// attempt is repaired rather than inherited.
pub fn claim_deterministic_run(
    run_root: &Path,
    run_id: &str,
    label: &str,
) -> Result<Option<std::fs::File>, String> {
    let parent = run_root
        .parent()
        .ok_or_else(|| format!("{label} run has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {label} run directory: {error}"))?;
    let claim_path = parent.join(format!(".{run_id}.claim.lock"));
    let claim = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&claim_path)
        .map_err(|error| format!("could not open {label} run claim: {error}"))?;
    FileExt::lock_exclusive(&claim)
        .map_err(|error| format!("could not lock {label} run claim: {error}"))?;
    if run_root.join("state.json").is_file() {
        return Ok(None);
    }
    if run_root.exists() {
        if run_root.file_name() != Some(std::ffi::OsStr::new(run_id)) {
            return Err(format!("refusing to repair an unexpected {label} run path"));
        }
        std::fs::remove_dir_all(run_root)
            .map_err(|error| format!("could not repair partial {label} run: {error}"))?;
    }
    std::fs::create_dir(run_root)
        .map_err(|error| format!("could not create {label} run: {error}"))?;
    Ok(Some(claim))
}

/// In-flight accounting behind the overlap policy.
///
/// Overlap is decided here rather than per variant so `skip` means the same
/// thing for a file burst and a poll change.
#[derive(Default)]
pub struct ListenerActivity {
    active: HashMap<String, usize>,
    pending: HashMap<String, ListenerFire>,
}

/// Holds one in-flight slot and releases it when dropped.
///
/// The release has to survive an aborted or panicking run task. Releasing only
/// on the success path leaks the slot forever, which permanently wedges a
/// `skip` listener and strands a `coalesce` listener's pending fire.
pub struct SlotGuard {
    app: AppHandle,
    listener_id: String,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let app = self.app.clone();
        let listener_id = std::mem::take(&mut self.listener_id);
        // Drop cannot await, so the release runs as its own task. Unwinding
        // from a panic and an abort-driven drop both reach here.
        tauri::async_runtime::spawn(async move {
            release(&app, &listener_id).await;
        });
    }
}

impl ListenerActivity {
    fn is_active(&self, listener_id: &str) -> bool {
        self.active.get(listener_id).copied().unwrap_or(0) > 0
    }

    fn enter(&mut self, listener_id: &str) {
        *self.active.entry(listener_id.to_string()).or_insert(0) += 1;
    }

    /// Release one in-flight run and hand back a coalesced fire if this was
    /// the last one.
    fn leave(&mut self, listener_id: &str) -> Option<ListenerFire> {
        if let Some(count) = self.active.get_mut(listener_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.active.remove(listener_id);
                return self.pending.remove(listener_id);
            }
        }
        None
    }
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn resolve_provider(listener: &AutomationListener) -> String {
    listener.provider.clone().unwrap_or_else(|| {
        crate::utils::load_shell_settings()
            .map(|settings| settings.default_provider)
            .unwrap_or_else(|_| "codex".to_string())
    })
}

/// Merge the listener's configured input with the event payload.
///
/// The event wins on conflict: configured input is a default the author chose
/// in advance, and the payload is what actually happened.
fn merge_input(configured: &Value, payload: &Map<String, Value>) -> Value {
    let mut merged = match configured {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    for (key, value) in payload {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

/// Apply an app-owned mutation to one listener's runtime and persist it.
///
/// The whole read-modify-write is serialized in core, and only `runtime` is
/// touched, so a concurrent CLI config edit survives.
pub fn write_runtime(listener_id: &str, mutate: impl FnOnce(&mut ListenerRuntime)) {
    let result = listeners::mutate_listeners(|stored| {
        if let Some(listener) = stored
            .iter_mut()
            .find(|listener| listener.id == listener_id)
        {
            mutate(&mut listener.runtime);
        }
        Ok(())
    });
    if let Err(error) = result {
        crate::utils::logging::log_debug(&format!(
            "[automation] listener runtime write failed for {listener_id}: {error}"
        ));
    }
}

/// Record why a listener refused an event, so an unfiring listener is
/// diagnosable rather than merely quiet.
pub fn record_rejection(listener_id: &str, reason: String) {
    crate::utils::logging::log_debug(&format!(
        "[automation] listener {listener_id} rejected an event: {reason}"
    ));
    write_runtime(listener_id, |runtime| {
        runtime.last_rejection = Some(ListenerRejection {
            reason,
            at_epoch_ms: now_ms(),
        });
    });
}

pub fn emit_listeners_updated(app: &AppHandle) {
    let _ = app.emit("listeners-updated", ());
}

/// Fire a listener, producing a durable run unless overlap, idempotency, or
/// the rate ceiling says otherwise.
pub async fn fire(app: AppHandle, listener: AutomationListener, fire: ListenerFire) -> FireOutcome {
    let supervisor = app.state::<super::ListenerSupervisor>();

    // Overlap is resolved before any durable work so a skipped fire costs
    // nothing but a lock acquisition.
    {
        let mut activity = supervisor.activity.lock().await;
        if activity.is_active(&listener.id) {
            match listener.effective_overlap() {
                OverlapPolicy::Skip => {
                    crate::utils::logging::log_debug(&format!(
                        "[automation] listener {} skipped a fire; a run is already active",
                        listener.id
                    ));
                    return FireOutcome::Skipped;
                }
                OverlapPolicy::Coalesce => {
                    activity.pending.insert(listener.id.clone(), fire);
                    return FireOutcome::Coalesced;
                }
                OverlapPolicy::Parallel => {}
            }
        }
        activity.enter(&listener.id);
    }
    let guard = SlotGuard {
        app: app.clone(),
        listener_id: listener.id.clone(),
    };

    let outcome = launch(&app, &listener, &fire, guard).await;
    emit_listeners_updated(&app);
    outcome
}

/// Release one in-flight slot and start any fire that coalesced behind it.
pub async fn release(app: &AppHandle, listener_id: &str) {
    let supervisor = app.state::<super::ListenerSupervisor>();
    let pending = {
        let mut activity = supervisor.activity.lock().await;
        activity.leave(listener_id)
    };
    let Some(pending) = pending else {
        return;
    };
    let Some(listener) = listeners::load_listeners()
        .into_iter()
        .find(|listener| listener.id == listener_id)
    else {
        return;
    };
    spawn_fire(app.clone(), listener, pending);
}

/// Spawn a fire through an erased future.
///
/// `fire` releases its slot and `release` starts the fire that coalesced
/// behind it, so the two are mutually recursive. Erasing the future here
/// breaks the type cycle, and spawning keeps a chain of coalesced fires from
/// growing the stack or holding a completion handler open.
fn spawn_fire(app: AppHandle, listener: AutomationListener, pending: ListenerFire) {
    let future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            self::fire(app, listener, pending).await;
        });
    tauri::async_runtime::spawn(future);
}

/// Take `guard` by value so every early return drops it and frees the slot,
/// and so the started-run path can hand it to the task that owns the run.
async fn launch(
    app: &AppHandle,
    listener: &AutomationListener,
    fire: &ListenerFire,
    guard: SlotGuard,
) -> FireOutcome {
    let now = now_ms();

    // The ceiling is evaluated against durable state, so a loop that survives
    // a restart is still caught.
    let mut within_ceiling = true;
    write_runtime(&listener.id, |runtime| {
        within_ceiling = listeners::record_fire_within_ceiling(runtime, now);
    });
    if !within_ceiling {
        crate::utils::logging::log_debug(&format!(
            "[automation] listener {} auto-disabled by the rate ceiling",
            listener.id
        ));
        return FireOutcome::RateLimited;
    }

    let resolved = match resolve(listener, fire) {
        Ok(resolved) => resolved,
        Err(message) => {
            mark_error(&listener.id, &message);
            match record_launch_failure(listener, fire, &message) {
                Ok(run_root) => {
                    runs::emit_automation_inbox_update_with_name(app, &listener.name, &run_root)
                }
                Err(error) => crate::utils::logging::log_debug(&format!(
                    "[automation] listener could not write failed run artifact: {error}"
                )),
            }
            return FireOutcome::Failed(message);
        }
    };

    let claim = match claim_deterministic_run(&resolved.run_root, &resolved.run_id, "listener") {
        Ok(Some(claim)) => claim,
        Ok(None) => {
            crate::utils::logging::log_debug(&format!(
                "[automation] listener {} already has a run for this event",
                listener.id
            ));
            return FireOutcome::AlreadyClaimed;
        }
        Err(error) => {
            mark_error(&listener.id, &error);
            return FireOutcome::Failed(error);
        }
    };

    let state = app.state::<crate::state::AppState>();
    let agent_catalog = runs::agent_catalog_from_state_with_assignments(
        &state,
        &resolved.bindings,
        &resolved.assignments,
        &resolved.workspace,
        &resolved.provider,
    )
    .await;

    let run_state = match runs::prepare_new_listener_run(
        runs::ListenerRunSetup {
            blueprint: &resolved.blueprint,
            run_id: &resolved.run_id,
            run_root: &resolved.run_root,
            workspace: &resolved.workspace,
            default_provider: &resolved.provider,
            bindings: &resolved.bindings,
            assignments: &resolved.assignments,
            listener_id: &listener.id,
        },
        resolved.input.clone(),
    ) {
        Ok(run_state) => run_state,
        Err(error) => {
            drop(claim);
            mark_error(&listener.id, &error);
            return FireOutcome::Failed(error);
        }
    };
    drop(claim);

    crate::utils::logging::log_debug(&format!(
        "[automation] listener '{}' firing -> blueprint {} run {}",
        listener.name, resolved.blueprint.id, resolved.run_id
    ));

    let run_id = resolved.run_id.clone();
    let listener_id = listener.id.clone();
    let run_root = resolved.run_root.clone();
    let blueprint_for_inbox = resolved.blueprint.clone();
    let app_for_run = app.clone();
    let app_for_emit = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = runs::drive_started_run_with_catalog_and_assignments(
            Some(app_for_run),
            resolved.blueprint,
            run_state,
            resolved.run_root,
            resolved.workspace,
            resolved.provider,
            resolved.bindings,
            resolved.assignments,
            agent_catalog,
        )
        .await;
        match result {
            Ok(()) => mark_from_checkpoint(&listener_id, &run_root),
            Err(error) => {
                crate::utils::logging::log_debug(&format!(
                    "[automation] listener run failed: {error}"
                ));
                mark_error(&listener_id, &error);
            }
        }
        runs::emit_automation_inbox_update(&app_for_emit, &blueprint_for_inbox, &run_root);
        // Dropping the guard here releases the slot and starts any coalesced
        // fire, on the success path and on an abort or panic alike.
        drop(guard);
        emit_listeners_updated(&app_for_emit);
    });

    FireOutcome::Started(run_id)
}

#[derive(Debug)]
struct ResolvedFire {
    blueprint: wardian_core::automation::Blueprint,
    run_id: String,
    run_root: PathBuf,
    provider: String,
    workspace: PathBuf,
    input: Value,
    bindings: HashMap<String, String>,
    assignments: wardian_core::models::AutomationAssignments,
}

fn resolve(listener: &AutomationListener, fire: &ListenerFire) -> Result<ResolvedFire, String> {
    let path = wardian_core::automation::resolve_blueprint_path(&listener.blueprint_id)
        .ok_or_else(|| {
            format!(
                "could not resolve blueprint path for {}",
                listener.blueprint_id
            )
        })?;
    let blueprint = wardian_core::automation::parse_file(&path)
        .map_err(|error| format!("parse failed: {error}"))?;
    let report = wardian_core::automation::validate(&blueprint);
    if !report.is_valid() {
        let diagnostics = serde_json::to_string(&report.diagnostics)
            .map_err(|error| format!("could not serialize validation diagnostics: {error}"))?;
        return Err(format!(
            "blueprint {} is invalid: {diagnostics}",
            listener.blueprint_id
        ));
    }
    let run_id = listener_run_id(&listener.id, &fire.event_identity);
    let run_root = wardian_core::paths::automation_run_dir(&blueprint.id, &run_id)
        .ok_or_else(|| "could not resolve run directory".to_string())?;
    let provider = resolve_provider(listener);
    let workspace = listener
        .workspace
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    Ok(ResolvedFire {
        blueprint,
        run_id,
        run_root,
        provider,
        workspace,
        input: merge_input(&listener.input, &fire.payload),
        bindings: listener.bindings.clone(),
        assignments: wardian_core::automation::assignment::normalize_assignments(
            Some(listener.assignments.clone()),
            &listener.bindings,
            InvocationKind::Listener,
        ),
    })
}

/// Persist a failed run artifact for a fire that could not launch.
///
/// Without this a broken blueprint makes a listener look silently inert, which
/// is the hardest listener failure to diagnose.
fn record_launch_failure(
    listener: &AutomationListener,
    fire: &ListenerFire,
    message: &str,
) -> Result<PathBuf, String> {
    let run_id = listener_run_id(&listener.id, &fire.event_identity);
    let run_root = wardian_core::paths::automation_run_dir(&listener.blueprint_id, &run_id)
        .ok_or_else(|| "could not resolve failed listener run directory".to_string())?;
    let provider = resolve_provider(listener);
    let workspace = listener
        .workspace
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| run_root.clone());
    let assignments = wardian_core::automation::assignment::normalize_assignments(
        Some(listener.assignments.clone()),
        &listener.bindings,
        InvocationKind::Listener,
    );

    runs::write_run_invocation_with_authority(
        &run_root,
        &provider,
        &workspace,
        &listener.bindings,
        &assignments,
        runs::InvokerAttribution::listener(&listener.id),
        None,
    )?;

    let event = Event::new(
        0,
        EventKind::RunFailed {
            error: message.to_string(),
        },
    );
    append_event(&run_root, &event).map_err(|error| error.to_string())?;

    let mut state = RunState::new(run_id, &listener.blueprint_id);
    state.status = RunStatus::Failed;
    state.failure = Some(message.to_string());
    state.next_seq = event.seq + 1;
    write_checkpoint(&run_root, &state).map_err(|error| error.to_string())?;
    Ok(run_root)
}

fn mark_error(listener_id: &str, message: &str) {
    crate::utils::logging::log_debug(&format!("[automation] listener {listener_id}: {message}"));
    let message = message.to_string();
    write_runtime(listener_id, move |runtime| {
        runtime.last_run_status = Some("failed".to_string());
        runtime.last_run_error = Some(message);
    });
}

fn mark_from_checkpoint(listener_id: &str, run_root: &Path) {
    match read_checkpoint(run_root) {
        Ok(Some(state)) => {
            let (status, error) = match state.status {
                RunStatus::Completed => ("completed".to_string(), None),
                RunStatus::AwaitingApproval => ("awaiting_approval".to_string(), None),
                RunStatus::Running => ("running".to_string(), None),
                RunStatus::Failed => ("failed".to_string(), state.failure),
            };
            write_runtime(listener_id, move |runtime| {
                runtime.last_run_status = Some(status);
                runtime.last_run_error = error;
            });
        }
        Ok(None) => mark_error(listener_id, "run completed without a checkpoint"),
        Err(error) => mark_error(
            listener_id,
            &format!("could not read completed run checkpoint: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUEPRINT: &str = r#"---
schema: 2
id: listener-audit
name: Listener Audit
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
  - id: analyze
    type: task
    fields:
      agent: role:analyst
      prompt: Review {{trigger.output.paths}}
edges:
  - from: trigger
    to: analyze
---

# Listener Audit
"#;

    struct TestHome {
        _guard: tokio::sync::MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
        path: PathBuf,
    }

    impl TestHome {
        fn new() -> Self {
            let guard = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());
            let path = home.path().to_path_buf();
            Self {
                _guard: guard,
                _home: home,
                previous,
                path,
            }
        }

        fn seed_blueprint(&self, file_stem: &str, text: &str) {
            let dir = self.path.join("library").join("automations");
            std::fs::create_dir_all(&dir).expect("automations dir");
            std::fs::write(dir.join(format!("{file_stem}.md")), text).expect("blueprint");
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn listener(id: &str) -> AutomationListener {
        AutomationListener {
            id: id.into(),
            blueprint_id: "listener-audit".into(),
            name: "Listener Audit".into(),
            enabled: true,
            trigger: wardian_core::listeners::ListenerTrigger::FileWatch(
                wardian_core::listeners::FileWatchTrigger {
                    path: "/watched".into(),
                    recursive: true,
                    patterns: Vec::new(),
                    ignore: Vec::new(),
                    events: Vec::new(),
                    debounce_ms: 500,
                },
            ),
            provider: Some("mock".into()),
            workspace: None,
            input: serde_json::json!({}),
            bindings: HashMap::new(),
            assignments: Default::default(),
            overlap: None,
            runtime: Default::default(),
        }
    }

    fn sample_fire() -> ListenerFire {
        let mut payload = Map::new();
        payload.insert("paths".into(), serde_json::json!(["src/main.rs"]));
        ListenerFire {
            event_identity: "burst-1".into(),
            payload,
        }
    }

    #[test]
    fn resolve_finds_a_blueprint_nested_in_a_library_subfolder() {
        let home = TestHome::new();
        // Resolution is by frontmatter id, not filename, so the deliberate
        // mismatch proves the listener path uses the same resolver the manual
        // and scheduled paths do.
        let nested = home
            .path
            .join("library")
            .join("automations")
            .join("quality")
            .join("checks");
        std::fs::create_dir_all(&nested).expect("nested dir");
        std::fs::write(nested.join("unrelated-name.md"), BLUEPRINT).expect("blueprint");

        let resolved = resolve(&listener("l1"), &sample_fire()).expect("resolve");
        assert_eq!(resolved.blueprint.id, "listener-audit");
        assert_eq!(resolved.provider, "mock");
        assert_eq!(resolved.input["paths"], serde_json::json!(["src/main.rs"]));
    }

    #[test]
    fn resolve_normalizes_legacy_bindings_with_the_unattended_busy_policy() {
        let home = TestHome::new();
        home.seed_blueprint("listener-audit", BLUEPRINT);
        let mut subject = listener("l1");
        subject
            .bindings
            .insert("analyst".to_string(), "agent-123".to_string());

        let resolved = resolve(&subject, &sample_fire()).expect("resolve");
        assert_eq!(
            resolved.assignments.get("analyst"),
            Some(&wardian_core::models::AutomationRoleAssignment::Agent {
                agent_id: "agent-123".to_string(),
                conversation: wardian_core::models::AgentConversationMode::Current,
                // Nobody is watching a listener run, so a busy agent must be
                // skipped rather than reported to an absent operator.
                busy_policy: wardian_core::models::BusyPolicy::Skip,
            })
        );
    }

    #[test]
    fn a_missing_blueprint_is_an_error_rather_than_a_silent_no_op() {
        let _home = TestHome::new();
        let error = resolve(&listener("l1"), &sample_fire()).unwrap_err();
        assert!(
            error.contains("could not resolve blueprint path"),
            "{error}"
        );
    }

    #[test]
    fn an_invalid_blueprint_is_refused_at_fire_time() {
        let home = TestHome::new();
        home.seed_blueprint(
            "listener-audit",
            r#"---
schema: 2
id: listener-audit
name: Listener Audit
nodes:
  - id: trigger
    type: manual_trigger
    fields: {}
edges:
  - from: trigger
    to: missing-node
---

# Listener Audit
"#,
        );
        let error = resolve(&listener("l1"), &sample_fire()).unwrap_err();
        assert!(error.contains("is invalid"), "{error}");
    }

    #[test]
    fn a_launch_failure_leaves_a_visible_failed_run_instead_of_silence() {
        let home = TestHome::new();
        let subject = listener("l1");
        let fire = sample_fire();

        let run_root =
            record_launch_failure(&subject, &fire, "blueprint went missing").expect("artifact");

        let state = read_checkpoint(&run_root)
            .expect("checkpoint readable")
            .expect("checkpoint written");
        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(state.failure.as_deref(), Some("blueprint went missing"));

        let invocation = runs::read_run_invocation(&run_root)
            .expect("invocation readable")
            .expect("invocation written");
        assert_eq!(invocation.listener_id.as_deref(), Some("l1"));
        assert_eq!(
            invocation.schedule_id, None,
            "a listener run must not claim schedule attribution"
        );
        assert!(home.path.exists());
    }

    #[test]
    fn the_failure_artifact_reuses_the_event_run_id_so_a_retry_does_not_duplicate_it() {
        let _home = TestHome::new();
        let subject = listener("l1");
        let fire = sample_fire();

        let first = record_launch_failure(&subject, &fire, "boom").expect("artifact");
        let second = record_launch_failure(&subject, &fire, "boom").expect("artifact");
        assert_eq!(first, second);
    }

    #[test]
    fn one_event_maps_to_one_run_id_regardless_of_retries() {
        let first = listener_run_id("listener-a", "delivery-1");
        assert_eq!(first, listener_run_id("listener-a", "delivery-1"));
        assert_ne!(first, listener_run_id("listener-a", "delivery-2"));
        assert_ne!(first, listener_run_id("listener-b", "delivery-1"));
    }

    #[test]
    fn run_ids_are_safe_path_components() {
        let id = listener_run_id("listener-a", "../../escape");
        assert!(wardian_core::paths::is_safe_path_component(&id), "{id}");
    }

    #[test]
    fn the_event_payload_overrides_configured_input() {
        let configured = serde_json::json!({"repo": "wardian", "mode": "default"});
        let payload = serde_json::json!({"mode": "release"})
            .as_object()
            .cloned()
            .expect("object");
        let merged = merge_input(&configured, &payload);
        assert_eq!(merged["repo"], "wardian");
        assert_eq!(merged["mode"], "release");
    }

    #[test]
    fn a_non_object_configured_input_still_yields_the_payload() {
        let payload = serde_json::json!({"paths": ["a"]})
            .as_object()
            .cloned()
            .expect("object");
        let merged = merge_input(&Value::Null, &payload);
        assert_eq!(merged["paths"], serde_json::json!(["a"]));
    }

    #[test]
    fn a_claimed_run_is_refused_a_second_time_and_a_partial_one_is_repaired() {
        let temp = tempfile::tempdir().expect("temp run root");
        let run_id = listener_run_id("listener-a", "delivery-1");
        let run_root = temp.path().join(&run_id);
        std::fs::create_dir(&run_root).expect("partial run");
        std::fs::write(run_root.join("partial.txt"), "incomplete").expect("partial marker");

        let claim = claim_deterministic_run(&run_root, &run_id, "listener")
            .expect("repair")
            .expect("claimable");
        assert!(!run_root.join("partial.txt").exists());
        std::fs::write(run_root.join("state.json"), "{}").expect("durable state");
        drop(claim);

        assert!(claim_deterministic_run(&run_root, &run_id, "listener")
            .expect("recognize")
            .is_none());
    }

    #[test]
    fn overlap_accounting_releases_and_hands_back_a_coalesced_fire() {
        let mut activity = ListenerActivity::default();
        assert!(!activity.is_active("a"));
        activity.enter("a");
        assert!(activity.is_active("a"));

        activity.pending.insert(
            "a".to_string(),
            ListenerFire {
                event_identity: "second".into(),
                payload: Map::new(),
            },
        );
        let released = activity.leave("a").expect("pending fire is handed back");
        assert_eq!(released.event_identity, "second");
        assert!(!activity.is_active("a"));
        assert!(activity.leave("a").is_none());
    }

    #[test]
    fn a_slot_taken_for_a_run_is_recovered_when_the_run_task_never_completes() {
        // Models what `SlotGuard::drop` does: a task that is aborted or panics
        // after taking a slot must still release it. Leaking it here would
        // permanently wedge a `skip` listener and strand a `coalesce`
        // listener's pending fire for the lifetime of the app.
        let mut activity = ListenerActivity::default();
        activity.enter("a");
        activity.pending.insert(
            "a".to_string(),
            ListenerFire {
                event_identity: "queued-behind-the-lost-run".into(),
                payload: Map::new(),
            },
        );

        let recovered = activity
            .leave("a")
            .expect("the coalesced fire must be handed back, not stranded");
        assert_eq!(recovered.event_identity, "queued-behind-the-lost-run");
        assert!(
            !activity.is_active("a"),
            "the listener must be able to fire again"
        );
    }

    #[test]
    fn a_launch_failure_is_distinguishable_from_a_policy_skip() {
        // The webhook handler maps these to different status codes, so
        // collapsing them would make Wardian acknowledge a delivery that never
        // became a run.
        assert_ne!(
            FireOutcome::Failed("blueprint missing".into()),
            FireOutcome::Skipped
        );
        assert_ne!(FireOutcome::Coalesced, FireOutcome::Skipped);
    }

    #[test]
    fn a_parallel_listener_holds_one_slot_per_run() {
        let mut activity = ListenerActivity::default();
        activity.enter("a");
        activity.enter("a");
        assert!(activity.leave("a").is_none());
        assert!(
            activity.is_active("a"),
            "the second run must keep the listener active"
        );
        assert!(activity.leave("a").is_none());
        assert!(!activity.is_active("a"));
    }
}
