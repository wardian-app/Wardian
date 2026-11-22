use super::*;
use serde_json::{json, Value};
use std::{fs, path::Path};
use tempfile::TempDir;

const ID: &str = "conv-test";

fn pending_native(id: &str) -> Value {
    json!({"id":id,"session_id":"agent","provider":"test","kind":"message",
        "role":"assistant","text":"future native body","metadata":{}})
}

fn pending_generated(fixture: &Fixture, number: u64) -> Value {
    let id = format!("generated:{ID}:{number}");
    let mut record = fixture.records[0].clone();
    record["seq"] = json!(number);
    record["event_refs"] = json!([id]);
    record["source_refs"] = json!([]);
    record["artifact_refs"] = json!([]);
    record["request_root_id"] = Value::Null;
    record["causal_ref"] = Value::Null;
    json!({"id":id,"session_id":"agent","provider":"wardian","kind":"message",
        "role":"user","text":"pending generated body",
        "metadata":{"generated":true,"archive_record":record}})
}

#[test]
fn existing_vacancy_defers_only_later_native_publication_and_new_authorization_stays_strict() {
    let mut fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    fixture.events.push(pending_native("future-native"));
    fixture.flush();
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
    rejected(fixture.inspect(), "references_resolved");
}

#[test]
fn unowned_native_before_historical_cohort_is_not_a_deferred_tail() {
    let mut fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    fixture
        .events
        .insert(0, pending_native("unresolved-historical"));
    fixture.flush();
    error_rule(
        validate_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "pending_record_boundary",
    );
}

#[test]
fn new_vacancy_rejects_historical_pending_generated_envelope() {
    let mut fixture = Fixture::new();
    let pending = pending_generated(&fixture, 1);
    fixture.records.remove(0);
    fixture.events[0] = pending;
    fixture.flush();
    assert_eq!(
        fixture
            .records
            .iter()
            .map(|record| record["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 4, 5]
    );
    assert!(!fixture.path().join("vacancies.jsonl").exists());

    rejected(fixture.inspect(), "pending_record_boundary");

    assert!(!fixture.path().join("vacancies.jsonl").exists());
}

#[test]
fn new_vacancy_allows_resolved_generated_envelope_above_physical_head() {
    let mut fixture = Fixture::new();
    fixture.events.push(pending_generated(&fixture, 6));
    fixture.flush();

    let report = fixture.inspect();
    assert!(report.eligible, "{:?}", report.rules);
    assert_eq!(report.committed_head_sequence, 5);
    assert!(!fixture.path().join("vacancies.jsonl").exists());
}

#[test]
fn existing_vacancy_rejects_pending_generated_at_or_below_authorized_head() {
    let mut fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    let pending = pending_generated(&fixture, 1);
    fixture.records.remove(0);
    fixture.events.remove(0);
    fixture.events.push(pending);
    fixture.flush();
    error_rule(
        validate_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "pending_record_boundary",
    );
}

#[test]
fn pending_generated_claiming_vacated_sequence_is_a_conflict() {
    let mut fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    fixture.events.push(pending_generated(&fixture, 3));
    fixture.flush();
    assert!(validate_vacancies(fixture.path(), ID).is_err());
}

#[test]
fn unowned_non_narrative_status_has_no_archival_sequence() {
    let mut fixture = Fixture::new();
    let mut status = pending_native("status");
    status["kind"] = json!("status");
    status["sequence"] = json!(3);
    fixture.events.push(status);
    fixture.flush();
    assert!(fixture.inspect().eligible);
}

#[test]
fn provider_mirror_request_root_alias_disqualifies_the_exact_sequence() {
    let mut fixture = Fixture::new();
    fixture.events.last_mut().unwrap()["metadata"]["provider_mirror_request_root_ids"] =
        json!(["wardian:input:3"]);
    fixture.flush();
    rejected(fixture.inspect(), "exact_reference_absent");
}

#[test]
fn provider_mirror_request_root_alias_added_after_authorization_conflicts() {
    let mut fixture = Fixture::new();
    let approved = fixture.authorization();
    fixture.ledger(&[approved]);
    // Record 5 is outside the immediate neighbour closures for vacancy 3.
    fixture.events.last_mut().unwrap()["metadata"]["provider_mirror_request_root_ids"] =
        json!(["wardian:input:3"]);
    fixture.flush();
    assert!(validate_vacancies(fixture.path(), ID).is_err());
}

#[test]
fn provider_mirror_request_root_aliases_reject_malformed_or_unresolved_evidence() {
    for value in [
        json!(null),
        json!("wardian:input:3"),
        json!([3]),
        json!([""]),
    ] {
        let mut fixture = Fixture::new();
        fixture.events.last_mut().unwrap()["metadata"]["provider_mirror_request_root_ids"] = value;
        fixture.flush();
        rejected(fixture.inspect(), "structured_evidence");
    }
    let mut fixture = Fixture::new();
    fixture.events.last_mut().unwrap()["metadata"]["provider_mirror_request_root_ids"] =
        json!(["wardian:input:999"]);
    fixture.flush();
    rejected(fixture.inspect(), "references_resolved");
}

#[test]
fn provider_observation_alias_at_vacancy_rejects_authorization_and_load() {
    for existing in [false, true] {
        let mut fixture = Fixture::new();
        if existing {
            fixture.ledger(&[fixture.authorization()]);
        }
        let event = fixture.events.last_mut().unwrap();
        event["provider"] = json!("codex");
        event["role"] = json!("assistant");
        event["source"] = json!("response_item");
        event["metadata"] = json!({
            "provider_log":true, "log_path":"/never/opened/provider",
            "provider_turn_id":"turn", "provider_observation_ids":["wardian:input:3"]
        });
        fixture.flush();
        if existing {
            error_rule(
                validate_vacancies(fixture.path(), ID).unwrap_err(),
                3,
                "exact_reference_absent",
            );
        } else {
            rejected(fixture.inspect(), "exact_reference_absent");
        }
    }
}

struct Fixture {
    temp: TempDir,
    records: Vec<Value>,
    events: Vec<Value>,
    sources: Vec<Value>,
    turns: Vec<Value>,
    manifest: Value,
}

fn write_rows(path: &Path, rows: &[Value]) {
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend(serde_json::to_vec(row).unwrap());
        bytes.push(b'\n');
    }
    fs::write(path, bytes).unwrap();
}

impl Fixture {
    fn new() -> Self {
        // Resolve only the synthetic temp parent (macOS /var may be a link).
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let temp = tempfile::tempdir_in(parent).unwrap();
        let mut records = Vec::new();
        let mut events = Vec::new();
        for seq in [1, 2, 4, 5] {
            let kind = match seq {
                2 => "tool_call",
                4 => "tool_result",
                _ => "message",
            };
            let role = if seq == 1 {
                "user"
            } else if seq == 5 {
                "assistant"
            } else {
                "tool"
            };
            records.push(json!({"schema":1,"seq":seq,"at":"2026-11-22T00:00:00Z",
                "kind":kind,"role":role,"text":format!("content {seq}"),
                "event_refs":[format!("e{seq}")],"source_refs":["s"],"artifact_refs":[]}));
            events.push(
                json!({"id":format!("e{seq}"),"session_id":"agent","provider":"test",
                "kind":kind,"role":role,"text":format!("content {seq}"),
                "source":"provider-channel","sequence":3,"metadata":{"tool_input":{"seq":3}}}),
            );
        }
        let sources = vec![json!({"schema":1,"source_id":"s","provider":"test",
            "source_kind":"provider_log","source_path":"/must/not/open/provider",
            "cursor":"3","offset":3,"row_id":"3"})];
        let turns = vec![json!({"schema":3,"conversation_id":ID,"turn_index":1,
            "turn_key":"turn1","status":"responded","status_source":"test",
            "seq_start":1,"seq_end":5,"started_at":"2026-11-22T00:00:00Z",
            "updated_at":"2026-11-22T00:00:01Z",
            "request":{"seq":1,"kind":"user_request","text":"3","text_truncated":false},
            "assistant_result":{"seq":5,"text":"3","text_truncated":false},
            "counts":{"records":4,"assistant_messages":1,"tool_calls":1,"tool_results":1,
                "nonzero_tool_results":0,"failed_tool_results":0,"timeouts":0},
            "tools_used":{"tool":3},"files":{"read":[],"written":[],"mentioned":[]},
            "external_side_effects":[],"failure_signals":[],
            "record_refs":{"seq_start":1,"seq_end":5,"event_count":4,"source_count":1,"artifact_count":0},
            "provider_refs":[]})];
        let manifest = json!({"schema":1,"conversation_id":ID,"agent_id":"agent",
            "agent_name":"agent","agent_class":"Coder","workspace":"/not/opened",
            "provider":"test","provider_session_ids":[],"effective_logging":"enabled",
            "created_at":"2026-11-22T00:00:00Z","updated_at":"2026-11-22T00:00:00Z",
            "closed_at":null,"status":"open","boundary_reason":"spawn",
            "format_versions":{"manifest":1,"conversation":1,"events":1,"sources":1,"turns":3},
            "record_count":4,"turn_count":1,"has_turns":true,"lifecycle_only":false});
        let fixture = Self {
            temp,
            records,
            events,
            sources,
            turns,
            manifest,
        };
        fixture.flush();
        fixture
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn flush(&self) {
        fs::write(
            self.path().join("manifest.json"),
            serde_json::to_vec(&self.manifest).unwrap(),
        )
        .unwrap();
        write_rows(&self.path().join("conversation.jsonl"), &self.records);
        write_rows(&self.path().join("events.jsonl"), &self.events);
        write_rows(&self.path().join("sources.jsonl"), &self.sources);
        write_rows(&self.path().join("turns.jsonl"), &self.turns);
    }

    fn inspect(&self) -> VacancyReport {
        inspect_vacancy(self.path(), ID, 3).unwrap()
    }

    fn authorization(&self) -> SequenceVacancy {
        let report = self.inspect();
        assert!(report.eligible, "{:?}", report.rules);
        SequenceVacancy {
            schema: 1,
            kind: "sequence_vacancy".into(),
            conversation_id: ID.into(),
            sequence: 3,
            evidence: report.evidence.unwrap(),
            authorization_id: "authorization".into(),
            approval_reference: "approval".into(),
            approval_text: "approved vacancy".into(),
            created_at: "2026-11-22T00:00:00Z".into(),
        }
    }

    fn ledger(&self, rows: &[SequenceVacancy]) {
        let mut bytes = Vec::new();
        for row in rows {
            bytes.extend(serde_json::to_vec(row).unwrap());
            bytes.push(b'\n');
        }
        fs::write(self.path().join("vacancies.jsonl"), bytes).unwrap();
    }

    fn body(&mut self, name: &str, bytes: &[u8]) {
        fs::create_dir_all(self.path().join("artifacts")).unwrap();
        fs::write(self.path().join("artifacts").join(name), bytes).unwrap();
        self.records[1]["artifact_refs"]
            .as_array_mut()
            .unwrap()
            .push(json!(name));
        self.records[1]["text"] = Value::Null;
        self.flush();
    }
}

fn rejected(report: VacancyReport, rule: &str) {
    assert!(!report.eligible);
    assert!(
        report.rules.iter().any(|r| r.rule == rule && !r.passed),
        "{:?}",
        report.rules
    );
    assert!(report.evidence.is_none());
}

fn error_rule(error: io::Error, sequence: u64, rule: &str) {
    let inner = error
        .get_ref()
        .unwrap()
        .downcast_ref::<VacancyConflict>()
        .expect("direct typed inner conflict");
    assert_eq!(inner.sequence, sequence);
    assert_eq!(inner.rule, rule);
    assert!(!error.to_string().contains("must/not"));
}

#[test]
fn sparse_spanning_turn_and_content_integers_are_not_membership_claims() {
    let fixture = Fixture::new();
    let report = fixture.inspect();
    assert!(report.eligible, "{:?}", report.rules);
    assert_eq!(report.committed_head_sequence, 5); // Four physical rows, head seq five.
    assert_eq!(fixture.turns[0]["counts"]["records"], 4);
    assert_eq!(fixture.records.len(), 4);
    assert!(report.rules.iter().all(|r| r.passed));
}

#[test]
fn inspector_and_missing_ledger_validation_are_read_only() {
    let fixture = Fixture::new();
    let before: Vec<_> = [
        "manifest.json",
        "conversation.jsonl",
        "events.jsonl",
        "sources.jsonl",
        "turns.jsonl",
    ]
    .map(|name| (name, fs::read(fixture.path().join(name)).unwrap()))
    .into_iter()
    .collect();
    assert!(fixture.inspect().eligible);
    assert!(read_vacancies(fixture.path(), ID).unwrap().is_empty());
    assert!(validate_vacancies(fixture.path(), ID).unwrap().is_empty());
    for (name, bytes) in before {
        assert_eq!(fs::read(fixture.path().join(name)).unwrap(), bytes);
    }
    assert!(!fixture.path().join("vacancies.jsonl").exists());
}

#[test]
fn missing_or_empty_ledger_never_reads_broken_narrative() {
    let fixture = Fixture::new();
    fs::write(fixture.path().join("conversation.jsonl"), b"invalid").unwrap();
    assert!(validate_vacancies(fixture.path(), ID).unwrap().is_empty());
    fs::write(fixture.path().join("vacancies.jsonl"), b"").unwrap();
    assert!(validate_vacancies(fixture.path(), ID).unwrap().is_empty());
}

#[test]
fn persisted_valid_vacancy_returns_only_reserved_sequence() {
    let fixture = Fixture::new();
    let row = fixture.authorization();
    fixture.ledger(std::slice::from_ref(&row));
    assert_eq!(read_vacancies(fixture.path(), ID).unwrap(), vec![row]);
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("conversation.jsonl"))
            .unwrap()
            .lines()
            .count(),
        4
    );
}

#[test]
fn exact_endpoints_request_answer_failure_and_tool_evidence_reject() {
    let cases = [
        ("seq_start", json!(3)),
        ("seq_end", json!(3)),
        ("record_refs", json!({"seq_start":3,"seq_end":5})),
        (
            "request",
            json!({"seq":3,"kind":"user_request","text":null,"text_truncated":false}),
        ),
        (
            "assistant_result",
            json!({"seq":3,"text":null,"text_truncated":false}),
        ),
        (
            "failure_signals",
            json!([{"kind":"tool_failure","seq":3,"tool":"tool"}]),
        ),
        (
            "external_side_effects",
            json!([{"kind":"file_write","evidence_seq":3,"summary":"done"}]),
        ),
    ];
    for (field, value) in cases {
        let mut fixture = Fixture::new();
        fixture.turns[0][field] = value;
        fixture.flush();
        rejected(fixture.inspect(), "exact_reference_absent");
    }
}

#[test]
fn compact_endpoint_aliases_are_recognized_and_conflicting_aliases_reject() {
    let mut fixture = Fixture::new();
    fixture.turns[0]["record_refs"] = json!({"conversation_seq_start":1,"conversation_seq_end":5});
    fixture.flush();
    assert!(fixture.inspect().eligible);
    fixture.turns[0]["record_refs"]["conversation_seq_start"] = json!(3);
    fixture.flush();
    rejected(fixture.inspect(), "exact_reference_absent");
    fixture.turns[0]["record_refs"]["seq_start"] = json!(1);
    fixture.flush();
    rejected(fixture.inspect(), "structured_evidence");
}

#[test]
fn bounds_use_persisted_maximum_and_reject_first_or_last_slot() {
    let fixture = Fixture::new();
    for seq in [0, 1, 5, 6, u64::MAX] {
        rejected(
            inspect_vacancy(fixture.path(), ID, seq).unwrap(),
            "sequence_bounds",
        );
    }
}

#[test]
fn immediate_physical_neighbours_cannot_be_replaced_by_farther_rows() {
    let mut fixture = Fixture::new();
    fixture.records.retain(|r| r["seq"] != 2);
    fixture.events.retain(|e| e["id"] != "e2");
    fixture.flush();
    rejected(fixture.inspect(), "physical_neighbours");
}

#[test]
fn real_record_later_is_typed_conflict() {
    let mut fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    let mut record = fixture.records[1].clone();
    record["seq"] = json!(3);
    record["event_refs"] = json!(["e3"]);
    let mut event = fixture.events[1].clone();
    event["id"] = json!("e3");
    fixture.records.push(record);
    fixture.events.push(event);
    fixture.flush();
    error_rule(
        validate_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "physical_sequence_absent",
    );
}

#[test]
fn changed_neighbour_body_bytes_invalidate_evidence() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"original");
    fixture.ledger(&[fixture.authorization()]);
    fs::write(fixture.path().join("artifacts/a.txt"), b"replacement").unwrap();
    error_rule(
        validate_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "neighbour_hashes_match",
    );
}

#[test]
fn swapping_two_owned_body_contents_invalidates_evidence() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"alpha");
    fixture.body("b.txt", b"beta");
    fixture.ledger(&[fixture.authorization()]);
    fs::write(fixture.path().join("artifacts/a.txt"), b"beta").unwrap();
    fs::write(fixture.path().join("artifacts/b.txt"), b"alpha").unwrap();
    error_rule(
        validate_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "neighbour_hashes_match",
    );
}

#[test]
fn same_content_in_two_owned_paths_retains_two_path_bindings() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"same");
    let single = fixture.authorization();
    fixture.body("b.txt", b"same");
    let both = fixture.authorization();
    assert_ne!(
        single.evidence.prev_record_hash,
        both.evidence.prev_record_hash
    );
}

#[test]
fn raw_direct_event_and_source_changes_invalidate_closure() {
    for file in ["events.jsonl", "sources.jsonl", "conversation.jsonl"] {
        let fixture = Fixture::new();
        fixture.ledger(&[fixture.authorization()]);
        let path = fixture.path().join(file);
        let bytes = fs::read(&path).unwrap();
        // Whitespace changes parsed semantics neither, but changes exact physical evidence.
        let bytes: Vec<_> = bytes
            .split_inclusive(|b| *b == b'\n')
            .flat_map(|line| {
                let mut line = line.to_vec();
                line.insert(0, b' ');
                line
            })
            .collect();
        fs::write(path, bytes).unwrap();
        error_rule(
            validate_vacancies(fixture.path(), ID).unwrap_err(),
            3,
            "neighbour_hashes_match",
        );
    }
}

#[test]
fn future_append_changes_audit_digest_but_keeps_existing_neighbour_proof_valid() {
    let mut fixture = Fixture::new();
    let row = fixture.authorization();
    fixture.ledger(std::slice::from_ref(&row));
    let mut record = fixture.records[3].clone();
    record["seq"] = json!(6);
    record["event_refs"] = json!(["e6"]);
    let mut event = fixture.events[3].clone();
    event["id"] = json!("e6");
    fixture.records.push(record);
    fixture.events.push(event);
    fixture.manifest["record_count"] = json!(5);
    fixture.turns[0]["seq_end"] = json!(6);
    fixture.turns[0]["record_refs"]["seq_end"] = json!(6);
    fixture.flush();
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
    let now = fixture.inspect().evidence.unwrap();
    assert_eq!(now.committed_head_sequence, 6);
    assert_eq!(now.prev_record_hash, row.evidence.prev_record_hash);
    assert_eq!(now.next_record_hash, row.evidence.next_record_hash);
    assert_ne!(now.scan_digest, row.evidence.scan_digest);
}

#[test]
fn generated_event_claim_at_hole_is_not_a_neighbour_or_absence_proof() {
    let mut fixture = Fixture::new();
    let mut record = fixture.records[1].clone();
    record["seq"] = json!(3);
    record["event_refs"] = json!([format!("generated:{ID}:3")]);
    let mut event = fixture.events[1].clone();
    event["id"] = json!(format!("generated:{ID}:3"));
    event["metadata"] = json!({"generated":true,"archive_record":record});
    fixture.events.push(event);
    fixture.flush();
    rejected(fixture.inspect(), "exact_reference_absent");
}

#[test]
fn legacy_generated_alias_and_accepted_input_root_are_scanned() {
    for field in ["legacy_event_ids", "request_root_id"] {
        let mut fixture = Fixture::new();
        fixture.events[0]["metadata"][field] = if field == "legacy_event_ids" {
            json!([format!("generated:{ID}:3")])
        } else {
            json!("wardian:input:3")
        };
        fixture.flush();
        rejected(fixture.inspect(), "exact_reference_absent");
    }
}

#[test]
fn qualified_wardian_cursor_is_seq_while_provider_cursor_and_native_step_are_not() {
    let mut fixture = Fixture::new();
    fixture.sources[0]["provider"] = json!("antigravity");
    fixture.sources[0]["source_kind"] = json!("conversation_database");
    fixture.sources[0]["cursor"] = json!("3");
    fixture.flush();
    assert!(fixture.inspect().eligible);
    fixture.sources[0]["provider"] = json!("wardian");
    fixture.sources[0]["source_kind"] = json!("wardian_agent");
    fixture.flush();
    rejected(fixture.inspect(), "exact_reference_absent");
}

#[test]
fn unknown_and_old_schemas_and_structural_extensions_fail_closed() {
    for target in ["manifest", "narrative", "source", "turn", "metadata"] {
        let mut fixture = Fixture::new();
        match target {
            "manifest" => fixture.manifest["format_versions"]["turns"] = json!(1),
            "narrative" => fixture.records[1]["schema"] = json!(2),
            "source" => fixture.sources[0]["schema"] = json!(2),
            "turn" => fixture.turns[0]["schema"] = json!(2),
            _ => fixture.events[0]["metadata"]["unknown_archive_seq"] = json!(3),
        }
        fixture.flush();
        rejected(fixture.inspect(), "schema_supported");
    }
    let mut fixture = Fixture::new();
    fixture.turns[0]["request"]["future_seq"] = json!(3);
    fixture.flush();
    rejected(fixture.inspect(), "schema_supported");
}

#[test]
fn duplicate_json_keys_and_malformed_structural_metadata_fail_closed() {
    let fixture = Fixture::new();
    let path = fixture.path().join("conversation.jsonl");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1),
    )
    .unwrap();
    rejected(fixture.inspect(), "structured_evidence");
    let mut fixture = Fixture::new();
    fixture.events[0]["metadata"]["archive_record"] = json!("bad");
    fixture.flush();
    rejected(fixture.inspect(), "structured_evidence");
}

#[test]
fn unknown_files_or_unowned_bodies_prevent_global_absence_proof() {
    let fixture = Fixture::new();
    fs::write(fixture.path().join("unknown.jsonl"), b"{}\n").unwrap();
    rejected(fixture.inspect(), "conversation_files_known");
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join("artifacts")).unwrap();
    fs::write(fixture.path().join("artifacts/orphan.txt"), b"unowned").unwrap();
    rejected(fixture.inspect(), "bodies_resolved");
}

#[test]
fn missing_canonical_file_duplicate_owner_and_dangling_ref_fail_closed() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.path().join("sources.jsonl")).unwrap();
    rejected(fixture.inspect(), "canonical_files_present");
    let mut fixture = Fixture::new();
    fixture.records.push(fixture.records[0].clone());
    fixture.flush();
    rejected(fixture.inspect(), "ownership_unique");
    let mut fixture = Fixture::new();
    fixture.records[0]["event_refs"] = json!(["missing"]);
    fixture.flush();
    rejected(fixture.inspect(), "references_resolved");
}

#[test]
fn traversal_body_pointer_rejects_without_opening_target() {
    for path in [
        "../secret",
        "artifacts/../secret",
        "C:/secret",
        "/secret",
        "a/b",
    ] {
        let mut fixture = Fixture::new();
        fixture.records[1]["artifact_refs"] = json!([path]);
        fixture.flush();
        rejected(fixture.inspect(), "conversation_contained");
    }
}

#[test]
fn legacy_backslash_body_reference_stays_inside_the_conversation() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"owned body");
    fixture.records[1]["artifact_refs"] = json!(["artifacts\\a.txt"]);
    fixture.flush();
    assert!(fixture.inspect().eligible);
}

#[test]
fn malformed_torn_blank_crlf_and_duplicate_ledger_rows_reject() {
    for bytes in [
        b"{".as_slice(),
        b"{}\n".as_slice(),
        b"\n".as_slice(),
        b"{}\r\n".as_slice(),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path().join("vacancies.jsonl"), bytes).unwrap();
        let error = read_vacancies(fixture.path(), ID).unwrap_err();
        assert!(error.get_ref().unwrap().is::<VacancyConflict>());
    }
    let fixture = Fixture::new();
    let row = fixture.authorization();
    fixture.ledger(&[row.clone(), row]);
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "ledger_unique",
    );
}

#[test]
fn malformed_tail_after_valid_ledger_is_never_silently_discarded() {
    let fixture = Fixture::new();
    fixture.ledger(&[fixture.authorization()]);
    let path = fixture.path().join("vacancies.jsonl");
    let valid = fs::read(&path).unwrap();
    for suffix in [b"{".as_slice(), b"{}\n".as_slice()] {
        let mut changed = valid.clone();
        changed.extend(suffix);
        fs::write(&path, changed).unwrap();
        assert!(validate_vacancies(fixture.path(), ID).is_err());
    }
}

#[test]
fn ledger_record_cap_and_line_cap_include_lf() {
    let fixture = Fixture::new();
    let row = fixture.authorization();
    let mut rows = Vec::new();
    for i in 0..MAX_VACANCIES {
        let mut next = row.clone();
        next.sequence = i as u64 + 2;
        next.evidence.committed_head_sequence = 1000;
        next.authorization_id = format!("approval-{i}");
        rows.push(next);
    }
    fixture.ledger(&rows);
    assert_eq!(read_vacancies(fixture.path(), ID).unwrap().len(), 64);
    let mut extra = rows[0].clone();
    extra.sequence = 100;
    extra.authorization_id = "extra".into();
    rows.push(extra);
    fixture.ledger(&rows);
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        100,
        "ledger_record_limit",
    );
    let mut exact = row;
    let length = serde_json::to_vec(&exact).unwrap().len() + 1;
    exact
        .approval_text
        .push_str(&"x".repeat(MAX_VACANCY_LINE_BYTES - length));
    fixture.ledger(std::slice::from_ref(&exact));
    assert_eq!(
        fs::read(fixture.path().join("vacancies.jsonl"))
            .unwrap()
            .len(),
        4096
    );
    assert_eq!(read_vacancies(fixture.path(), ID).unwrap().len(), 1);
    exact.approval_text.push('x');
    fixture.ledger(&[exact]);
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        0,
        "ledger_line_limit",
    );
}

#[test]
fn ledger_identity_schema_authorization_hash_and_timestamp_rules() {
    let fixture = Fixture::new();
    let row = fixture.authorization();
    let cases = [
        ("conversation_id", json!("other"), "conversation_identity"),
        ("schema", json!(2), "ledger_schema"),
        ("kind", json!("future"), "ledger_schema"),
        ("sequence", json!(1), "sequence_bounds"),
        ("approval_reference", json!(" "), "authorization_complete"),
        ("approval_text", json!(""), "authorization_complete"),
        ("created_at", json!("not-a-date"), "authorization_timestamp"),
    ];
    for (field, value, rule) in cases {
        let mut encoded = serde_json::to_value(&row).unwrap();
        encoded[field] = value;
        write_rows(&fixture.path().join("vacancies.jsonl"), &[encoded]);
        let error = read_vacancies(fixture.path(), ID).unwrap_err();
        let inner = error
            .get_ref()
            .unwrap()
            .downcast_ref::<VacancyConflict>()
            .unwrap();
        assert_eq!(inner.rule, rule);
    }
    let mut bad = row;
    bad.evidence.prev_record_hash = "not-a-hash".into();
    fixture.ledger(&[bad]);
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "evidence_hash_encoding",
    );
}

#[test]
fn report_never_exposes_paths_body_or_approval_contents() {
    let mut fixture = Fixture::new();
    fixture.events[0]["metadata"]["future_seq"] = json!(3);
    fixture.flush();
    let encoded = serde_json::to_string(&fixture.inspect()).unwrap();
    for forbidden in [
        "must/not/open",
        "content 1",
        "tool_input",
        "provider-channel",
        "approval",
    ] {
        assert!(!encoded.contains(forbidden), "{encoded}");
    }
    let encoded =
        serde_json::to_string(&inspect_vacancy(fixture.path(), "../secret", 3).unwrap()).unwrap();
    assert!(!encoded.contains("../secret"));
}

#[cfg(unix)]
#[test]
fn symlink_file_artifact_and_ancestor_are_never_followed() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), b"outside").unwrap();
    fs::remove_file(fixture.path().join("events.jsonl")).unwrap();
    symlink(
        outside.path().join("secret"),
        fixture.path().join("events.jsonl"),
    )
    .unwrap();
    assert!(!fixture.inspect().eligible);
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"inside");
    fs::remove_file(fixture.path().join("artifacts/a.txt")).unwrap();
    symlink(
        outside.path().join("secret"),
        fixture.path().join("artifacts/a.txt"),
    )
    .unwrap();
    assert!(!fixture.inspect().eligible);
    let fixture = Fixture::new();
    let alias = outside.path().join("alias");
    symlink(fixture.path(), &alias).unwrap();
    assert!(!inspect_vacancy(&alias, ID, 3).unwrap().eligible);
}

#[cfg(windows)]
#[test]
fn windows_junction_artifact_directory_and_ancestor_are_rejected() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("a.txt"), b"outside").unwrap();
    junction::create(outside.path(), fixture.path().join("artifacts")).unwrap();
    assert!(!fixture.inspect().eligible);
    fs::remove_dir(fixture.path().join("artifacts")).unwrap();
    let alias = outside.path().join("alias");
    junction::create(fixture.path(), &alias).unwrap();
    assert!(!inspect_vacancy(&alias, ID, 3).unwrap().eligible);
    // Remove the junction itself; never recursively remove its target.
    fs::remove_dir(alias).unwrap();
}

#[test]
fn recoverable_future_generated_record_sources_and_bodies_do_not_block_vacancy() {
    let mut fixture = Fixture::new();
    let authorization = fixture.authorization();
    fixture.ledger(std::slice::from_ref(&authorization));
    let id = format!("generated:{ID}:6");
    let mut payload = fixture.records[0].clone();
    payload["seq"] = json!(6);
    payload["request_root_id"] = json!("wardian:input:6");
    payload["event_refs"] = json!([id]);
    payload["source_refs"] = json!(["src_6"]);
    payload["artifact_refs"] = json!(["future.txt"]);
    payload["text"] = Value::Null;
    let mut event = fixture.events[0].clone();
    event["id"] = json!(id);
    event["source"] = json!("src_6");
    event["metadata"] = json!({"generated":true,"archive_record":payload});
    fixture.events.push(event);
    fixture
        .sources
        .push(json!({"schema":1,"source_id":"src_6","provider":"wardian",
        "source_kind":"wardian_agent","cursor":"6"}));
    fs::create_dir(fixture.path().join("artifacts")).unwrap();
    fs::write(
        fixture.path().join("artifacts/future.txt"),
        b"durable future input",
    )
    .unwrap();
    let mut future_turn = fixture.turns[0].clone();
    future_turn["turn_index"] = json!(2);
    future_turn["turn_key"] = json!("future-turn");
    future_turn["seq_start"] = json!(6);
    future_turn["seq_end"] = json!(6);
    future_turn["request"]["seq"] = json!(6);
    future_turn["assistant_result"] = Value::Null;
    future_turn["record_refs"]["seq_start"] = json!(6);
    future_turn["record_refs"]["seq_end"] = json!(6);
    fixture.turns.push(future_turn);
    fixture.flush();
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
    let report = fixture.inspect();
    assert!(report.eligible, "{:?}", report.rules);
    assert_eq!(report.committed_head_sequence, 5);
    assert_eq!(
        report.evidence.unwrap().prev_record_hash,
        authorization.evidence.prev_record_hash
    );
    assert!(!fixture.records.iter().any(|r| r["seq"] == 6));
    fixture.events.last_mut().unwrap()["id"] = json!("unbound-future");
    fixture.flush();
    assert!(!fixture.inspect().eligible);
}

#[test]
fn valid_generated_copy_is_never_a_physical_neighbour() {
    let mut fixture = Fixture::new();
    let mut payload = fixture.records[1].clone();
    let id = format!("generated:{ID}:2");
    payload["event_refs"] = json!([id]);
    let mut event = fixture.events[1].clone();
    event["id"] = json!(id);
    event["metadata"] = json!({"generated":true,"archive_record":payload});
    fixture.records.remove(1);
    fixture.events[1] = event;
    fixture.flush();
    // Historical generated copies fail before physical-neighbour inspection.
    rejected(fixture.inspect(), "pending_record_boundary");
}

#[test]
fn codex_retirement_is_resolved_and_never_becomes_physical_occupancy() {
    let mut fixture = Fixture::new();
    fixture.events[0]["provider"] = json!("codex");
    fixture.events[0]["source"] = json!("response_item");
    fixture.events[0]["metadata"] = json!({"provider_log":true,"input_origin":"human_input",
        "input_purpose":"request","raw_type":"message","provider_turn_id":"turn",
        "log_path":"/never/opened/provider","request_root_id":"root-request"});
    let mut mirror = fixture.events[0].clone();
    mirror["id"] = json!("mirror");
    mirror["source"] = json!("event_msg");
    mirror["metadata"]["raw_type"] = json!("user_message");
    mirror["metadata"]["request_root_id"] = json!("root-mirror");
    mirror["metadata"]["wardian_archive_coalesced_narrative_sequences"] = json!([7]);
    fixture.events.push(mirror);
    fixture.records[0]["event_refs"] = json!(["e1", "mirror"]);
    fixture.flush();
    let report = fixture.inspect();
    assert!(report.eligible, "{:?}", report.rules);
    assert_eq!(report.committed_head_sequence, 5);
    fixture.events.last_mut().unwrap()["metadata"]
        ["wardian_archive_coalesced_narrative_sequences"] = json!([3]);
    fixture.flush();
    rejected(fixture.inspect(), "reservation_absent");
    fixture.events.last_mut().unwrap()["metadata"]["raw_type"] = json!("not-a-mirror");
    fixture.flush();
    rejected(fixture.inspect(), "reservation_proven");
}

#[test]
fn missing_canonical_files_duplicate_owners_and_dangling_references_fail_closed() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.path().join("turns.jsonl")).unwrap();
    rejected(fixture.inspect(), "canonical_files_present");
    let mut fixture = Fixture::new();
    fixture.records.push(fixture.records[1].clone());
    fixture.flush();
    rejected(fixture.inspect(), "ownership_unique");
    let mut fixture = Fixture::new();
    fixture.records[1]["event_refs"] = json!(["unresolved"]);
    fixture.flush();
    rejected(fixture.inspect(), "references_resolved");
}

#[test]
fn missing_body_unsafe_pointer_and_unknown_nested_fields_fail_closed() {
    let mut fixture = Fixture::new();
    fixture.records[1]["artifact_refs"] = json!(["missing.txt"]);
    fixture.flush();
    rejected(fixture.inspect(), "bodies_resolved");
    fixture.records[1]["artifact_refs"] = json!(["../outside"]);
    fixture.flush();
    rejected(fixture.inspect(), "conversation_contained");
    fixture.records[1]["artifact_refs"] = json!([]);
    fixture.turns[0]["counts"]["future_seq"] = json!(3);
    fixture.flush();
    rejected(fixture.inspect(), "schema_supported");
}

#[test]
fn legacy_body_spelling_normalizes_to_one_path() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"body");
    fixture.records[1]["artifact_refs"] = json!(["artifacts\\a.txt"]);
    fixture.flush();
    assert!(fixture.inspect().eligible);
}

#[test]
fn ledger_duplicates_torn_lines_unknown_fields_and_audit_hash_rules() {
    let fixture = Fixture::new();
    let row = fixture.authorization();
    fixture.ledger(&[row.clone(), row.clone()]);
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        3,
        "ledger_unique",
    );
    let mut bytes = serde_json::to_vec(&row).unwrap();
    fs::write(fixture.path().join("vacancies.jsonl"), &bytes).unwrap();
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        0,
        "ledger_lf_terminated",
    );
    bytes.extend(b"\n{malformed}\n");
    fs::write(fixture.path().join("vacancies.jsonl"), bytes).unwrap();
    error_rule(
        read_vacancies(fixture.path(), ID).unwrap_err(),
        0,
        "structured_evidence",
    );
    let mut value = serde_json::to_value(&row).unwrap();
    value["evidence"]["unknown"] = json!("field");
    write_rows(&fixture.path().join("vacancies.jsonl"), &[value]);
    assert!(read_vacancies(fixture.path(), ID).is_err());

    // Well-formed scan_digest is audit-only even if no longer equal to the cohort.
    let mut changed = row;
    changed.evidence.scan_digest = "a".repeat(64);
    fixture.ledger(&[changed]);
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
}

#[test]
fn shared_sender_sources_bind_to_each_narrative_owner_cursor() {
    let mut fixture = Fixture::new();
    fixture.records[1]["source_refs"] = json!(["agent:sender"]);
    fixture.records[3]["source_refs"] = json!(["agent:sender"]);
    fixture.sources.push(
        json!({"schema":1,"source_id":"agent:sender","provider":"wardian",
        "provider_session_id":"sender","source_kind":"wardian_agent","cursor":"2"}),
    );
    fixture.sources.push(
        json!({"schema":1,"source_id":"agent:sender","provider":"wardian",
        "provider_session_id":"sender","source_kind":"wardian_agent","cursor":"5"}),
    );
    fixture.flush();
    let authorization = fixture.authorization();
    fixture.ledger(&[authorization]);
    assert_eq!(
        validate_vacancies(fixture.path(), ID).unwrap(),
        BTreeSet::from([3])
    );
    let duplicate = fixture.sources.last().unwrap().clone();
    fixture.sources.push(duplicate);
    fixture.flush();
    rejected(fixture.inspect(), "ownership_unique");
}

#[test]
fn absent_conversation_directory_has_no_ledger_and_does_not_scan() {
    let fixture = Fixture::new();
    assert!(validate_vacancies(&fixture.path().join("missing"), ID)
        .unwrap()
        .is_empty());
    assert!(!fixture.path().join("missing").exists());
}

#[test]
fn replay_body_matching_uses_contained_content() {
    let mut fixture = Fixture::new();
    fixture.body("a.txt", b"stored body");
    assert!(body_matches_text(fixture.path(), "a.txt", "stored body").unwrap());
    assert!(!body_matches_text(fixture.path(), "artifacts/a.txt", "changed body").unwrap());
    assert!(body_matches_text(fixture.path(), "../outside", "stored body").is_err());
}

fn make_owned_legacy_generated(fixture: &mut Fixture, number: u64) -> String {
    let record = fixture
        .records
        .iter_mut()
        .find(|record| record["seq"] == number)
        .unwrap();
    let old = record["event_refs"][0].as_str().unwrap().to_owned();
    let id = format!("generated:{ID}:{number}");
    record["event_refs"] = json!([id]);
    let event = fixture
        .events
        .iter_mut()
        .find(|event| event["id"] == old)
        .unwrap();
    event["id"] = json!(id);
    event["metadata"] = json!({"generated":true});
    id
}

#[test]
fn physically_owned_legacy_generated_rows_need_no_recovery_envelope() {
    for number in [2, 5] {
        let mut fixture = Fixture::new();
        make_owned_legacy_generated(&mut fixture, number);
        fixture.flush();
        assert!(fixture.inspect().eligible);
        fixture.ledger(&[fixture.authorization()]);
        assert_eq!(
            validate_vacancies(fixture.path(), ID).unwrap(),
            BTreeSet::from([3])
        );
    }
}

#[test]
fn legacy_generated_rows_cannot_supply_missing_or_mismatched_physical_owners() {
    let mut fixture = Fixture::new();
    let mut unowned = pending_native(&format!("generated:{ID}:3"));
    unowned["metadata"] = json!({"generated":true});
    fixture.events.push(unowned);
    fixture.flush();
    rejected(fixture.inspect(), "legacy_generated_owner_proven");

    let mut fixture = Fixture::new();
    let id = make_owned_legacy_generated(&mut fixture, 2);
    fixture.records[1]["event_refs"] = json!([format!("generated:{ID}:3")]);
    fixture
        .events
        .iter_mut()
        .find(|event| event["id"] == id)
        .unwrap()["id"] = json!(format!("generated:{ID}:3"));
    fixture.flush();
    rejected(fixture.inspect(), "legacy_generated_owner_proven");
}

#[test]
fn legacy_generated_rows_keep_strict_envelope_and_reference_checks() {
    let mut fixture = Fixture::new();
    let id = make_owned_legacy_generated(&mut fixture, 2);
    fixture
        .events
        .iter_mut()
        .find(|event| event["id"] == id)
        .unwrap()["metadata"]["archive_record"] = Value::Null;
    fixture.flush();
    rejected(fixture.inspect(), "structured_evidence");
    for field in ["request_root_id", "causal_ref"] {
        let mut fixture = Fixture::new();
        let id = make_owned_legacy_generated(&mut fixture, 2);
        fixture
            .events
            .iter_mut()
            .find(|event| event["id"] == id)
            .unwrap()["metadata"][field] = json!(3);
        fixture.flush();
        rejected(fixture.inspect(), "structured_evidence");
    }
    let mut fixture = Fixture::new();
    let id = make_owned_legacy_generated(&mut fixture, 2);
    fixture
        .events
        .iter_mut()
        .find(|event| event["id"] == id)
        .unwrap()["metadata"]["provider_mirror_request_root_ids"] = json!(["wardian:input:3"]);
    fixture.flush();
    rejected(fixture.inspect(), "exact_reference_absent");
}
