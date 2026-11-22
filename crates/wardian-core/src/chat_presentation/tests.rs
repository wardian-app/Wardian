use super::*;
use crate::models::chat::AgentChatPage;
use serde_json::json;

fn wire() -> ChatPresentation {
    serde_json::from_str(include_str!("fixtures/presentation-v1.json")).unwrap()
}

#[test]
fn wire_fixture_round_trips_with_pending_content_without_a_canonical_receipt() {
    let envelope = wire();
    let value = envelope.decode_v1().unwrap().unwrap();
    assert_eq!(value.items[0].provenance, ChatItemProvenance::Pending);
    assert_eq!(
        value.items[0].submission_status,
        Some(ChatSubmissionStatus::Queued)
    );
    assert!(value.items[0].event_refs.is_empty());
    assert!(!value.items[0].preview.as_ref().unwrap().text.is_empty());
    let restored: ChatPresentation =
        serde_json::from_slice(&serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert_eq!(restored.decode_v1().unwrap(), Some(value));
}

#[test]
fn unknown_versions_stay_opaque_while_malformed_v1_fails_closed() {
    for payload in [
        json!(["future kind"]),
        json!(null),
        json!({"different_shape": true}),
    ] {
        let future: ChatPresentation =
            serde_json::from_value(json!({"version": 9000, "payload": payload})).unwrap();
        assert_eq!(future.decode_v1().unwrap(), None);
    }
    let malformed = ChatPresentation {
        version: 1,
        payload: json!(["invalid v1"]),
    };
    assert!(malformed.decode_v1().is_err());
}

#[test]
fn preview_bridge_and_running_prefix_cannot_grant_final_authority() {
    let mut value = wire().decode_v1().unwrap().unwrap();
    value.preview_bridge = Some(ChatPreviewBridge {
        admission_id: "recent-owner".into(),
        canonical_head: "a".repeat(64),
        replaces: None,
    });
    value.turns[0].prefix_attributed = true;
    value.turns[0].membership_complete = false;
    value.turns[0].status = ChatTurnStatus::Running;
    value.turns[0].counts_complete = false;
    value.turns[0].references_complete = false;
    value.turns[0].answer_recorded = false;
    value.turns[0].answer_item_keys.clear();
    assert!(value.validate().is_ok());
    for field in ["counts", "references", "completed", "answer", "head"] {
        let mut invalid = value.clone();
        match field {
            "counts" => invalid.turns[0].counts_complete = true,
            "references" => invalid.turns[0].references_complete = true,
            "completed" => invalid.turns[0].status = ChatTurnStatus::Completed,
            "answer" => invalid.turns[0].answer_recorded = true,
            _ => invalid.preview_bridge.as_mut().unwrap().canonical_head = "invented".into(),
        }
        assert!(invalid.validate().is_err(), "{field}");
    }
}

#[test]
fn legacy_page_omits_capability_and_future_capability_does_not_break_events() {
    let mut value = json!({
        "session_id": "fixture-agent", "conversation_id": "fixture-conversation",
        "generation": "fixture-generation", "source_epoch": null,
        "revision": "fixture-revision", "events": [], "next_before": null,
        "unchanged": false, "reset": false, "progress": "ready", "aliases": [],
        "removed_ids": [], "detail": null, "bytes_read": 0, "records_decoded": 0
    });
    let page: AgentChatPage = serde_json::from_value(value.clone()).unwrap();
    assert!(page.presentation.is_none());
    assert!(serde_json::to_value(page)
        .unwrap()
        .get("presentation")
        .is_none());
    value["presentation"] = json!({"version": 2, "payload": null});
    let page: AgentChatPage = serde_json::from_value(value).unwrap();
    assert_eq!(page.presentation.unwrap().decode_v1().unwrap(), None);
}

#[test]
fn duplicated_keys_order_and_dangling_ownership_are_rejected() {
    for change in ["item", "turn", "order", "owner", "prompt_ref", "answer_ref"] {
        let mut value = wire().decode_v1().unwrap().unwrap();
        match change {
            "item" => value.items.push(value.items[0].clone()),
            "turn" => value.turns.push(value.turns[0].clone()),
            "order" => {
                let mut other = value.items[0].clone();
                other.item_key = "item:other".into();
                value.items.push(other);
            }
            "owner" => value.items[0].turn_key = "unowned-turn".into(),
            "prompt_ref" => value.turns[0].prompt_item_keys = vec!["unowned-prompt".into()],
            _ => value.turns[0].answer_item_keys = vec![value.items[0].item_key.clone()],
        }
        assert!(value.validate().is_err(), "{change}");
    }
}

#[test]
fn pending_provisional_and_canonical_require_distinct_evidence_shapes() {
    for change in [
        "no_admission",
        "no_content",
        "no_status",
        "no_identity",
        "source_pending",
        "canonical_no_ref",
        "canonical_admission",
    ] {
        let mut value = wire().decode_v1().unwrap().unwrap();
        let item = &mut value.items[0];
        match change {
            "no_admission" => item.admission_id = None,
            "no_content" => item.preview = None,
            "no_status" => item.submission_status = None,
            "no_identity" => {
                item.evidence = ChatEvidenceIdentity::Submission {
                    submission_id: String::new(),
                }
            }
            "source_pending" => {
                item.evidence = ChatEvidenceIdentity::Source {
                    source_key: "source".into(),
                    source_epoch: "epoch".into(),
                    record_key: "record".into(),
                }
            }
            "canonical_no_ref" => {
                item.provenance = ChatItemProvenance::Canonical;
                item.admission_id = None;
            }
            _ => {
                item.provenance = ChatItemProvenance::Canonical;
                item.event_refs = vec!["canonical-event".into()];
            }
        }
        assert!(value.validate().is_err(), "{change}");
    }
    for state in [
        ChatSubmissionStatus::Delivered,
        ChatSubmissionStatus::Failed,
        ChatSubmissionStatus::Cancelled,
        ChatSubmissionStatus::Uncertain,
    ] {
        let mut value = wire().decode_v1().unwrap().unwrap();
        value.items[0].submission_status = Some(state);
        assert!(
            value.validate().is_ok(),
            "schema preserves terminal and uncertain delivery distinctions"
        );
    }
}

#[test]
fn header_and_wire_byte_budgets_include_provisional_content() {
    let mut value = wire();
    let template = value.payload["items"][0].clone();
    value.payload["items"] = json!((0..81).map(|_| template.clone()).collect::<Vec<_>>());
    assert!(value.decode_v1().unwrap_err().contains("header budget"));
    let mut value = wire();
    value.payload["items"][0]["preview"]["text"] = json!("x".repeat(1025));
    assert!(value.decode_v1().is_err());
    let mut value = wire();
    value.payload["extra"] = json!("x".repeat(CHAT_PRESENTATION_MAX_BYTES));
    assert!(value.decode_v1().unwrap_err().contains("byte budget"));
}

fn detail() -> ChatStepDetail {
    ChatStepDetail {
        step_key: "step:fixture".into(),
        revision: "fixture-revision".into(),
        text: "{\"tabs\":[]}".into(),
        input_text: None,
        input_truncated: false,
        result_summary: None,
        format: ChatOutputFormat::Json,
        pending: false,
        unavailable: false,
        complete: true,
        truncated_before: false,
        total_bytes: Some(11),
    }
}

#[test]
fn legacy_groups_select_one_presentation_and_keep_unknown_membership_partial() {
    let mut value = wire().decode_v1().unwrap().unwrap();
    value.items[0].provenance = ChatItemProvenance::Canonical;
    value.items[0].admission_id = None;
    value.items[0].event_refs = vec!["new-event".into()];
    value.legacy_turns.push(ChatLegacyTurn {
        turn_key: "earlier:fixture".into(),
        order_key: "0:earlier".into(),
        event_refs: vec!["old-event".into()],
        kind: ChatLegacyTurnKind::EarlierActivity,
        membership_complete: false,
    });
    value.validate().unwrap();
    let mut mixed = value.clone();
    mixed.legacy_turns[0].turn_key = mixed.turns[0].turn_key.clone();
    assert!(mixed.validate().is_err());
    let mut duplicate = value.clone();
    duplicate.legacy_turns[0]
        .event_refs
        .push("new-event".into());
    assert!(duplicate.validate().is_err());
    let mut invented = value.clone();
    invented.legacy_turns[0].membership_complete = true;
    assert!(invented.validate().is_err());
    value.legacy_turns[0].event_refs = (0..80).map(|i| format!("old-{i}")).collect();
    assert!(value.validate().unwrap_err().contains("header budget"));
}

#[test]
fn detail_allows_only_complete_bounded_json_and_honest_tail_metadata() {
    let mut value = wire().decode_v1().unwrap().unwrap();
    value.detail = Some(detail());
    value.validate().unwrap();
    for change in [
        "clipped_json",
        "pending_json",
        "invalid_json",
        "oversized",
        "impossible_size",
        "unavailable_complete",
    ] {
        let mut invalid = value.clone();
        let detail = invalid.detail.as_mut().unwrap();
        match change {
            "clipped_json" => detail.truncated_before = true,
            "pending_json" => {
                detail.pending = true;
                detail.complete = false;
            }
            "invalid_json" => detail.text = "invalid".into(),
            "oversized" => {
                detail.text = "é".repeat(8193);
                detail.format = ChatOutputFormat::Text;
                detail.total_bytes = None;
            }
            "impossible_size" => detail.total_bytes = Some(1),
            _ => detail.unavailable = true,
        }
        assert!(invalid.validate().is_err(), "{change}");
    }
    let tail = value.detail.as_mut().unwrap();
    tail.text = "é".repeat(8192);
    tail.format = ChatOutputFormat::Text;
    tail.complete = false;
    tail.truncated_before = true;
    tail.total_bytes = Some(32_768);
    value.validate().unwrap();
}
