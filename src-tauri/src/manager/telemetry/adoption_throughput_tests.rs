//! Opt-in synthetic throughput ruler (`adoption-throughput` feature).
//! No provider or production import is used.
use super::*;
use crate::state::conversation_archive::adoption::profile::{Mark, Profile};
use wardian_core::control::{ArchiveAdoptionNativeIdentity, ArchiveAdoptionRequest};

const ROWS: usize = 3072;
const BODY_BYTES: usize = 8192;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_throughput_synthetic_fixed_range() {
    let home = crate::control::test_support::TestWardianHome::new_async().await;
    let (state, session, path) = registered_off_source(&home).await;
    dispatch_ordinary_capture(&state).await;
    let snapshot = crate::commands::chat::agent_archive_capture_snapshot(&state, &session)
        .await
        .unwrap();
    let original = capture_state(&state, &snapshot);
    let start = original.committed_offset;
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(&snapshot);
    state
        .conversation_archive
        .append_delivered_input_with_context(context, "Synthetic benchmark anchor", None)
        .unwrap();
    let conversation = state
        .conversation_archive
        .active_conversation_id(&session)
        .unwrap()
        .unwrap();

    let retired = path.with_extension("synthetic-old-source");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::copy(&retired, &path).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    let padding = "x".repeat(BODY_BYTES);
    for row in 0..ROWS {
        writeln!(file, "{}", serde_json::json!({
            "type":"event_msg", "timestamp":"2026-10-04T12:00:00Z",
            "payload":{"type":"user_message", "message":format!("Synthetic adoption row {row:05}: {padding}")}
        })).unwrap();
    }
    file.sync_all().unwrap();
    let selected_end = file.metadata().unwrap().len();
    // Pin the end inside a partial record, then complete it after approval.
    file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Excluded partial").unwrap();
    file.sync_all().unwrap();
    let numeric_end = file.metadata().unwrap().len();
    drop(file);
    dispatch_ordinary_capture(&state).await;
    assert_eq!(
        capture_state(&state, &snapshot).reason.as_deref(),
        Some("provider_log_source_replaced")
    );
    let identity = crate::commands::provider_log_acquisition::native_file_identity(
        &std::fs::File::open(&path).unwrap(),
    )
    .unwrap();
    let authorization = uuid::Uuid::new_v4().to_string();
    let prepare_start = std::time::Instant::now();
    let response = crate::commands::chat::authorize_archive_adoption(
        &state,
        &session,
        ArchiveAdoptionRequest {
            authorization_id: authorization.clone(),
            conversation_id: conversation.clone(),
            provider_session: snapshot.resume_session.clone().unwrap(),
            source_path: std::fs::canonicalize(&path)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            start_offset: start,
            end_offset: numeric_end,
            native_identity: ArchiveAdoptionNativeIdentity {
                platform: identity.platform,
                primary: identity.primary,
                secondary: identity.secondary,
            },
            approval_reference: "synthetic-throughput-fixture".into(),
            approval_text: "Import only this synthetic fixed interval into the isolated archive"
                .into(),
        },
        None,
        "synthetic-test-operator".into(),
    )
    .await
    .unwrap();
    let prepare_us = prepare_start.elapsed().as_micros();
    assert_eq!(response.end_offset, selected_end);
    let mut later = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    later.write_all(b" completed after approval\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Excluded later append\"}}\n").unwrap();
    later.sync_all().unwrap();
    drop(later);
    let root = wardian_core::paths::agent_conversations_dir(&session).unwrap();
    let work_path = root
        .parent()
        .unwrap()
        .join("archive-adoptions")
        .join(format!("{authorization}.json"));
    let initial: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&work_path).unwrap()).unwrap();
    let chunks = initial["grant"]["selection"]["chunks"]
        .as_array()
        .unwrap()
        .len();
    let request = crate::commands::chat::background_capture_request(&state, &session)
        .await
        .unwrap()
        .unwrap();
    let claim =
        crate::commands::background_capture::admit_background_capture(&state, request, false)
            .await
            .expect("approved unchanged-source adoption admits one owner");
    let trace_enabled = std::env::var("WARDIAN_ADOPTION_PROFILE").as_deref() == Ok("1");
    let profile = trace_enabled.then(|| Profile::start(&session));
    eprintln!(
        "SYNTHETIC_ADOPTION_BEGIN {}",
        serde_json::json!({"bytes":selected_end-start,"rows":ROWS,"chunks":chunks,"trace":trace_enabled,"debug_assertions":cfg!(debug_assertions)})
    );
    let started = std::time::Instant::now();
    crate::commands::background_capture::run_background_capture(&state, claim).await;
    let elapsed_us = started.elapsed().as_micros();
    let marks = profile.map(Profile::finish).unwrap_or_default();
    eprintln!("SYNTHETIC_ADOPTION_WORKER_DONE {elapsed_us}");
    report_marks(&marks);
    let work: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&work_path).unwrap()).unwrap();
    assert_eq!(
        work["status"], "complete",
        "synthetic import must complete, not retire or fail quickly"
    );
    assert_eq!(work["cursor"].as_u64(), Some(selected_end));
    let events = state
        .conversation_archive
        .chat_events_for_agent(&session)
        .unwrap();
    let imported: Vec<_> = events
        .iter()
        .filter(|event| {
            event.metadata["archive_adoption"]["authorization_id"].as_str() == Some(&authorization)
        })
        .collect();
    assert_eq!(
        imported.len(),
        ROWS,
        "every selected record must have a durable owned event"
    );
    let mut imported_rows = std::collections::HashSet::new();
    for event in &imported {
        let text = archived_body(&root, &conversation, event);
        let row: usize = text
            .strip_prefix("Synthetic adoption row ")
            .and_then(|text| text.split_once(": "))
            .unwrap()
            .0
            .parse()
            .unwrap();
        assert!(row < ROWS && imported_rows.insert(row));
        assert_eq!(text, format!("Synthetic adoption row {row:05}: {padding}"));
    }
    assert!(!state
        .conversation_archive
        .chat_projection
        .has_pending_capture_work(&session));
    let page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        None,
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert!(
        page.archive_read_ready,
        "the full saved reader must be ready"
    );
    assert_eq!(page.conversation_id.as_deref(), Some(conversation.as_str()));
    assert!(page.source_epoch.is_none());
    let owned = page
        .events
        .iter()
        .find(|row| imported.iter().any(|event| event.id == row.id))
        .expect("the normal page must expose an imported row");
    let detail_ref = owned.metadata["chat_detail_ref"]
        .as_str()
        .expect("the imported body must have a normal detail reference");
    let detail_page = crate::commands::chat::load_agent_chat_page_with_recent_for_state(
        &state,
        session.clone(),
        None,
        None,
        Some(detail_ref.into()),
        None,
        crate::commands::chat::RecentChatRead::default(),
    )
    .await
    .unwrap();
    assert!(detail_page.archive_read_ready);
    let detail = detail_page.detail.unwrap();
    assert!(detail.complete && detail.next.is_none());
    assert_eq!(detail.event_id, owned.id);
    let expected = imported.iter().find(|event| event.id == owned.id).unwrap();
    assert_eq!(detail.text, archived_body(&root, &conversation, expected));
    let after = capture_state(&state, &snapshot);
    assert_eq!(
        after.committed_offset, start,
        "adoption cannot renew native capture"
    );
    assert_eq!(after.native_identity, original.native_identity);
    eprintln!(
        "SYNTHETIC_ADOPTION_RESULT {}",
        serde_json::json!({
            "bytes": selected_end-start, "rows":imported.len(), "chunks":chunks,
            "elapsed_us":elapsed_us, "prepare_us":prepare_us,
            "bytes_per_second":(selected_end-start) as f64 / (elapsed_us as f64 / 1_000_000.0),
            "trace":trace_enabled,"status":work["status"],"cursor":work["cursor"],"end":selected_end,
            "marks":marks.len(), "failures":0,
            "archive_read_ready":page.archive_read_ready,"detail_complete":detail.complete
        })
    );
}

fn archived_body(
    root: &std::path::Path,
    conversation: &str,
    event: &wardian_core::models::chat::AgentChatEvent,
) -> String {
    event.text.clone().unwrap_or_else(|| {
        let artifact = event.metadata["text_artifact_refs"]
            .as_array()
            .filter(|refs| refs.len() == 1)
            .and_then(|refs| refs[0].as_str())
            .expect("large body must have one durable artifact");
        std::fs::read_to_string(root.join(conversation).join("artifacts").join(artifact)).unwrap()
    })
}

fn report_marks(marks: &[Mark]) {
    if marks.is_empty() {
        return;
    }
    let mut previous_checkpoint = None;
    let mut begin = None;
    let mut loaded = None;
    let mut prior = None;
    let mut phases = std::collections::BTreeMap::<&str, u128>::new();
    for mark in marks {
        if mark.phase == "adoption_begin" {
            begin = Some(mark.at);
            prior = Some(mark.at);
            loaded = None;
            phases.clear();
        } else if mark.phase == "adoption_idle" {
            begin = None;
        } else if mark.phase.starts_with("adoption_") {
            if let Some(prior) = prior.replace(mark.at) {
                phases.insert(mark.phase, mark.at.saturating_sub(prior).as_micros());
            }
            if mark.phase == "adoption_loaded" {
                loaded = Some(mark.cursor);
            }
            if mark.phase == "adoption_checkpoint" {
                let begin = begin.expect("step begin");
                eprintln!(
                    "SYNTHETIC_ADOPTION_STEP {}",
                    serde_json::json!({
                        "cursor":mark.cursor, "bytes":mark.cursor-loaded.unwrap(),
                        "wall_us":mark.at.saturating_sub(begin).as_micros(),
                        "gap_us":previous_checkpoint.map(|last| begin.saturating_sub(last).as_micros()),
                        "phases_us":phases
                    })
                );
                previous_checkpoint = Some(mark.at);
            }
        }
    }
    let passes = marks
        .iter()
        .filter(|mark| mark.phase == "pass_begin")
        .count();
    eprintln!("SYNTHETIC_ADOPTION_PASSES {passes}");
}
