use serde_json::{json, Value};
use wardian_core::{
    models::workbench::{WorkbenchDocumentV1, MAX_SAFE_INTEGER},
    paths::{workbench_backup_path_for_home, workbench_path_for_home},
    workbench::{
        load_workbench_for_home, reset_workbench_for_home, save_workbench_for_home,
        WorkbenchLoadSource, WorkbenchPersistenceOutcome, WorkbenchResetRequest,
        WorkbenchSaveRequest,
    },
};

fn fixture_value() -> Value {
    serde_json::from_str(include_str!("fixtures/workbench-v1.json")).unwrap()
}

fn fixture_document() -> WorkbenchDocumentV1 {
    serde_json::from_value(fixture_value()).unwrap()
}

fn nested_root(depth: usize) -> Value {
    let mut node = json!({ "kind": "group", "group_id": "group-main" });
    for index in 1..depth {
        node = json!({
            "kind": "split",
            "node_id": format!("split-{index}"),
            "direction": "horizontal",
            "ratio": 0.5,
            "first": node,
            "second": {
                "kind": "group",
                "group_id": format!("group-{index}"),
            },
        });
    }
    node
}

fn document_with_depth(depth: usize) -> WorkbenchDocumentV1 {
    let mut value = fixture_value();
    value["root"] = nested_root(depth);
    let groups = value["groups"].as_object_mut().unwrap();
    groups.remove("group-plugin");
    for index in 1..depth {
        groups.insert(
            format!("group-{index}"),
            json!({
                "group_id": format!("group-{index}"),
                "surface_ids": [],
                "active_surface_id": null,
            }),
        );
    }
    value["surfaces"]
        .as_object_mut()
        .unwrap()
        .remove("surface-missing-plugin");
    serde_json::from_value(value).unwrap()
}

#[test]
fn workbench_v1_shared_fixture_deserializes() {
    let document = fixture_document();

    assert_eq!(document.schema_version, 1);
    document.validate().unwrap();
}

#[test]
fn workbench_validation_rejects_duplicate_and_missing_references() {
    let mut duplicate = fixture_value();
    duplicate["groups"]["group-main"]["surface_ids"] =
        json!(["surface-overview", "surface-overview"]);
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(duplicate)
        .unwrap()
        .validate()
        .is_err());

    let mut missing_active = fixture_value();
    missing_active["groups"]["group-main"]["active_surface_id"] = json!("surface-missing");
    assert!(
        serde_json::from_value::<WorkbenchDocumentV1>(missing_active)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn workbench_tree_rejects_cycle_shaped_json_and_depth_above_64() {
    let mut cycle_shaped = fixture_value();
    cycle_shaped["root"] = json!({
        "kind": "split",
        "node_id": "split-cycle",
        "direction": "horizontal",
        "ratio": 0.5,
        "first": { "kind": "group", "group_id": "group-main" },
        "second": { "kind": "cycle", "node_id": "split-cycle" },
    });
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(cycle_shaped).is_err());

    document_with_depth(64).validate().unwrap();
    assert!(document_with_depth(65).validate().is_err());
}

#[test]
fn workbench_validation_rejects_invalid_ratio_and_too_many_recent_surfaces() {
    let mut invalid_ratio = fixture_value();
    invalid_ratio["root"]["ratio"] = json!(0.09);
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(invalid_ratio)
        .unwrap()
        .validate()
        .is_err());

    let mut too_many_recent = fixture_value();
    let closed = too_many_recent["recently_closed"][0].clone();
    too_many_recent["recently_closed"] = Value::Array(std::iter::repeat_n(closed, 21).collect());
    assert!(
        serde_json::from_value::<WorkbenchDocumentV1>(too_many_recent)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn workbench_validation_enforces_surface_and_document_utf8_limits() {
    let mut oversized_state = fixture_value();
    oversized_state["surfaces"]["surface-overview"]["state"] = json!("x".repeat(65_536));
    assert!(
        serde_json::from_value::<WorkbenchDocumentV1>(oversized_state)
            .unwrap()
            .validate()
            .is_err()
    );

    let mut oversized_document = fixture_value();
    {
        let groups = oversized_document["groups"].as_object_mut().unwrap();
        groups.get_mut("group-plugin").unwrap()["surface_ids"] = json!([]);
        groups.get_mut("group-plugin").unwrap()["active_surface_id"] = Value::Null;
    }
    let mut surface_ids = Vec::new();
    let surfaces = oversized_document["surfaces"].as_object_mut().unwrap();
    surfaces.clear();
    for index in 0..33 {
        let surface_id = format!("surface-{index}");
        surface_ids.push(Value::String(surface_id.clone()));
        surfaces.insert(
            surface_id.clone(),
            json!({
                "surface_id": surface_id,
                "surface_type": "opaque",
                "state_schema_version": 1,
                "state": "x".repeat(65_534),
            }),
        );
    }
    let main = oversized_document["groups"]["group-main"]
        .as_object_mut()
        .unwrap();
    main["surface_ids"] = Value::Array(surface_ids);
    main["active_surface_id"] = json!("surface-0");
    assert!(
        serde_json::from_value::<WorkbenchDocumentV1>(oversized_document)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn workbench_unknown_surface_state_round_trips_opaquely() {
    let document = fixture_document();
    document.validate().unwrap();

    let encoded = serde_json::to_vec(&document).unwrap();
    let decoded: WorkbenchDocumentV1 = serde_json::from_slice(&encoded).unwrap();
    decoded.validate().unwrap();

    let unknown = &decoded.surfaces["surface-missing-plugin"];
    assert_eq!(unknown.surface_type, "example-contribution");
    assert_eq!(unknown.state_schema_version, 7);
    assert_eq!(
        unknown.state,
        json!({
            "query": "status:open",
            "columns": ["name", "status"],
            "unicode_label": "Habitat 🌿",
        })
    );
}

#[test]
fn workbench_validation_rejects_non_v1_unsafe_integer_and_explicit_null_resource_key() {
    let mut future = fixture_value();
    future["schema_version"] = json!(2);
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(future)
        .unwrap()
        .validate()
        .is_err());

    let mut unsafe_revision = fixture_document();
    unsafe_revision.revision = MAX_SAFE_INTEGER + 1;
    assert!(unsafe_revision.validate().is_err());

    let mut null_resource = fixture_value();
    null_resource["surfaces"]["surface-overview"]["resource_key"] = Value::Null;
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(null_resource).is_err());
}

#[test]
fn workbench_validation_matches_timestamp_number_and_exact_field_semantics() {
    for saved_at in ["0000-01-01T00:00:00.000Z", "9999-12-31T23:59:59.999Z"] {
        let mut document = fixture_document();
        document.saved_at = saved_at.to_string();
        document.validate().unwrap();
    }
    for saved_at in [
        "2026-02-29T00:00:00.000Z",
        "2026-07-10T12:34:56Z",
        "2026-07-10T12:34:56.789+00:00",
    ] {
        let mut document = fixture_document();
        document.saved_at = saved_at.to_string();
        assert!(document.validate().is_err(), "accepted {saved_at}");
    }

    let mut negative_zero_shell = fixture_document();
    negative_zero_shell.shell.left_sidebar_width = -0.0;
    assert!(negative_zero_shell.validate().is_err());

    let mut negative_zero_state = fixture_document();
    negative_zero_state
        .surfaces
        .get_mut("surface-overview")
        .unwrap()
        .state = json!(-0.0);
    assert!(negative_zero_state.validate().is_err());

    let mut unsafe_nested_integers = fixture_document();
    unsafe_nested_integers
        .surfaces
        .get_mut("surface-overview")
        .unwrap()
        .state_schema_version = MAX_SAFE_INTEGER + 1;
    unsafe_nested_integers.recently_closed[0].previous_index = MAX_SAFE_INTEGER + 1;
    assert!(unsafe_nested_integers.validate().is_err());

    let mut extra_document_field = fixture_value();
    extra_document_field["unexpected"] = json!(true);
    assert!(serde_json::from_value::<WorkbenchDocumentV1>(extra_document_field).is_err());
}

#[test]
fn workbench_paths_are_scoped_to_settings() {
    let home = std::path::Path::new("wardian-home");
    assert_eq!(
        workbench_path_for_home(home),
        home.join("settings").join("workbench.json")
    );
    assert_eq!(
        workbench_backup_path_for_home(home),
        home.join("settings").join("workbench.backup.json")
    );
}

fn next_document(base: &WorkbenchDocumentV1, revision: u64, marker: f64) -> WorkbenchDocumentV1 {
    let mut document = base.clone();
    document.revision = revision;
    document.saved_at = "2026-07-10T12:34:56.789Z".to_string();
    document.shell.left_sidebar_width = marker;
    document
}

#[test]
fn workbench_load_classifies_default_primary_and_backup() {
    let default_home = tempfile::tempdir().unwrap();
    let default_load = load_workbench_for_home(default_home.path()).unwrap();
    assert_eq!(default_load.source, WorkbenchLoadSource::Default);
    assert_eq!(default_load.durable_revision, Some(0));
    assert!(default_load.durable_token.is_some());
    assert_eq!(
        default_load.document,
        Some(WorkbenchDocumentV1::default_document())
    );

    let primary_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(primary_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    std::fs::write(&primary_path, include_bytes!("fixtures/workbench-v1.json")).unwrap();
    let primary_load = load_workbench_for_home(primary_home.path()).unwrap();
    assert_eq!(primary_load.source, WorkbenchLoadSource::Primary);
    assert_eq!(primary_load.document, Some(fixture_document()));

    let backup_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(backup_home.path());
    let backup_path = workbench_backup_path_for_home(backup_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    std::fs::write(&primary_path, b"{corrupt").unwrap();
    std::fs::write(&backup_path, include_bytes!("fixtures/workbench-v1.json")).unwrap();
    let backup_load = load_workbench_for_home(backup_home.path()).unwrap();
    assert_eq!(backup_load.source, WorkbenchLoadSource::Backup);
    assert_eq!(backup_load.document, Some(fixture_document()));
    assert!(backup_load.notice.is_some());
}

#[test]
fn workbench_load_rejects_oversized_raw_v1_and_honors_future_backup() {
    let oversized_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(oversized_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    let compact = serde_json::to_string(&WorkbenchDocumentV1::default_document()).unwrap();
    let mut oversized = String::from("{");
    oversized.push_str(&" ".repeat(2 * 1024 * 1024));
    oversized.push_str(compact.strip_prefix('{').unwrap());
    std::fs::write(&primary_path, oversized).unwrap();
    let load = load_workbench_for_home(oversized_home.path()).unwrap();
    assert_eq!(load.source, WorkbenchLoadSource::Default);
    assert!(load.notice.is_some());

    let future_backup_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(future_backup_home.path());
    let backup_path = workbench_backup_path_for_home(future_backup_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    std::fs::write(&primary_path, b"{corrupt").unwrap();
    let future_backup = br#"{"schema_version":9,"future":"backup"}"#;
    std::fs::write(&backup_path, future_backup).unwrap();
    let load = load_workbench_for_home(future_backup_home.path()).unwrap();
    assert_eq!(load.source, WorkbenchLoadSource::FutureSchema);
    assert_eq!(std::fs::read(&backup_path).unwrap(), future_backup);

    let valid_primary_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(valid_primary_home.path());
    let backup_path = workbench_backup_path_for_home(valid_primary_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    let primary_bytes = include_bytes!("fixtures/workbench-v1.json");
    std::fs::write(&primary_path, primary_bytes).unwrap();
    std::fs::write(&backup_path, future_backup).unwrap();
    let load = load_workbench_for_home(valid_primary_home.path()).unwrap();
    assert_eq!(load.source, WorkbenchLoadSource::FutureSchema);
    assert_eq!(std::fs::read(&primary_path).unwrap(), primary_bytes);
    assert_eq!(std::fs::read(&backup_path).unwrap(), future_backup);
}

#[test]
fn workbench_save_enforces_cas_and_idempotent_request_semantics() {
    let home = tempfile::tempdir().unwrap();
    let initial = load_workbench_for_home(home.path()).unwrap();
    let initial_document = initial.document.unwrap();
    let initial_token = initial.durable_token.unwrap();
    let document = next_document(&initial_document, 1, 241.0);
    let request = WorkbenchSaveRequest {
        document: document.clone(),
        expected_revision: 0,
        expected_token: initial_token.clone(),
        request_id: "request-first-save".to_string(),
    };

    let saved = save_workbench_for_home(home.path(), request.clone()).unwrap();
    assert_eq!(saved.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(saved.request_id, "request-first-save");
    assert_eq!(saved.durable_revision, Some(1));
    let saved_token = saved.durable_token.clone().unwrap();

    let retry = save_workbench_for_home(home.path(), request).unwrap();
    assert_eq!(retry.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(retry.durable_token, Some(saved_token.clone()));

    let reused = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: next_document(&initial_document, 1, 242.0),
            expected_revision: 0,
            expected_token: initial_token.clone(),
            request_id: "request-first-save".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        reused.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
    assert_eq!(reused.durable_revision, Some(1));
    assert_eq!(reused.durable_token, Some(saved_token.clone()));

    let same_revision_different_bytes = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: next_document(&initial_document, 1, 243.0),
            expected_revision: 0,
            expected_token: initial_token,
            request_id: "request-same-revision-different-bytes".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        same_revision_different_bytes.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
    assert_eq!(
        same_revision_different_bytes.durable_token,
        Some(saved_token)
    );
}

#[test]
fn workbench_save_rejects_wrong_token_and_non_successor_revision() {
    let home = tempfile::tempdir().unwrap();
    let initial = load_workbench_for_home(home.path()).unwrap();
    let document = initial.document.unwrap();
    let durable_token = initial.durable_token.unwrap();

    let wrong_token = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: next_document(&document, 1, 241.0),
            expected_revision: 0,
            expected_token: "not-the-durable-token".to_string(),
            request_id: "request-wrong-token".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        wrong_token.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );

    let skipped_revision = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: next_document(&document, 2, 242.0),
            expected_revision: 0,
            expected_token: durable_token,
            request_id: "request-skipped-revision".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        skipped_revision.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
}

#[test]
fn workbench_request_replays_conflict_after_durable_state_advances() {
    let save_home = tempfile::tempdir().unwrap();
    let initial = load_workbench_for_home(save_home.path()).unwrap();
    let initial_document = initial.document.unwrap();
    let request_a = WorkbenchSaveRequest {
        document: next_document(&initial_document, 1, 250.0),
        expected_revision: 0,
        expected_token: initial.durable_token.unwrap(),
        request_id: "request-old-save".to_string(),
    };
    let saved_a = save_workbench_for_home(save_home.path(), request_a.clone()).unwrap();
    let saved_b = save_workbench_for_home(
        save_home.path(),
        WorkbenchSaveRequest {
            document: next_document(&request_a.document, 2, 251.0),
            expected_revision: 1,
            expected_token: saved_a.durable_token.unwrap(),
            request_id: "request-new-save".to_string(),
        },
    )
    .unwrap();
    assert_eq!(saved_b.outcome, WorkbenchPersistenceOutcome::Saved);

    let stale_save_retry = save_workbench_for_home(save_home.path(), request_a).unwrap();
    assert_eq!(
        stale_save_retry.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
    assert_eq!(stale_save_retry.durable_revision, Some(2));

    let reset_home = tempfile::tempdir().unwrap();
    let initial = load_workbench_for_home(reset_home.path()).unwrap();
    let reset_request = WorkbenchResetRequest {
        expected_revision: 0,
        expected_token: initial.durable_token.unwrap(),
        request_id: "request-old-reset".to_string(),
    };
    let reset = reset_workbench_for_home(reset_home.path(), reset_request.clone()).unwrap();
    let reset_document = reset.document.unwrap();
    let saved_after_reset = save_workbench_for_home(
        reset_home.path(),
        WorkbenchSaveRequest {
            document: next_document(&reset_document, 2, 252.0),
            expected_revision: 1,
            expected_token: reset.durable_token.unwrap(),
            request_id: "request-after-reset".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        saved_after_reset.outcome,
        WorkbenchPersistenceOutcome::Saved
    );

    let stale_reset_retry = reset_workbench_for_home(reset_home.path(), reset_request).unwrap();
    assert_eq!(
        stale_reset_retry.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
    assert_eq!(stale_reset_retry.durable_revision, Some(2));
}

#[test]
fn workbench_save_rotates_only_a_valid_primary_and_recovers_from_backup() {
    let valid_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(valid_home.path());
    let backup_path = workbench_backup_path_for_home(valid_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    let fixture_bytes = include_bytes!("fixtures/workbench-v1.json");
    std::fs::write(&primary_path, fixture_bytes).unwrap();
    let initial = load_workbench_for_home(valid_home.path()).unwrap();
    let saved = save_workbench_for_home(
        valid_home.path(),
        WorkbenchSaveRequest {
            document: next_document(initial.document.as_ref().unwrap(), 1, 244.0),
            expected_revision: 0,
            expected_token: initial.durable_token.unwrap(),
            request_id: "request-valid-rotation".to_string(),
        },
    )
    .unwrap();
    assert_eq!(saved.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(std::fs::read(&backup_path).unwrap(), fixture_bytes);

    let recovered_home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(recovered_home.path());
    let backup_path = workbench_backup_path_for_home(recovered_home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    std::fs::write(&primary_path, b"{corrupt primary").unwrap();
    std::fs::write(&backup_path, fixture_bytes).unwrap();
    let recovered = load_workbench_for_home(recovered_home.path()).unwrap();
    assert_eq!(recovered.source, WorkbenchLoadSource::Backup);
    let saved = save_workbench_for_home(
        recovered_home.path(),
        WorkbenchSaveRequest {
            document: next_document(recovered.document.as_ref().unwrap(), 1, 245.0),
            expected_revision: 0,
            expected_token: recovered.durable_token.unwrap(),
            request_id: "request-backup-recovery".to_string(),
        },
    )
    .unwrap();
    assert_eq!(saved.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(std::fs::read(&backup_path).unwrap(), fixture_bytes);
    assert_eq!(
        load_workbench_for_home(recovered_home.path())
            .unwrap()
            .source,
        WorkbenchLoadSource::Primary
    );
}

#[test]
fn workbench_future_schema_is_preserved_and_read_only() {
    let home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(home.path());
    let backup_path = workbench_backup_path_for_home(home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    let future_bytes = br#"{"schema_version":2,"future":"preserve exactly"}"#;
    let backup_bytes = b"backup sentinel";
    std::fs::write(&primary_path, future_bytes).unwrap();
    std::fs::write(&backup_path, backup_bytes).unwrap();

    let load = load_workbench_for_home(home.path()).unwrap();
    assert_eq!(load.source, WorkbenchLoadSource::FutureSchema);
    assert!(load.document.is_none());
    assert!(load.durable_revision.is_none());
    assert!(load.durable_token.is_none());

    let save = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: next_document(&WorkbenchDocumentV1::default_document(), 1, 246.0),
            expected_revision: 0,
            expected_token: "ignored".to_string(),
            request_id: "request-future-save".to_string(),
        },
    )
    .unwrap();
    assert_eq!(save.outcome, WorkbenchPersistenceOutcome::FutureSchema);
    assert_eq!(std::fs::read(&primary_path).unwrap(), future_bytes);
    assert_eq!(std::fs::read(&backup_path).unwrap(), backup_bytes);
}

#[test]
fn workbench_max_safe_revision_cannot_advance() {
    let home = tempfile::tempdir().unwrap();
    let primary_path = workbench_path_for_home(home.path());
    std::fs::create_dir_all(primary_path.parent().unwrap()).unwrap();
    let mut maximum = WorkbenchDocumentV1::default_document();
    maximum.revision = MAX_SAFE_INTEGER;
    std::fs::write(&primary_path, serde_json::to_vec(&maximum).unwrap()).unwrap();
    let load = load_workbench_for_home(home.path()).unwrap();

    let mut attempted_successor = maximum;
    attempted_successor.shell.left_sidebar_width = 241.0;
    let result = save_workbench_for_home(
        home.path(),
        WorkbenchSaveRequest {
            document: attempted_successor,
            expected_revision: MAX_SAFE_INTEGER,
            expected_token: load.durable_token.unwrap(),
            request_id: "request-max-safe".to_string(),
        },
    )
    .unwrap();
    assert_eq!(
        result.outcome,
        WorkbenchPersistenceOutcome::RevisionConflict
    );
}

#[test]
fn workbench_concurrent_saves_have_one_cas_winner() {
    let home = tempfile::tempdir().unwrap();
    let initial = load_workbench_for_home(home.path()).unwrap();
    let base = initial.document.unwrap();
    let token = initial.durable_token.unwrap();
    let home_path = home.path().to_path_buf();

    let handles = [247.0, 248.0].map(|marker| {
        let home_path = home_path.clone();
        let document = next_document(&base, 1, marker);
        let token = token.clone();
        std::thread::spawn(move || {
            save_workbench_for_home(
                &home_path,
                WorkbenchSaveRequest {
                    document,
                    expected_revision: 0,
                    expected_token: token,
                    request_id: format!("request-concurrent-{marker}"),
                },
            )
            .unwrap()
            .outcome
        })
    });
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == WorkbenchPersistenceOutcome::Saved)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == WorkbenchPersistenceOutcome::RevisionConflict)
            .count(),
        1
    );
}

#[test]
fn workbench_load_cleans_owned_stale_temps_and_reset_preserves_other_files() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings");
    std::fs::create_dir_all(&settings).unwrap();
    let primary_temp = settings.join(".workbench.json.tmp");
    let backup_temp = settings.join(".workbench.backup.json.deadbeef.tmp");
    let unrelated_temp = settings.join(".other.json.tmp");
    let sentinel = home.path().join("agents").join("keep.txt");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&primary_temp, b"stale").unwrap();
    std::fs::write(&backup_temp, b"stale").unwrap();
    std::fs::write(&unrelated_temp, b"keep").unwrap();
    std::fs::write(&sentinel, b"domain data").unwrap();

    let initial = load_workbench_for_home(home.path()).unwrap();
    assert!(!primary_temp.exists());
    assert!(!backup_temp.exists());
    assert!(unrelated_temp.exists());

    let request = WorkbenchResetRequest {
        expected_revision: 0,
        expected_token: initial.durable_token.unwrap(),
        request_id: "request-reset".to_string(),
    };
    let reset = reset_workbench_for_home(home.path(), request.clone()).unwrap();
    assert_eq!(reset.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(reset.request_id, "request-reset");
    assert_eq!(reset.durable_revision, Some(1));
    assert_eq!(reset.document.as_ref().unwrap().revision, 1);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"domain data");
    assert!(unrelated_temp.exists());

    let retry = reset_workbench_for_home(home.path(), request).unwrap();
    assert_eq!(retry.outcome, WorkbenchPersistenceOutcome::Saved);
    assert_eq!(retry.durable_token, reset.durable_token);
    assert_eq!(retry.document, reset.document);
}
