//! Persisted identities can outlive restore configs without losing operator deletion.
use super::*;
use wardian_core::db::{self, AgentUpsert};

fn persist_metadata(id: &str, name: &str, workspace: &std::path::Path) {
    db::upsert_agent(&AgentUpsert {
        session_id: id,
        session_name: name,
        description: "isolated persisted-only fixture",
        agent_class: "Reviewer",
        provider: "codex",
        workspace: workspace.to_str(),
        project: None,
        is_off: false,
        created_at: None,
    })
    .unwrap();
    db::update_agent_status(id, "Off", None).unwrap();
}

fn fixture(
    home: &std::path::Path,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    String,
    std::path::PathBuf,
) {
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let id = uuid::Uuid::new_v4().to_string();
    persist_metadata(&id, "PersistedOnly", home);
    let profile = home.join("agents").join(&id);
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("retain.txt"), "owned profile").unwrap();
    (app, id, profile)
}

fn saved(home: &std::path::Path) -> Vec<AgentConfig> {
    serde_json::from_slice(&std::fs::read(home.join("settings/state.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn persisted_unloaded_force_preserves_confirmation_and_ownership_fences() {
    for case in [
        "confirmation",
        "pid",
        "active",
        "lease",
        "uncertain",
        "native",
        "spawn",
    ] {
        let home = crate::control::test_support::TestWardianHome::new_async().await;
        let (app, id, profile) = fixture(home.path());
        let config = AgentConfig {
            session_id: id.clone(),
            session_name: "PersistedOnly".into(),
            is_off: true,
            ..Default::default()
        };
        crate::manager::try_save_state_snapshot_unlocked(&[config]).unwrap();
        let before = std::fs::read(home.path().join("settings/state.json")).unwrap();
        match case {
            "pid" => db::update_agent_status(&id, "Off", Some(u32::MAX)).unwrap(),
            "active" => db::update_agent_status(&id, "Processing", None).unwrap(),
            "lease" | "uncertain" => {
                let now = chrono::Utc::now();
                // Unknown current resume identity must not waive an older
                // provider hold, including one past its nominal expiry.
                wardian_core::conversation_lease::save_leases(&[
                    wardian_core::conversation_lease::ConversationLease {
                        agent_id: id.clone(),
                        provider: "codex".into(),
                        resume_session: "old-session".into(),
                        owner_kind: if case == "uncertain" {
                            "prior_provider_hold"
                        } else {
                            "agent_lifecycle"
                        }
                        .into(),
                        owner_id: format!("owner-{id}"),
                        acquisition_id: uuid::Uuid::new_v4().to_string(),
                        owner_node_id: None,
                        mode: if case == "uncertain" {
                            "uncertain_previous_provider"
                        } else {
                            "lifecycle_transition"
                        }
                        .into(),
                        started_at: (now - chrono::Duration::minutes(10)).to_rfc3339(),
                        heartbeat_at: (now - chrono::Duration::minutes(10)).to_rfc3339(),
                        expires_at: (now
                            + chrono::Duration::minutes(if case == "uncertain" { -5 } else { 5 }))
                        .to_rfc3339(),
                    },
                ])
                .unwrap();
            }
            "native" => app
                .state::<AppState>()
                .native_delivery
                .prepare_opencode_http(&id, 1, "pending-config".into())
                .await
                .unwrap(),
            "spawn" => wardian_core::agent_spawn::reserve_legacy_name(
                &uuid::Uuid::new_v4().to_string(),
                &id,
                wardian_core::agent_spawn::LegacyNamePurpose::Rename,
                Some("PersistedOnly"),
                "PendingRename",
            )
            .unwrap(),
            _ => {}
        }
        let expected = match case {
            "confirmation" => "exactly match",
            "pid" => "recorded process",
            "active" => "persisted status is active",
            "lease" => "saved conversation is in use",
            "uncertain" => "uncertain previous-provider hold",
            "native" => "native owner or preparation",
            "spawn" => "unresolved spawn ownership",
            _ => unreachable!(),
        };
        let error = remove_agent(
            id.clone(),
            Some(if case == "confirmation" {
                "persistedonly"
            } else {
                "PersistedOnly"
            }),
            app.state::<AppState>(),
            app.handle().clone(),
            case == "active",
        )
        .await
        .unwrap_err();
        assert!(error.contains(expected), "{case}: {error}");
        assert!(
            db::get_all_agents()
                .unwrap()
                .iter()
                .any(|row| row.session_id == id),
            "{case}"
        );
        assert_eq!(
            std::fs::read(home.path().join("settings/state.json")).unwrap(),
            before,
            "{case}"
        );
        assert_eq!(
            std::fs::read_to_string(profile.join("retain.txt")).unwrap(),
            "owned profile",
            "{case}"
        );
        wardian_core::conversation_lease::save_leases(&[]).unwrap();
    }
}

#[tokio::test]
async fn persisted_unloaded_sql_rechecks_identity_spawn_and_lease_before_cascade() {
    for case in ["identity", "spawn", "lease"] {
        let home = crate::control::test_support::TestWardianHome::new_async().await;
        let (_app, id, profile) = fixture(home.path());
        let expected = db::get_all_agents()
            .unwrap()
            .into_iter()
            .find(|row| row.session_id == id)
            .unwrap();
        db::get_db_conn(|conn| {
            conn.execute(
                "INSERT INTO events(session_id,event_type,payload) VALUES (?1,'fixture','{}')",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
        if case == "identity" {
            db::update_agent_status(&id, "Processing", None).unwrap();
        } else if case == "spawn" {
            // A newly admitted asynchronous owner must also be checked inside
            // SQL, independently of the existing legacy-ticket exclusion.
            wardian_core::agent_spawn::admit(
                &uuid::Uuid::new_v4().to_string(),
                &wardian_core::agent_spawn::AgentSpawnRequest {
                    provider: "mock".into(),
                    class: "Reviewer".into(),
                    name: Some("AtomicPendingSpawn".into()),
                    workspace: None,
                    model: None,
                    reasoning_effort: None,
                },
                &id,
                None,
            )
            .unwrap();
        }
        let error = db::delete_unloaded_agent_fenced(&expected, || {
            if case == "lease" {
                Err("lost retained lifecycle authority".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .to_string();
        let expected_error = match case {
            "identity" => "identity changed",
            "spawn" => "unresolved spawn",
            _ => "lost retained lifecycle",
        };
        assert!(error.contains(expected_error), "{case}: {error}");
        assert!(db::get_all_agents()
            .unwrap()
            .iter()
            .any(|row| row.session_id == id));
        db::get_db_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM events WHERE session_id=?1 AND event_type='fixture'",
                    [&id],
                    |row| row.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
        assert!(profile.join("retain.txt").exists());
    }
}

#[tokio::test]
async fn persisted_unloaded_sql_failure_restores_actual_saved_config_and_rolls_back_history() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (app, id, profile) = fixture(home.path());
    let target = AgentConfig {
        session_id: id.clone(),
        session_name: "PersistedOnly".into(),
        provider: "codex".into(),
        description: "original complete saved config".into(),
        resume_session: Some("original-session".into()),
        is_off: true,
        ..Default::default()
    };
    let peer = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        session_name: "UnloadedPeer".into(),
        ..Default::default()
    };
    crate::manager::try_save_state_snapshot_unlocked(&[target.clone(), peer.clone()]).unwrap();
    let original_roster = saved(home.path());
    db::get_db_conn(|conn| {
        conn.execute("INSERT INTO events(session_id,event_type,payload) VALUES (?1,'fixture','{}')", [&id])?;
        conn.execute_batch("CREATE TRIGGER reject_unloaded_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'isolated SQL failure'); END;")?;
        Ok(())
    }).unwrap();
    let error = remove_agent(
        id.clone(),
        Some("PersistedOnly"),
        app.state::<AppState>(),
        app.handle().clone(),
        true,
    )
    .await
    .unwrap_err();
    assert!(error.contains("isolated SQL failure"), "{error}");
    assert!(db::get_all_agents()
        .unwrap()
        .iter()
        .any(|row| row.session_id == id));
    let roster = saved(home.path());
    assert_eq!(roster.len(), 2);
    for original in original_roster {
        let current = roster
            .iter()
            .find(|config| config.session_id == original.session_id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(current).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
    db::get_db_conn(|conn| {
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM events WHERE session_id=?1 AND event_type='fixture'",
                [&id],
                |row| row.get::<_, i64>(0)
            )?,
            1
        );
        Ok(())
    })
    .unwrap();
    assert!(profile.join("retain.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persisted_unloaded_cancellation_retains_authority_until_physical_delete() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (app, id, profile) = fixture(home.path());
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (started, _started_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let handle = app.handle().clone();
    let target = id.clone();
    let removal = tokio::spawn(async move {
        crate::state::interactions::DELETE_IO_PROBE
            .scope(
                std::cell::RefCell::new(Some(crate::state::interactions::DeleteIoProbe {
                    entered,
                    started,
                    release: release_rx,
                })),
                remove_agent(
                    target,
                    Some("PersistedOnly"),
                    handle.state::<AppState>(),
                    handle.clone(),
                    true,
                ),
            )
            .await
    });
    let admitted = tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx).await;
    if admitted.is_err() {
        let _ = release.send(());
    }
    admitted.unwrap().unwrap();
    removal.abort();
    assert!(removal.await.unwrap_err().is_cancelled());
    let lease_held = wardian_core::conversation_lease::load_leases_checked()
        .unwrap()
        .iter()
        .any(|lease| lease.agent_id == id && lease.owner_kind == "agent_lifecycle");
    let contender = tokio::task::spawn_blocking(|| {
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false).unwrap()
    })
    .await
    .unwrap();
    let barrier_held = contender.is_none();
    drop(contender);
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if !profile.exists()
                && !wardian_core::conversation_lease::load_leases_checked()
                    .unwrap()
                    .iter()
                    .any(|lease| lease.agent_id == id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(lease_held && barrier_held);
    assert!(db::get_all_agents()
        .unwrap()
        .iter()
        .all(|row| row.session_id != id));
}

#[tokio::test]
async fn persisted_unloaded_agent_delete_cascades_without_removing_peers_or_workspace() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let sentinel = workspace.path().join("keep.txt");
    std::fs::write(&sentinel, "workspace bytes stay").unwrap();
    let app = tauri::test::mock_app();
    app.manage(AppState::new());
    let state = app.state::<AppState>();
    let target = uuid::Uuid::new_v4().to_string();
    let unloaded_peer = uuid::Uuid::new_v4().to_string();
    let live_peer = uuid::Uuid::new_v4().to_string();
    for (id, name) in [
        (&target, "PersistedOnly"),
        (&unloaded_peer, "UnloadedPeer"),
        (&live_peer, "LivePeer"),
    ] {
        persist_metadata(id, name, workspace.path());
    }
    let live = make_test_agent();
    {
        let mut config = live.config.lock().unwrap();
        config.session_id = live_peer.clone();
        config.session_name = "LivePeer".into();
        config.is_off = true;
    }
    let saved_live = live.config.lock().unwrap().clone();
    let saved_unloaded = AgentConfig {
        session_id: unloaded_peer.clone(),
        session_name: "UnloadedPeer".into(),
        description: "preserve this saved but unloaded config".into(),
        is_off: true,
        ..Default::default()
    };
    state.agents.lock().await.insert(live_peer.clone(), live);
    state.agent_order.lock().await.push(live_peer.clone());
    crate::manager::try_save_state_snapshot_unlocked(&[saved_live, saved_unloaded]).unwrap();

    let profile = home.path().join("agents").join(&target);
    let sessions = profile.join("habitat/.codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("owned.jsonl"), "private history").unwrap();
    create_directory_link(workspace.path(), &profile.join("habitat/workspace")).unwrap();
    db::get_db_conn(|conn| {
        for id in [&target, &unloaded_peer, &live_peer] {
            conn.execute(
                "INSERT INTO events (session_id,event_type,payload) VALUES (?1,'fixture','{}')",
                [id],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let watchlists = home.path().join("watchlists");
    std::fs::create_dir_all(&watchlists).unwrap();
    std::fs::write(
        watchlists.join("index.json"),
        serde_json::json!({
            "version": 2,
            "teams": [{"id":"team", "name":"Peers", "agentIds":[target, unloaded_peer, live_peer]}],
            "watchlists": [{"id":"main", "name":"Main", "entries":[
                {"type":"agent", "agentId":target},
                {"type":"agent", "agentId":unloaded_peer},
                {"type":"agent", "agentId":live_peer}
            ]}]
        })
        .to_string(),
    )
    .unwrap();
    let mut topology = wardian_core::topology::Topology::default();
    topology.add_edge(&target, &live_peer, "2026-11-21T00:00:00Z");
    topology.add_edge(&live_peer, &unloaded_peer, "2026-11-21T00:00:01Z");
    wardian_core::topology::save_topology(home.path(), &topology).unwrap();

    assert!(!state.agents.lock().await.contains_key(&target));
    remove_agent(
        target.clone(),
        Some("PersistedOnly"),
        state,
        app.handle().clone(),
        true,
    )
    .await
    .expect("an operator-confirmed persisted-only identity must be deletable");

    let agents = db::get_all_agents().unwrap();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|agent| agent.session_id != target));
    assert!(app
        .state::<AppState>()
        .agents
        .lock()
        .await
        .contains_key(&live_peer));
    db::get_db_conn(|conn| {
        let remaining: Vec<String> = conn
            .prepare(
                "SELECT session_id FROM events WHERE event_type='fixture' ORDER BY session_id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.contains(&target));
        Ok(())
    })
    .unwrap();
    let saved: Vec<AgentConfig> =
        serde_json::from_slice(&std::fs::read(home.path().join("settings/state.json")).unwrap())
            .unwrap();
    assert_eq!(saved.len(), 2);
    assert!(saved
        .iter()
        .any(|config| config.session_id == unloaded_peer));
    assert!(saved.iter().any(|config| config.session_id == live_peer));
    assert!(
        !profile.exists(),
        "private habitat and session history must be removed"
    );
    assert_eq!(
        std::fs::read_to_string(sentinel).unwrap(),
        "workspace bytes stay"
    );
    let watchlists = std::fs::read_to_string(watchlists.join("index.json")).unwrap();
    assert!(!watchlists.contains(&target));
    assert!(watchlists.contains(&unloaded_peer));
    assert!(watchlists.contains(&live_peer));
    let topology = std::fs::read_to_string(home.path().join("topology.json")).unwrap();
    assert!(!topology.contains(&target));
    assert!(topology.contains(&unloaded_peer));
    assert!(topology.contains(&live_peer));
}
