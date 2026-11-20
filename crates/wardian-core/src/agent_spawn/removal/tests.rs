use super::*;
use crate::models::AgentConfig;

fn profile() -> RemovalProfileIdentity {
    RemovalProfileIdentity {
        directory_id: None,
        ownership_records: BTreeMap::new(),
    }
}

fn fixture() -> (tempfile::TempDir, AgentConfig) {
    let home = tempfile::tempdir().unwrap();
    crate::db::init_db_at_path(&home.path().join("state.db")).unwrap();
    let config = AgentConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        session_name: "Bound".into(),
        provider: "mock".into(),
        agent_class: "Coder".into(),
        folder: "workspace".into(),
        ..Default::default()
    };
    let attempt = uuid::Uuid::new_v4().to_string();
    super::super::reserve_legacy_name(
        &attempt,
        &config.session_id,
        super::super::LegacyNamePurpose::Spawn,
        None,
        &config.session_name,
    )
    .unwrap();
    super::super::begin_legacy_name_effects(&attempt, &config.session_id).unwrap();
    crate::db::upsert_agent(&crate::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: &config.session_name,
        description: "",
        agent_class: &config.agent_class,
        provider: &config.provider,
        workspace: Some(&config.folder),
        project: None,
        is_off: true,
        created_at: None,
    })
    .unwrap();
    super::super::complete_legacy_name(&attempt, &config.session_id, std::slice::from_ref(&config))
        .unwrap();
    (home, config)
}

fn admit(config: &AgentConfig) -> BoundRemovalTicket {
    admit_bound_removal(
        &RemovalAgentIdentity::from(config),
        None,
        &profile(),
        || Ok(()),
    )
    .unwrap()
    .unwrap()
}

fn closure() -> RemovalClosureReceipt {
    RemovalClosureReceipt {
        runtime_generation: None,
        owned_process_count: 0,
        all_process_handles_joined: true,
        broker_closed: true,
    }
}

fn sql(statement: &str) {
    crate::db::get_db_conn(|conn| {
        conn.execute_batch(statement)?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn bound_delete_abort_retains_sealed_proof_across_reopen_then_same_ticket_is_idempotent() {
    let _env = crate::tests::env_lock();
    let (home, config) = fixture();
    let foreign = uuid::Uuid::new_v4().to_string();
    super::super::reserve_legacy_name(
        &foreign,
        &foreign,
        super::super::LegacyNamePurpose::Spawn,
        None,
        "Foreign",
    )
    .unwrap();
    let ticket = seal_bound_removal(&admit(&config), &closure(), &profile(), || Ok(())).unwrap();
    sql("CREATE TRIGGER cut_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'delete cut'); END;");
    assert!(complete_bound_removal(&ticket, &[], &profile(), || Ok(())).is_err());
    assert!(crate::db::get_all_agents()
        .unwrap()
        .iter()
        .any(|row| row.session_id == config.session_id));
    assert!(super::super::claimed_names().unwrap().contains("Bound"));
    crate::db::init_db_at_path(&home.path().join("state.db")).unwrap();
    let reopened = bound_removal_ticket(&config.session_id).unwrap().unwrap();
    assert_eq!(reopened, ticket);
    sql("DROP TRIGGER cut_delete");
    complete_bound_removal(&reopened, &[], &profile(), || Ok(())).unwrap();
    complete_bound_removal(&reopened, &[], &profile(), || Ok(())).unwrap();
    assert!(crate::db::get_all_agents().unwrap().is_empty());
    assert_eq!(
        bound_removal_ticket(&config.session_id)
            .unwrap()
            .unwrap()
            .state,
        "done"
    );
    let names = super::super::claimed_names().unwrap();
    assert!(!names.contains("Bound"));
    assert!(names.contains("Foreign"));
}

#[test]
fn bound_intent_and_unsealed_closure_cuts_never_authorize_metadata_completion() {
    let _env = crate::tests::env_lock();
    let (_home, config) = fixture();
    sql(
        "CREATE TRIGGER cut_intent BEFORE UPDATE OF removal_state ON legacy_name_attempts
        WHEN NEW.removal_state = 'intent' BEGIN SELECT RAISE(ABORT, 'intent cut'); END;",
    );
    assert!(admit_bound_removal(
        &RemovalAgentIdentity::from(&config),
        None,
        &profile(),
        || Ok(())
    )
    .is_err());
    assert!(bound_removal_ticket(&config.session_id).unwrap().is_none());
    sql("DROP TRIGGER cut_intent");
    let ticket = admit(&config);
    assert!(complete_bound_removal(
        &ticket,
        std::slice::from_ref(&config),
        &profile(),
        || Ok(())
    )
    .is_err());
    assert!(complete_bound_removal(&ticket, &[], &profile(), || Ok(())).is_err());
    let mut unproved = closure();
    unproved.all_process_handles_joined = false;
    assert!(seal_bound_removal(&ticket, &unproved, &profile(), || Ok(())).is_err());
    sql(
        "CREATE TRIGGER cut_seal BEFORE UPDATE OF removal_state ON legacy_name_attempts
        WHEN NEW.removal_state = 'closure_proved' BEGIN SELECT RAISE(ABORT, 'seal cut'); END;",
    );
    assert!(seal_bound_removal(&ticket, &closure(), &profile(), || Ok(())).is_err());
    assert_eq!(
        bound_removal_ticket(&config.session_id)
            .unwrap()
            .unwrap()
            .state,
        "intent"
    );
    assert!(complete_bound_removal(&ticket, &[], &profile(), || Ok(())).is_err());
    assert_eq!(crate::db::get_all_agents().unwrap().len(), 1);
}

#[test]
fn bound_completion_rejects_changed_json_database_profile_and_name_owner() {
    let _env = crate::tests::env_lock();
    for change in ["json", "database", "profile", "claim"] {
        let (_home, config) = fixture();
        let ticket =
            seal_bound_removal(&admit(&config), &closure(), &profile(), || Ok(())).unwrap();
        let mut actual_profile = profile();
        let mut roster = Vec::new();
        match change {
            "json" => roster.push(config.clone()),
            "database" => sql("UPDATE agents SET session_name = 'Replacement'"),
            "profile" => actual_profile.directory_id = Some((1, 2)),
            _ => sql(
                "UPDATE agent_name_claims SET owner_id = 'foreign' WHERE session_name = 'Bound'",
            ),
        }
        assert!(
            complete_bound_removal(&ticket, &roster, &actual_profile, || Ok(())).is_err(),
            "{change}"
        );
        assert_eq!(crate::db::get_all_agents().unwrap().len(), 1);
        assert_eq!(
            bound_removal_ticket(&config.session_id)
                .unwrap()
                .unwrap()
                .state,
            "closure_proved"
        );
    }
}

#[test]
fn terminal_later_attempt_never_revives_an_older_deletion_ticket() {
    let _env = crate::tests::env_lock();
    let (_home, config) = fixture();
    let ticket = seal_bound_removal(&admit(&config), &closure(), &profile(), || Ok(())).unwrap();
    crate::db::get_db_conn(|conn| {
        conn.execute(
            "INSERT INTO legacy_name_attempts
        (attempt_id,agent_id,purpose,new_name,phase) VALUES ('later',?1,'spawn','later','removed')",
            params![config.session_id],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(bound_removal_ticket(&config.session_id).is_err());
    assert!(complete_bound_removal(&ticket, &[], &profile(), || Ok(())).is_err());
    assert_eq!(crate::db::get_all_agents().unwrap().len(), 1);
}

#[test]
fn uncertain_operation_is_not_covered_by_a_legacy_process_join() {
    let _env = crate::tests::env_lock();
    let (_home, config) = fixture();
    let ticket = seal_bound_removal(&admit(&config), &closure(), &profile(), || Ok(())).unwrap();
    let request = super::super::AgentSpawnRequest {
        provider: "mock".into(),
        class: "Coder".into(),
        name: Some("Uncertain".into()),
        workspace: Some("workspace".into()),
        model: None,
        reasoning_effort: None,
    };
    super::super::admit(
        &uuid::Uuid::new_v4().to_string(),
        &request,
        &config.session_id,
        None,
    )
    .unwrap();
    assert!(complete_bound_removal(&ticket, &[], &profile(), || Ok(())).is_err());
    assert!(super::super::claimed_names().unwrap().contains("Uncertain"));
    assert_eq!(crate::db::get_all_agents().unwrap().len(), 1);
}
