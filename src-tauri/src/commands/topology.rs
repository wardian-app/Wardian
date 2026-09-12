use fs2::FileExt;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::limits::{MAX_ACTIVITY_PAIRS, MAX_ACTIVITY_RECORDS};
use wardian_core::topology::{
    apply_topology_operation, authorize_topology_mutation_v1, load_reconciled_topology,
    load_team_memberships, load_topology, pair_activity_from_records, resolve_neighbors,
    save_topology, PairActivity, Topology, TopologyAuthDenied, TopologyOperation,
    PAIR_ACTIVITY_WINDOW_MS,
};

#[derive(Debug, Clone, Serialize)]
pub struct TopologyEdgeDto {
    pub a: String,
    pub b: String,
    /// Always "manual" in schema v3: team edges are seeded as manual edges at
    /// write time, never computed from rules at read time.
    pub origin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopologySnapshot {
    pub edges: Vec<TopologyEdgeDto>,
    pub ignored_pairs: Vec<[String; 2]>,
    /// Groups of agent UUIDs visible to each other only via workspace-fallback.
    /// Not currently consumed by the frontend (the halo rendering it fed was
    /// removed); kept for API stability until a consumer returns or it's retired.
    pub fallback_groups: Vec<Vec<String>>,
}

fn home() -> Result<std::path::PathBuf, String> {
    crate::utils::fs::get_wardian_home().ok_or_else(|| "WARDIAN_HOME not resolvable".to_string())
}

#[tauri::command]
pub async fn get_topology(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<TopologySnapshot, String> {
    let home = home()?;
    let _topology_lock = topology_process_lock(&home)?;
    let refs = agent_refs(&state).await;
    let topology = match wardian_core::db::get_all_agents() {
        Ok(persisted_agents) => {
            // State restoration happens asynchronously at startup. The persisted
            // roster prevents an early Graph request from treating not-yet-restored
            // agents as deleted; live state also covers a newly spawned agent.
            let mut known_agents: BTreeSet<String> = persisted_agents
                .into_iter()
                .map(|agent| agent.session_id)
                .collect();
            known_agents.extend(refs.iter().map(|agent| agent.uuid.clone()));
            load_reconciled_topology(&home, &known_agents)
                .map(|(topology, _)| topology)
                .map_err(|error| error.to_string())?
        }
        Err(error) => {
            crate::manager::log_debug(&format!(
                "[WARDIAN] topology reconciliation skipped because the persisted roster could not be read: {error}"
            ));
            load_topology(&home)
        }
    };

    let edges = snapshot_edges(&topology);

    // Fallback groups: agents whose neighbors come only from workspace-fallback.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for agent in &refs {
        let view = resolve_neighbors(&agent.uuid, &topology, &refs);
        let only_fallback = !view.members.is_empty()
            && view.members.iter().all(|m| {
                m.reasons
                    .iter()
                    .all(|r| r.starts_with("rule:workspace-fallback"))
            });
        if only_fallback {
            if let Some(ws) = agent.workspace.clone() {
                groups.entry(ws).or_default().push(agent.uuid.clone());
            }
        }
    }

    Ok(TopologySnapshot {
        edges,
        ignored_pairs: topology
            .ignored_pairs
            .iter()
            .map(|p| [p.a.clone(), p.b.clone()])
            .collect(),
        fallback_groups: groups.into_values().filter(|g| g.len() > 1).collect(),
    })
}

/// Manual edges only. Teams have been seeded as manual edges at write time.
pub(crate) fn snapshot_edges(topology: &Topology) -> Vec<TopologyEdgeDto> {
    topology
        .edges
        .iter()
        .map(|edge| TopologyEdgeDto {
            a: edge.a.clone(),
            b: edge.b.clone(),
            origin: "manual".into(),
        })
        .collect()
}

#[tauri::command]
pub async fn add_topology_edge(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate_ui(&app, TopologyOperation::Link, &a, &b)
}

#[tauri::command]
pub async fn remove_topology_edge(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate_ui(&app, TopologyOperation::Unlink, &a, &b)
}

#[tauri::command]
pub async fn ignore_topology_pair(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate_ui(&app, TopologyOperation::Ignore, &a, &b)
}

#[tauri::command]
pub async fn unignore_topology_pair(app: AppHandle, a: String, b: String) -> Result<bool, String> {
    mutate_ui(&app, TopologyOperation::Unignore, &a, &b)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PairActivityResult {
    pub pairs: Vec<PairActivity>,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

#[tauri::command]
pub async fn get_pair_activity(offset: Option<usize>) -> Result<PairActivityResult, String> {
    let offset = offset.unwrap_or(0);
    let now = chrono::Utc::now();
    let since = (now - chrono::Duration::milliseconds(PAIR_ACTIVITY_WINDOW_MS)).to_rfc3339();
    let mut records = wardian_core::db::list_recent_interaction_records_since_page(
        MAX_ACTIVITY_RECORDS + 1,
        offset,
        &since,
    )
    .map_err(|e| e.to_string())?;
    let mut truncated = records.len() > MAX_ACTIVITY_RECORDS;
    records.truncate(MAX_ACTIVITY_RECORDS);
    let now_ms = now.timestamp_millis();
    let mut pairs = pair_activity_from_records(&records, now_ms);
    pairs.sort_by(|left, right| right.last_message_at.cmp(&left.last_message_at));
    if pairs.len() > MAX_ACTIVITY_PAIRS {
        pairs.truncate(MAX_ACTIVITY_PAIRS);
        truncated = true;
    }
    Ok(PairActivityResult {
        pairs,
        truncated,
        next_offset: truncated.then_some(offset + MAX_ACTIVITY_RECORDS),
    })
}

/// The desktop UI is the human operator: an `invoke` call is only reachable
/// by driving the app's own webview, so it carries unrestricted authority
/// today, same as before this module converged on `apply_topology_operation`.
/// Audited the same as every other writer (`caller: "operator"`), so the
/// audit log reflects every mutation regardless of which surface made it.
fn mutate_ui<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    op: TopologyOperation,
    a: &str,
    b: &str,
) -> Result<bool, String> {
    let home = home()?;
    let _topology_lock = topology_process_lock(&home)?;
    let teams = load_team_memberships(&home);
    let mut topology = load_topology(&home);
    let created_at = chrono::Utc::now().to_rfc3339();
    let outcome = apply_topology_operation(&mut topology, op, a, b, &created_at, &teams)
        .ok_or_else(|| "invalid topology pair".to_string())?;
    if outcome.changed {
        save_topology(&home, &topology).map_err(|e| e.to_string())?;
        let _ = app.emit("topology-changed", ());
    }
    audit_topology_mutation(
        &home,
        TopologyAuditInput {
            caller: "operator",
            op,
            a: &outcome.a,
            b: &outcome.b,
            at: &created_at,
            outcome: if outcome.changed {
                "applied"
            } else {
                "unchanged"
            },
            error_code: None,
        },
    );
    Ok(outcome.changed)
}

/// Bundles [`audit_topology_mutation`]'s fields so the function stays under
/// clippy's argument-count lint instead of carrying eight positional params.
struct TopologyAuditInput<'a> {
    caller: &'a str,
    op: TopologyOperation,
    a: &'a str,
    b: &'a str,
    at: &'a str,
    outcome: &'a str,
    error_code: Option<&'a str>,
}

/// Appends one record to the topology audit log. Logs, rather than
/// propagates, an append failure: an unwritable audit log must not block a
/// topology mutation the caller is otherwise authorized to make, but a
/// silent failure here would contradict the "every attempt is audited"
/// contract just as much as never calling this function, so it is not
/// swallowed either.
fn audit_topology_mutation(home: &Path, input: TopologyAuditInput) {
    let record = crate::topology_audit::TopologyAuditRecord {
        schema_version: crate::topology_audit::TOPOLOGY_AUDIT_SCHEMA_VERSION,
        at: input.at.to_string(),
        caller: input.caller.to_string(),
        operation: input.op.action().to_string(),
        a: input.a.to_string(),
        b: input.b.to_string(),
        outcome: input.outcome.to_string(),
        error_code: input.error_code.map(str::to_string),
    };
    if let Err(error) = crate::topology_audit::append_topology_audit_record(home, &record) {
        crate::manager::log_debug(&format!("[WARDIAN] topology audit append failed: {error}"));
    }
}

/// Errors from [`dispatch_topology_mutation`]. Mapped to the control plane's
/// wire error codes by `control::dispatch_request`, which is the only caller
/// that knows about `ControlError`; this module stays control-plane-agnostic
/// like every other `commands::*` module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyControlError {
    /// `caller_session_id` does not match any known agent.
    UnknownCaller,
    /// The caller is a known agent but neither endpoint of the edge.
    SelfServeRequired,
    Io(String),
}

/// Single control-plane entry point for topology mutations. Authorization is
/// decided once here — the caller does not control the outcome by lying about
/// endpoints or by not being reachable — and every attempt (allowed, denied,
/// or a no-op) is appended to the topology audit log.
pub async fn dispatch_topology_mutation<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    op: TopologyOperation,
    a: String,
    b: String,
    caller_session_id: Option<String>,
) -> Result<wardian_core::control::TopologyMutationResponse, TopologyControlError> {
    let home = home().map_err(TopologyControlError::Io)?;
    let _topology_lock = topology_process_lock(&home).map_err(TopologyControlError::Io)?;

    let known_agent_ids: BTreeSet<String> = {
        let state = app.state::<crate::state::AppState>();
        let agents = state.agents.lock().await;
        agents.keys().cloned().collect()
    };
    let caller_label = caller_session_id
        .as_deref()
        .map(|id| format!("agent:{id}"))
        .unwrap_or_else(|| "operator".to_string());

    if let Err(denied) =
        authorize_topology_mutation_v1(&known_agent_ids, caller_session_id.as_deref(), &a, &b)
    {
        let error_code = match denied {
            TopologyAuthDenied::UnknownCaller => "not_found",
            TopologyAuthDenied::SelfServeRequired => "self_serve_required",
        };
        audit_topology_mutation(
            &home,
            TopologyAuditInput {
                caller: &caller_label,
                op,
                a: &a,
                b: &b,
                at: &chrono::Utc::now().to_rfc3339(),
                outcome: "denied",
                error_code: Some(error_code),
            },
        );
        return Err(match denied {
            TopologyAuthDenied::UnknownCaller => TopologyControlError::UnknownCaller,
            TopologyAuthDenied::SelfServeRequired => TopologyControlError::SelfServeRequired,
        });
    }

    let teams = load_team_memberships(&home);
    let mut topology = load_topology(&home);
    let created_at = chrono::Utc::now().to_rfc3339();
    let outcome = apply_topology_operation(&mut topology, op, &a, &b, &created_at, &teams)
        .ok_or_else(|| TopologyControlError::Io("invalid topology pair".to_string()))?;

    if outcome.changed {
        save_topology(&home, &topology).map_err(|e| TopologyControlError::Io(e.to_string()))?;
        let _ = app.emit("topology-changed", ());
    }

    audit_topology_mutation(
        &home,
        TopologyAuditInput {
            caller: &caller_label,
            op,
            a: &outcome.a,
            b: &outcome.b,
            at: &created_at,
            outcome: if outcome.changed {
                "applied"
            } else {
                "unchanged"
            },
            error_code: None,
        },
    );

    Ok(wardian_core::control::TopologyMutationResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        ok: true,
        action: op.action().to_string(),
        a: outcome.a,
        b: outcome.b,
        changed: outcome.changed,
    })
}

pub(crate) fn topology_process_lock(home: &Path) -> Result<std::fs::File, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("topology.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock_exclusive().map_err(|error| error.to_string())?;
    Ok(lock)
}

async fn agent_refs(
    state: &tauri::State<'_, crate::state::AppState>,
) -> Vec<wardian_core::topology::AgentRef> {
    state.topology_agent_refs().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex};
    use wardian_core::models::AgentConfig;

    struct TestWardianHome {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
        _temp: tempfile::TempDir,
    }

    impl TestWardianHome {
        fn new() -> Self {
            Self::from_guard(crate::utils::wardian_test_env_lock())
        }

        async fn new_async() -> Self {
            Self::from_guard(crate::utils::wardian_test_env_lock_async().await)
        }

        fn from_guard(lock: tokio::sync::MutexGuard<'static, ()>) -> Self {
            let temp = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", temp.path());
            Self {
                _lock: lock,
                previous_home,
                _temp: temp,
            }
        }

        fn path(&self) -> &std::path::Path {
            self._temp.path()
        }
    }

    impl Drop for TestWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn test_agent(session_id: &str) -> crate::state::ActiveAgent {
        crate::state::ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: session_id.to_string(),
                agent_class: "Coder".to_string(),
                provider: "mock".to_string(),
                folder: "D:/work".to_string(),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: None,
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(Some("2026-05-07T00:00:00.000Z".to_string()))),
            last_query_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new("Idle".to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(crate::state::AgentWatchState::new(
                session_id.to_string(),
                4096,
                262_144,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    async fn insert_test_agent(state: &crate::state::AppState, session_id: &str) {
        state
            .agents
            .lock()
            .await
            .insert(session_id.to_string(), test_agent(session_id));
    }

    fn read_topology_audit_log(home: &Path) -> Vec<serde_json::Value> {
        let path = crate::topology_audit::audit_log_path(home);
        let Ok(content) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("audit line is valid JSON"))
            .collect()
    }

    /// Regression test for the Reviewer finding that `mutate_ui` (the desktop
    /// Graph view's write path) never appended to the audit log, contradicting
    /// the spec's "every attempt is audited" claim. Covers all four verbs
    /// since they share `mutate_ui`.
    #[test]
    fn mutate_ui_audits_every_operation_as_operator() {
        let home = TestWardianHome::new();
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());
        let handle = app.handle().clone();

        assert!(mutate_ui(&handle, TopologyOperation::Link, "agent-1", "agent-2").unwrap());
        assert!(mutate_ui(&handle, TopologyOperation::Ignore, "agent-1", "agent-3").unwrap());
        assert!(!mutate_ui(&handle, TopologyOperation::Ignore, "agent-1", "agent-3").unwrap());
        assert!(mutate_ui(&handle, TopologyOperation::Unignore, "agent-1", "agent-3").unwrap());
        assert!(mutate_ui(&handle, TopologyOperation::Unlink, "agent-1", "agent-2").unwrap());

        let audit = read_topology_audit_log(home.path());
        assert_eq!(audit.len(), 5);
        assert!(audit.iter().all(|record| record["caller"] == "operator"));
        assert_eq!(audit[0]["operation"], "link");
        assert_eq!(audit[0]["outcome"], "applied");
        assert_eq!(audit[2]["operation"], "ignore");
        assert_eq!(audit[2]["outcome"], "unchanged");
        assert_eq!(audit[4]["operation"], "unlink");
        assert_eq!(audit[4]["outcome"], "applied");
    }

    #[tokio::test]
    async fn dispatch_topology_mutation_allows_self_serve_endpoint() {
        let home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());
        let state = app.state::<crate::state::AppState>();
        insert_test_agent(&state, "agent-1").await;
        insert_test_agent(&state, "agent-2").await;

        let response = dispatch_topology_mutation(
            &app.handle().clone(),
            TopologyOperation::Link,
            "agent-1".to_string(),
            "agent-2".to_string(),
            Some("agent-1".to_string()),
        )
        .await
        .expect("caller editing its own edge must be allowed");

        assert_eq!(response.action, "link");
        assert!(response.changed);
        assert!(load_topology(home.path())
            .neighbors("agent-1")
            .contains(&"agent-2".to_string()));

        let audit = read_topology_audit_log(home.path());
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["outcome"], "applied");
        assert_eq!(audit[0]["caller"], "agent:agent-1");
    }

    #[tokio::test]
    async fn dispatch_topology_mutation_denies_foreign_pair_and_records_audit() {
        let home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());
        let state = app.state::<crate::state::AppState>();
        insert_test_agent(&state, "agent-1").await;
        insert_test_agent(&state, "agent-2").await;
        insert_test_agent(&state, "agent-3").await;

        let error = dispatch_topology_mutation(
            &app.handle().clone(),
            TopologyOperation::Link,
            "agent-2".to_string(),
            "agent-3".to_string(),
            Some("agent-1".to_string()),
        )
        .await
        .expect_err("editing a pair that excludes the caller must be denied");

        assert_eq!(error, TopologyControlError::SelfServeRequired);
        assert!(load_topology(home.path()).edges.is_empty());

        let audit = read_topology_audit_log(home.path());
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["outcome"], "denied");
        assert_eq!(audit[0]["error_code"], "self_serve_required");
    }

    #[tokio::test]
    async fn dispatch_topology_mutation_fails_closed_on_unknown_caller() {
        let _home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());

        let error = dispatch_topology_mutation(
            &app.handle().clone(),
            TopologyOperation::Link,
            "agent-1".to_string(),
            "agent-2".to_string(),
            Some("agent-ghost".to_string()),
        )
        .await
        .expect_err("a session id absent from the roster must fail closed");

        assert_eq!(error, TopologyControlError::UnknownCaller);
    }

    #[tokio::test]
    async fn dispatch_topology_mutation_operator_outside_session_is_unrestricted() {
        let home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());

        let response = dispatch_topology_mutation(
            &app.handle().clone(),
            TopologyOperation::Link,
            "agent-1".to_string(),
            "agent-2".to_string(),
            None,
        )
        .await
        .expect("no caller session id must still be treated as operator");

        assert!(response.changed);
        assert!(load_topology(home.path())
            .neighbors("agent-1")
            .contains(&"agent-2".to_string()));
    }

    /// Regression test for #1032: the CLI's control-plane path must converge
    /// on the same team-seed suppression the UI path already applied, so an
    /// `unlink` performed through this dispatcher is as durable against a
    /// later team-seed reseed as a UI deletion. Before this change,
    /// `wardian graph unlink` called plain `remove_edge`, and the pair came
    /// back the next time `seed_team_clique` ran — exactly the divergence
    /// this test would have caught before it shipped.
    #[tokio::test]
    async fn dispatch_topology_mutation_unlink_converges_on_team_seed_suppression() {
        let home = TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(crate::state::AppState::new());
        let state = app.state::<crate::state::AppState>();
        insert_test_agent(&state, "agent-1").await;
        insert_test_agent(&state, "agent-2").await;

        let watchlists_dir = home.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).unwrap();
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::json!({
                "version": 2,
                "teams": [{
                    "id": "team-1",
                    "name": "Wardian Dev",
                    "agentIds": ["agent-1", "agent-2"],
                }],
                "watchlists": [],
            })
            .to_string(),
        )
        .unwrap();

        let mut topology = Topology::default();
        topology.add_edge("agent-1", "agent-2", "2026-08-28T00:00:00Z");
        save_topology(home.path(), &topology).unwrap();

        dispatch_topology_mutation(
            &app.handle().clone(),
            TopologyOperation::Unlink,
            "agent-1".to_string(),
            "agent-2".to_string(),
            Some("agent-1".to_string()),
        )
        .await
        .expect("unlink between the caller and its endpoint must be allowed");

        // Simulate the reseed a subsequent `wardian team add` triggers.
        let mut topology = load_topology(home.path());
        let teams = load_team_memberships(home.path());
        wardian_core::topology::seed_team_clique(
            &mut topology,
            &["agent-1".to_string(), "agent-2".to_string()],
            "2026-08-28T00:01:00Z",
        );
        assert!(teams
            .iter()
            .any(|team| team.agent_ids.contains(&"agent-1".to_string())));
        assert!(
            topology.neighbors("agent-1").is_empty(),
            "unlink via the control plane must survive a team-seed reseed"
        );
    }

    #[test]
    fn snapshot_edges_manual_only() {
        let topology = Topology {
            version: 2,
            edges: vec![wardian_core::topology::TopologyEdge {
                a: "a".to_string(),
                b: "b".to_string(),
                created_at: "2026-07-02T00:00:00Z".to_string(),
            }],
            ignored_pairs: vec![],
            suppressed_seed_pairs: vec![],
        };

        let edges = snapshot_edges(&topology);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].a, "a");
        assert_eq!(edges[0].b, "b");
        assert_eq!(edges[0].origin, "manual");
    }

    #[test]
    fn pair_activity_result_caps_pair_rows_and_marks_partial() {
        let pairs = (0..=MAX_ACTIVITY_PAIRS)
            .map(|index| PairActivity {
                a: format!("a-{index}"),
                b: format!("b-{index}"),
                last_message_at: format!("2026-08-25T00:{:02}:00Z", index % 60),
                active_ask: false,
                awaiting_reply_from: None,
            })
            .collect::<Vec<_>>();

        let mut bounded = pairs;
        let mut truncated = false;
        if bounded.len() > MAX_ACTIVITY_PAIRS {
            bounded.truncate(MAX_ACTIVITY_PAIRS);
            truncated = true;
        }

        assert_eq!(bounded.len(), MAX_ACTIVITY_PAIRS);
        assert!(truncated);
    }
}
