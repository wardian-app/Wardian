//! Cold, derived-only admission under the existing capture owner's gates.
//! Every pass reads one bounded JSONL window. Immutable roots and cursors live
//! in the published head, so a restart never mistakes a partial index for ready.
use super::*;
#[cfg(test)]
use wardian_core::conversations::{
    ConversationIndexEntry, ConversationLoggingSetting, ConversationManifest,
    ConversationNarrativeRecord, ConversationStatus,
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: String,
    identity: crate::commands::provider_log_acquisition::ProviderLogNativeIdentity,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Progress {
    #[serde(default)]
    selection_scope: Option<SelectionScope>,
    index: Stamp,
    manifest: Stamp,
    narrative: Stamp,
    events: Stamp,
    narrative_before: u64,
    event_before: u64,
    proofs: Option<String>,
    bodies: Option<String>,
    body_before: Option<String>,
    #[serde(default)]
    archive_display: Option<super::super::chat_logical_index::Index>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SelectionScope {
    agent_id: String,
    provider: String,
    provider_source_key: Option<String>,
    provider_session_ids: Vec<String>,
}

#[cfg(test)]
#[path = "chat_archive_display_tests.rs"]
mod archive_display_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_saved_window_bounds_rows_and_rejects_giant_envelopes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let text = (0..100)
            .map(|i| format!("{{\"i\":{i}}}\n"))
            .collect::<String>();
        std::fs::write(&path, &text).unwrap();
        let rows = window(&path, text.len() as u64).unwrap();
        assert_eq!(rows.len(), ADMIT_ROWS);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&rows[0].1).unwrap()["i"],
            99
        );
        assert!(rows.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() <= WINDOW_BYTES as usize);
        std::fs::write(&path, vec![b'x'; WINDOW_BYTES as usize + 1]).unwrap();
        assert!(window(&path, WINDOW_BYTES + 1)
            .unwrap_err()
            .to_string()
            .contains("exceeds bootstrap window"));
    }

    #[test]
    fn cold_saved_attempted_object_writes_have_a_byte_budget() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::checkpoint_writer(temp.path());
        let bytes = vec![0; BODY_BYTES];
        for _ in 0..512 {
            store.put_bytes(&bytes).unwrap();
        }
        assert!(store
            .put_bytes(&bytes)
            .unwrap_err()
            .to_string()
            .contains("write budget exhausted"));
    }

    #[test]
    fn cold_selection_partial_flush_preserves_pointer_and_restarts() {
        use crate::state::conversation_archive::tests::isolated_home;
        use wardian_core::conversations::ConversationBoundaryReason;

        let (_guard, _temp) = isolated_home();
        let context = ConversationArchiveContext::for_agent_id("cold-selection", "unknown");
        let entries = (0..26)
            .map(|index| ConversationIndexEntry {
                schema: 1,
                conversation_id: format!("closed-{index}"),
                agent_id: context.agent_id.clone(),
                agent_name: context.agent_name.clone(),
                agent_class: String::new(),
                workspace: String::new(),
                provider: context.provider.clone(),
                provider_session_ids: Vec::new(),
                started_at: String::new(),
                ended_at: None,
                status: ConversationStatus::Closed,
                boundary_reason: ConversationBoundaryReason::Shutdown,
                first_prompt_excerpt: None,
                last_record_excerpt: None,
                record_count: 0,
                turn_count: 0,
                has_turns: false,
                lifecycle_only: false,
                artifact_count: 0,
                path: String::new(),
            })
            .collect::<Vec<_>>();
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        std::fs::create_dir_all(index_path.parent().unwrap()).unwrap();
        let journal = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap() + "\n")
            .collect::<String>();
        std::fs::write(&index_path, journal).unwrap();
        let objects = locations(&context.agent_id).unwrap().1;
        let mut first = Store::checkpoint_writer(&objects);
        let (selected, pending) = select(&context, &mut first).unwrap();
        assert!(selected.is_none() && pending);
        drop(first);
        let pointer = selection_path(&context).unwrap();
        let prior_pointer = std::fs::read(&pointer).unwrap();
        let prior_ref: Option<String> = bounded_json(&pointer).unwrap();
        let mut restart = Store::new(&objects);
        let prior: Selection = restart.read(prior_ref.as_ref().unwrap()).unwrap();
        assert_eq!(restart.page(&prior.seen, None, 80).unwrap().len(), 24);

        // Locate the final node for a real filesystem flush collision. The
        // previous selection stays published while some new children flush.
        let mut rehearsal = Store::checkpoint_writer(&objects);
        let mut final_root = prior.seen.clone();
        for entry in entries[..2].iter().rev() {
            let marker = rehearsal.put(&true).unwrap();
            final_root = Some(
                rehearsal
                    .insert(
                        &final_root,
                        &digest(entry.conversation_id.as_bytes()),
                        &marker,
                    )
                    .unwrap(),
            );
        }
        let blocked = objects.join(final_root.as_ref().unwrap());
        assert!(!blocked.exists());
        drop(rehearsal);
        std::fs::create_dir(&blocked).unwrap();
        let files = || {
            std::fs::read_dir(&objects)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
                .count()
        };
        let before = files();
        let mut failing = Store::checkpoint_writer(&objects);
        assert!(select(&context, &mut failing).is_err());
        assert!(files() > before, "the failure must follow a partial flush");
        assert_eq!(std::fs::read(&pointer).unwrap(), prior_pointer);
        assert_eq!(
            Store::new(&objects)
                .page(&prior.seen, None, 80)
                .unwrap()
                .len(),
            24
        );
        drop(failing);
        std::fs::remove_dir(&blocked).unwrap();

        let mut retry = Store::checkpoint_writer(&objects);
        let (selected, pending) = select(&context, &mut retry).unwrap();
        assert!(selected.is_none() && !pending);
        drop(retry);
        let reference: Option<String> = bounded_json(&pointer).unwrap();
        let mut reader = Store::new(&objects);
        let saved: Selection = reader.read(reference.as_ref().unwrap()).unwrap();
        assert_eq!(saved.before, 0);
        assert_eq!(reader.page(&saved.seen, None, 80).unwrap().len(), 26);
        for entry in entries {
            assert!(reader
                .get(&saved.seen, &digest(entry.conversation_id.as_bytes()))
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn cold_body_jobs_keep_private_chunks_reachable_without_a_display_row() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        let mut first = Store::checkpoint_writer(&objects);
        let chunk = first.put_bytes(b"first chunk").unwrap();
        let body_root = first.insert(&None, "00000000000000000000", &chunk).unwrap();
        let mut job = BodyJob {
            event_start: 0,
            event_bytes: 1,
            row: Row {
                agent_id: "cold-agent".into(),
                conversation_id: "saved-conversation".into(),
                key: "00000000000000000000".into(),
                event: serde_json::from_value(json!({
                    "id":"owned-body", "session_id":"cold-agent",
                    "provider":"unknown", "kind":"message"
                }))
                .unwrap(),
                body: Some(Body {
                    root: Some(body_root.clone()),
                    ..Default::default()
                }),
                relation: None,
                removed_display_ids: Vec::new(),
            },
            offset: 11,
        };
        let reference = job.put(&mut first).unwrap();
        let jobs = first.insert(&None, &job.row.key, &reference).unwrap();
        assert!(!objects.join(&body_root).exists());
        first
            .finish_checkpoint(&[Dependency::Node(jobs.clone())])
            .unwrap();
        drop(first);
        let mut restart = Store::checkpoint_writer(&objects);
        let reference = restart.get(&Some(jobs), &job.row.key).unwrap().unwrap();
        job = restart.read(&reference).unwrap();
        assert_eq!(
            job.row.body.as_ref().unwrap().root.as_ref(),
            Some(&body_root)
        );
        assert_eq!(
            restart
                .get(&Some(body_root.clone()), "00000000000000000000")
                .unwrap(),
            Some(chunk)
        );
        let next_chunk = restart.put_bytes(b"next chunk").unwrap();
        let next_root = restart
            .insert(&Some(body_root), "00000000000000000011", &next_chunk)
            .unwrap();
        job.row.body.as_mut().unwrap().root = Some(next_root.clone());
        job.offset = 21;
        let next_job = job.put(&mut restart).unwrap();
        let jobs = restart.insert(&None, &job.row.key, &next_job).unwrap();
        assert!(!objects.join(&next_root).exists());
        restart
            .finish_checkpoint(&[Dependency::Node(jobs.clone())])
            .unwrap();
        drop(restart);
        let mut reader = Store::new(&objects);
        let reference = reader.get(&Some(jobs), &job.row.key).unwrap().unwrap();
        let saved: BodyJob = reader.read(&reference).unwrap();
        assert_eq!(saved.offset, 21);
        assert_eq!(
            saved.row.body.as_ref().unwrap().root.as_ref(),
            Some(&next_root)
        );
        assert_eq!(reader.page(&Some(next_root), None, 2).unwrap().len(), 2);
    }

    fn cold_body_drain_fixture(
        body_padding: &[usize],
        envelope_padding: &[usize],
    ) -> (
        ConversationArchiveContext,
        Vec<Row>,
        HashMap<String, String>,
    ) {
        assert_eq!(body_padding.len(), envelope_padding.len());
        let context = ConversationArchiveContext::for_agent_id("cold-body-drain", "codex");
        let conversation_id = "saved-body-drain";
        let directory =
            super::super::super::storage::conversation_dir(&context.agent_id, conversation_id)
                .unwrap();
        fs::create_dir_all(directory.join("artifacts")).unwrap();
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        fs::write(&index_path, b"").unwrap();
        fs::write(directory.join("manifest.json"), b"{}").unwrap();
        fs::write(directory.join("conversation.jsonl"), b"").unwrap();

        let (head_path, objects) = locations(&context.agent_id).unwrap();
        let mut store = Store::checkpoint_writer(&objects);
        let candidate = candidate(&context, conversation_id);
        let mut envelopes = Vec::new();
        let mut rows = Vec::new();
        let mut jobs = None;
        let mut expected = HashMap::new();
        for (index, &body_bytes) in body_padding.iter().enumerate() {
            let id = format!("cold-body-{index}");
            let artifact = format!("body-{index}.txt");
            let text = format!("Body {index}: {}", "x".repeat(body_bytes));
            fs::write(directory.join("artifacts").join(&artifact), text.as_bytes()).unwrap();
            let event: AgentChatEvent = serde_json::from_value(json!({
                "id": id,
                "session_id": context.agent_id,
                "provider": "codex",
                "kind": "message",
                "role": "user",
                "text": format!("Body {index}"),
                "metadata": {"text_artifact_refs": [artifact], "text_excerpt": true}
            }))
            .unwrap();
            let event_start = envelopes.len() as u64;
            let mut envelope = serde_json::to_vec(&event).unwrap();
            envelope.resize(envelope.len() + envelope_padding[index], b' ');
            envelope.push(b'\n');
            assert!(envelope.len() <= WINDOW_BYTES as usize);
            let source = body_source(&candidate, &event).unwrap().unwrap();
            let row = Row {
                agent_id: context.agent_id.clone(),
                conversation_id: conversation_id.into(),
                key: format!("{event_start:020}"),
                event: header(&event),
                body: Some(Body {
                    binding: source.binding().unwrap(),
                    ..Default::default()
                }),
                relation: None,
                removed_display_ids: Vec::new(),
            };
            let job = BodyJob {
                event_start,
                event_bytes: envelope.len() as u64,
                row: row.clone(),
                offset: 0,
            }
            .put(&mut store)
            .unwrap();
            jobs = Some(store.insert(&jobs, &row.key, &job).unwrap());
            envelopes.extend(envelope);
            expected.insert(event.id.clone(), text);
            rows.push(row);
        }
        let event_path = directory.join("events.jsonl");
        fs::write(&event_path, &envelopes).unwrap();

        // Seed an admitted cohort with metadata exhausted, isolating body drain
        // from selection, proof scanning, header admission and relation work.
        let mut archive_display = super::super::super::chat_logical_index::Index::default();
        archive_display.qualify_narratives(envelopes.len() as u64);
        let mut saved = Head {
            agent_id: context.agent_id.clone(),
            conversation_id: conversation_id.into(),
            source_key: None,
            generation: "body-drain-generation".into(),
            source_epoch: None,
            root: None,
            identities: None,
            logical: Default::default(),
            turns: Default::default(),
            committed_output_bytes: envelopes.len() as u64,
            capture_stamp: "saved-archive".into(),
            progress: "indexing".into(),
            source_coverage: None,
            parent: None,
            changes: Vec::new(),
            recent_start: 0,
            row_count: envelopes.len(),
            recent_page: None,
            bootstrap: Some(Progress {
                selection_scope: Some(SelectionScope::new(&context)),
                index: Stamp::read(&index_path).unwrap(),
                manifest: Stamp::read(&directory.join("manifest.json")).unwrap(),
                narrative: Stamp::read(&directory.join("conversation.jsonl")).unwrap(),
                events: Stamp::read(&event_path).unwrap(),
                narrative_before: 0,
                event_before: 0,
                proofs: None,
                bodies: jobs,
                body_before: None,
                archive_display: Some(archive_display),
            }),
        };
        for row in &rows {
            insert_row(&mut store, &mut saved, row).unwrap();
        }
        store
            .finish_checkpoint(&saved.checkpoint_dependencies())
            .unwrap();
        let reference = store.put(&saved).unwrap();
        write_json_atomic(&head_path, &reference).unwrap();
        drop(store);
        (context, rows, expected)
    }

    fn cold_body_drain_rows(context: &ConversationArchiveContext) -> (Head, Vec<Row>) {
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let published: Head = reader
            .read(&head(&context.agent_id).unwrap().unwrap())
            .unwrap();
        let rows = reader
            .page(&published.root, None, PAGE_ROWS)
            .unwrap()
            .into_iter()
            .map(|(_, reference)| reader.read(&reference).unwrap())
            .collect();
        (published, rows)
    }

    fn assert_cold_body_drain_payloads(
        context: &ConversationArchiveContext,
        expected: &HashMap<String, String>,
    ) {
        let (_, rows) = cold_body_drain_rows(context);
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        for (id, text) in expected {
            let row = rows.iter().find(|row| &row.event.id == id).unwrap();
            let body = row.body.as_ref().unwrap();
            assert!(body.complete);
            assert_eq!(body.bytes, text.len() as u64);
            let chunks = reader.page(&body.root, None, PAGE_ROWS).unwrap();
            let mut actual = Vec::new();
            for (_, reference) in chunks.into_iter().rev() {
                actual.extend(reader.read_bytes(&reference, BODY_BYTES).unwrap());
            }
            assert_eq!(actual.as_slice(), text.as_bytes());
        }
    }

    #[test]
    fn cold_body_drain_envelope_budget_waits_without_skipping_a_large_valid_job() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) =
            cold_body_drain_fixture(&[8 * 1024; 3], &[0, WINDOW_BYTES as usize - 2048, 4096]);
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let initial: Head = reader
            .read(&head(&context.agent_id).unwrap().unwrap())
            .unwrap();
        let jobs = reader
            .page(&initial.bootstrap.as_ref().unwrap().bodies, None, 3)
            .unwrap()
            .into_iter()
            .map(|(_, reference)| reader.read::<BodyJob>(&reference).unwrap())
            .collect::<Vec<_>>();
        assert!(jobs[1].event_bytes <= WINDOW_BYTES);
        assert!(jobs[0].event_bytes + jobs[1].event_bytes > WINDOW_BYTES);
        assert!(jobs[0].event_bytes + jobs[2].event_bytes < WINDOW_BYTES);
        drop(reader);

        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        assert_eq!(
            partial.bootstrap.as_ref().unwrap().body_before.as_ref(),
            Some(&rows[2].key)
        );
        for row in published {
            assert_eq!(
                row.body.as_ref().unwrap().complete,
                row.event.id == rows[2].event.id
            );
        }
        // The deferred envelope is now first and receives the full allowance.
        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_invalid_coordinates_error_before_envelope_budget_deferral() {
        use crate::state::conversation_archive::tests::isolated_home;

        for oversized in [false, true] {
            let (_guard, temp) = isolated_home();
            let (context, rows, expected) =
                cold_body_drain_fixture(&[8 * 1024; 3], &[0, WINDOW_BYTES as usize - 2048, 4096]);
            let (head_path, objects) = locations(&context.agent_id).unwrap();
            assert!(head_path.starts_with(temp.path()));
            let mut writer = Store::checkpoint_writer(&objects);
            let mut saved: Head = writer
                .read(&head(&context.agent_id).unwrap().unwrap())
                .unwrap();
            let progress = saved.bootstrap.as_mut().unwrap();
            let key = &rows[1].key;
            let reference = writer.get(&progress.bodies, key).unwrap().unwrap();
            let mut job: BodyJob = writer.read(&reference).unwrap();
            if oversized {
                job.event_bytes = WINDOW_BYTES + 1;
            } else {
                job.event_start = progress.events.length;
            }
            let reference = job.put(&mut writer).unwrap();
            progress.bodies = Some(writer.insert(&progress.bodies, key, &reference).unwrap());
            writer
                .finish_checkpoint(&saved.checkpoint_dependencies())
                .unwrap();
            let reference = writer.put(&saved).unwrap();
            write_json_atomic(&head_path, &reference).unwrap();
            drop(writer);
            let before = fs::read(&head_path).unwrap();

            assert!(advance(&context)
                .unwrap_err()
                .to_string()
                .contains("invalid saved chat body coordinates"));
            assert_eq!(fs::read(&head_path).unwrap(), before);
            assert!(objects
                .join(digest(expected[&rows[2].event.id].as_bytes()))
                .is_file());
        }
    }

    #[test]
    fn cold_body_drain_shares_chunk_budget_and_restarts_the_first_partial_job() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(
            &[8 * 1024, (BODY_CHUNKS_PER_PASS + 1) * BODY_BYTES, 8 * 1024],
            &[0; 3],
        );
        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        let middle = published
            .iter()
            .find(|row| row.event.id == rows[1].event.id)
            .unwrap();
        assert!(!middle.body.as_ref().unwrap().complete);
        assert_eq!(
            middle.body.as_ref().unwrap().bytes,
            (BODY_CHUNKS_PER_PASS - 1) as u64 * BODY_BYTES as u64
        );
        assert!(
            published
                .iter()
                .map(|row| row.body.as_ref().unwrap().bytes)
                .sum::<u64>()
                <= BODY_CHUNKS_PER_PASS as u64 * BODY_BYTES as u64
        );
        assert!(
            !published
                .iter()
                .find(|row| row.event.id == rows[0].event.id)
                .unwrap()
                .body
                .as_ref()
                .unwrap()
                .complete
        );
        let progress = partial.bootstrap.as_ref().unwrap();
        assert_eq!(progress.body_before.as_ref(), Some(&rows[2].key));
        let mut reader = Store::new(&locations(&context.agent_id).unwrap().1);
        let reference = reader.get(&progress.bodies, &rows[1].key).unwrap().unwrap();
        let saved: BodyJob = reader.read(&reference).unwrap();
        assert_eq!(
            saved.offset,
            (BODY_CHUNKS_PER_PASS - 1) as u64 * BODY_BYTES as u64
        );
        drop(reader);

        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_changed_second_source_preserves_the_previous_checkpoint() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; 3], &[0; 3]);
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &rows[1].conversation_id,
        )
        .unwrap();
        // A changed length makes the binding mismatch deterministic even on
        // filesystems whose modification timestamps have coarse resolution.
        fs::write(directory.join("artifacts/body-1.txt"), b"changed source").unwrap();
        let (head_path, objects) = locations(&context.agent_id).unwrap();
        let before = fs::read(&head_path).unwrap();
        assert!(advance(&context)
            .unwrap_err()
            .to_string()
            .contains("saved chat body changed"));
        assert_eq!(fs::read(&head_path).unwrap(), before);
        assert!(objects
            .join(digest(expected[&rows[2].event.id].as_bytes()))
            .is_file());
    }

    #[test]
    fn cold_body_drain_missing_artifact_keeps_binding_and_obeys_job_limit() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, mut expected) = cold_body_drain_fixture(
            &[8 * 1024; BODY_JOBS_PER_PASS + 1],
            &[0; BODY_JOBS_PER_PASS + 1],
        );
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &rows[BODY_JOBS_PER_PASS].conversation_id,
        )
        .unwrap();
        fs::remove_file(directory.join(format!("artifacts/body-{BODY_JOBS_PER_PASS}.txt")))
            .unwrap();
        assert!(advance(&context).unwrap());
        let (partial, published) = cold_body_drain_rows(&context);
        assert_eq!(
            partial.bootstrap.as_ref().unwrap().body_before.as_ref(),
            Some(&rows[1].key)
        );
        let missing = published
            .iter()
            .find(|row| row.event.id == rows[BODY_JOBS_PER_PASS].event.id)
            .unwrap();
        assert_eq!(missing.event.metadata["chat_body_unavailable"], true);
        assert_eq!(
            missing.body.as_ref().unwrap().binding,
            rows[BODY_JOBS_PER_PASS].body.as_ref().unwrap().binding
        );
        assert!(!missing.body.as_ref().unwrap().complete);
        assert!(
            !published
                .iter()
                .find(|row| row.event.id == rows[0].event.id)
                .unwrap()
                .body
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(!advance(&context).unwrap());
        expected.remove(&rows[BODY_JOBS_PER_PASS].event.id);
        assert_cold_body_drain_payloads(&context, &expected);
    }

    #[test]
    fn cold_body_drain_packs_bounded_short_jobs_per_checkpoint_across_retry_and_restart() {
        use crate::state::conversation_archive::tests::isolated_home;
        use std::collections::HashSet;

        const JOBS: usize = 33;
        const CHUNKS_PER_CHECKPOINT: usize = 16;

        let (_guard, temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; JOBS], &[0; JOBS]);
        let (head_path, objects) = locations(&context.agent_id).unwrap();
        assert!(expected.values().all(|text| text.len() <= BODY_BYTES));
        let generation = cold_body_drain_rows(&context).0.generation;

        // Fail the second job after an earlier chunk has been persisted. The
        // whole unpublished batch must retry from the previous head/cursor.
        let second = &rows[JOBS - 2];
        let blocked = objects.join(digest(expected[&second.event.id].as_bytes()));
        assert!(blocked.starts_with(temp.path()));
        assert!(!blocked.exists());
        fs::create_dir(&blocked).unwrap();
        let initial_pointer = fs::read(&head_path).unwrap();
        assert!(advance(&context).is_err());
        let first = rows.last().unwrap();
        assert!(objects
            .join(digest(expected[&first.event.id].as_bytes()))
            .is_file());
        assert_eq!(fs::read(&head_path).unwrap(), initial_pointer);
        fs::remove_dir(&blocked).unwrap();

        let mut completed = HashSet::new();
        let mut cursor = None;
        let mut productive_checkpoints = 0;
        let mut settled = false;
        for _ in 0..=JOBS {
            let pending = advance(&context).unwrap();
            // Reopen all persisted state after every checkpoint. No owner,
            // queued file handle or transient Store survives a restart.
            let mut reader = Store::new(&objects);
            let reference = head(&context.agent_id).unwrap().unwrap();
            let current: Head = reader.read(&reference).unwrap();
            assert_eq!(current.generation, generation);
            let progress = current.bootstrap.as_ref().unwrap();
            assert_eq!(progress.narrative_before, 0);
            assert_eq!(progress.event_before, 0);
            let mut next_completed = HashSet::new();
            let mut completed_bytes = 0;
            let published_rows = reader.page(&current.root, None, JOBS + 1).unwrap();
            assert_eq!(published_rows.len(), JOBS);
            for (_, reference) in published_rows {
                let row: Row = reader.read(&reference).unwrap();
                let body = row.body.as_ref().unwrap();
                if !body.complete {
                    continue;
                }
                let chunks = reader.page(&body.root, None, 2).unwrap();
                assert_eq!(chunks.len(), 1, "a one-chunk body must not be duplicated");
                assert_eq!(chunks[0].0, format!("{:020}", 0));
                let actual = reader.read_bytes(&chunks[0].1, BODY_BYTES).unwrap();
                assert_eq!(actual.as_slice(), expected[&row.event.id].as_bytes());
                assert_eq!(body.bytes, actual.len() as u64);
                if !completed.contains(&row.event.id) {
                    completed_bytes += actual.len();
                }
                next_completed.insert(row.event.id);
            }
            assert!(next_completed.is_superset(&completed));
            let finished = next_completed.len() - completed.len();
            assert!(finished <= CHUNKS_PER_CHECKPOINT);
            assert!(completed_bytes <= CHUNKS_PER_CHECKPOINT * BODY_BYTES);
            if finished > 0 {
                productive_checkpoints += 1;
                let next_cursor = progress.body_before.as_ref().unwrap();
                if let Some(previous) = &cursor {
                    assert!(
                        next_cursor < previous,
                        "body cursor must move only backward"
                    );
                }
            } else {
                assert_eq!(progress.body_before, cursor);
            }
            cursor = progress.body_before.clone();
            completed = next_completed;
            if !pending {
                assert_eq!(current.progress, "ready");
                assert!(reader
                    .page(&progress.bodies, progress.body_before.as_deref(), 1)
                    .unwrap()
                    .is_empty());
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "the finite body queue must settle without restarting"
        );
        assert_eq!(
            completed.len(),
            JOBS,
            "every artifact body must recover in full"
        );
        let settled_pointer = fs::read(&head_path).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&head_path).unwrap(), settled_pointer);
        let maximum = JOBS.div_ceil(CHUNKS_PER_CHECKPOINT);
        assert!(
            productive_checkpoints <= maximum,
            "{JOBS} one-chunk bodies took {productive_checkpoints} productive checkpoints \
             (maximum {maximum}); \
             the bounded body allowance must be shared across body jobs"
        );
    }

    fn cold_body_boundary_records(rows: &[Row]) -> Vec<ConversationNarrativeRecord> {
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let mut record =
                    super::super::super::narrative_from_chat_event(&row.event, index as u64 + 1)
                        .unwrap();
                record.at = "2026-01-01T00:00:00Z".into();
                record.artifact_refs = vec![format!("body-{index}.txt")];
                record
            })
            .collect()
    }

    // Replace the minimal body-only fixture's placeholders with a physically
    // bound saved cohort before exercising normal reads or metadata admission.
    fn cold_body_boundary_cohort(
        context: &ConversationArchiveContext,
        saved: &mut Head,
        records: &[ConversationNarrativeRecord],
    ) {
        use wardian_core::conversations::{ConversationBoundaryReason, ConversationFormatVersions};

        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let manifest = ConversationManifest {
            schema: 1,
            conversation_id: saved.conversation_id.clone(),
            agent_id: context.agent_id.clone(),
            agent_name: context.agent_name.clone(),
            agent_class: context.agent_class.clone(),
            workspace: context.workspace.clone(),
            provider: context.provider.clone(),
            provider_session_ids: context.provider_session_ids.clone(),
            provider_source_key: context.provider_source_key.clone(),
            provider_native_refs: Vec::new(),
            effective_logging: ConversationLoggingSetting::Enabled,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            closed_at: None,
            status: ConversationStatus::Open,
            boundary_reason: ConversationBoundaryReason::Spawn,
            format_versions: ConversationFormatVersions::default(),
            record_count: records.len() as u64,
            turn_count: 0,
            has_turns: false,
            lifecycle_only: false,
        };
        let entry = super::super::super::storage::index_entry_from_manifest(
            &manifest,
            None,
            None,
            None,
            records.len() as u64,
            records
                .iter()
                .map(|record| record.artifact_refs.len() as u64)
                .sum(),
        );
        let index_path = super::super::super::storage::index_path(&context.agent_id).unwrap();
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(&index_path, serde_json::to_string(&entry).unwrap() + "\n").unwrap();
        fs::write(
            directory.join("conversation.jsonl"),
            records
                .iter()
                .map(|record| serde_json::to_string(record).unwrap() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let progress = saved.bootstrap.as_mut().unwrap();
        progress.selection_scope = Some(SelectionScope::new(context));
        progress.index = Stamp::read(&index_path).unwrap();
        progress.manifest = Stamp::read(&directory.join("manifest.json")).unwrap();
        progress.narrative = Stamp::read(&directory.join("conversation.jsonl")).unwrap();
        progress.events = Stamp::read(&directory.join("events.jsonl")).unwrap();
    }

    fn cold_body_boundary_publish(context: &ConversationArchiveContext, saved: &Head) -> String {
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        let mut store = Store::checkpoint_writer(&objects);
        store
            .finish_checkpoint(&saved.checkpoint_dependencies())
            .unwrap();
        let reference = store.put(saved).unwrap();
        write_json_atomic(&pointer, &reference).unwrap();
        reference
    }

    #[test]
    fn cold_body_near_limit_head_resets_old_revision_and_drains_after_restart() {
        use super::super::super::chat_read_store::OBJECT_BYTES;
        use crate::state::conversation_archive::tests::isolated_home;

        const JOBS: usize = 17;
        let (_guard, temp) = isolated_home();
        let (mut context, rows, expected) = cold_body_drain_fixture(&[8 * 1024; JOBS], &[0; JOBS]);
        let (mut saved, _) = cold_body_drain_rows(&context);
        let records = cold_body_boundary_records(&rows);
        context.provider_session_ids = vec!["saved-session-000".into()];
        cold_body_boundary_cohort(&context, &mut saved, &records);

        // Use realistic bounded IDs, not an oversized arbitrary Head field.
        // Scope, caller context, manifest and index all retain the same list.
        let target = OBJECT_BYTES - 500;
        loop {
            saved.bootstrap.as_mut().unwrap().selection_scope = Some(SelectionScope::new(&context));
            let length = serde_json::to_vec(&saved).unwrap().len();
            assert!(length <= target);
            if target - length <= 128 {
                context
                    .provider_session_ids
                    .last_mut()
                    .unwrap()
                    .push_str(&"s".repeat(target - length));
                break;
            }
            context.provider_session_ids.push(format!(
                "saved-session-{:03}-{}",
                context.provider_session_ids.len(),
                "s".repeat(96)
            ));
        }
        assert!(context
            .provider_session_ids
            .iter()
            .all(|id| id.len() <= 256));
        cold_body_boundary_cohort(&context, &mut saved, &records);
        let initial_size = serde_json::to_vec(&saved).unwrap().len();
        assert!((OBJECT_BYTES - 520..=OBJECT_BYTES - 480).contains(&initial_size));
        assert!(scope_matches(&saved, &context));

        let mut four = saved.clone();
        four.parent = Some("0".repeat(64));
        four.changes = vec!["0".repeat(64); 4];
        four.bootstrap.as_mut().unwrap().body_before = Some(rows[JOBS - 4].key.clone());
        four.recent_page = Some("0".repeat(64));
        assert!(serde_json::to_vec(&four).unwrap().len() <= OBJECT_BYTES);
        let mut sixteen = four;
        sixteen.changes = vec!["0".repeat(64); 16];
        sixteen.recent_page = None;
        assert!(serde_json::to_vec(&sixteen).unwrap().len() > OBJECT_BYTES);

        let previous = cold_body_boundary_publish(&context, &saved);
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        assert!(pointer.starts_with(temp.path()) && objects.starts_with(temp.path()));
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let protected = [
            super::super::super::storage::index_path(&context.agent_id).unwrap(),
            directory.join("manifest.json"),
            directory.join("conversation.jsonl"),
            directory.join("events.jsonl"),
        ]
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        });
        let mut snapshot = super::super::tests::snapshot(&context);
        snapshot.resume_session = context.provider_session_ids.first().cloned();
        let read = |revision| {
            super::super::read_with_revalidation_and_limit(
                &context, &snapshot, None, revision, None, None, None,
            )
            .unwrap()
        };
        let initial = read(None);
        assert_eq!(initial.events.len(), JOBS);
        assert!(initial.revision.starts_with(&previous));

        assert!(advance(&context).expect("a large optional delta must not block body publication"));
        let (checkpoint, published_rows) = cold_body_drain_rows(&context);
        assert!(checkpoint.parent.is_none() && checkpoint.changes.is_empty());
        assert_eq!(checkpoint.generation, saved.generation);
        assert!(scope_matches(&checkpoint, &context));
        assert!(checkpoint.source_epoch.is_none());
        assert_eq!(
            published_rows
                .iter()
                .filter(|row| row.body.as_ref().unwrap().complete)
                .count(),
            16
        );
        let current = head(&context.agent_id).unwrap().unwrap();
        assert!(super::super::changes_since(
            &mut Store::new(&objects),
            &current,
            &checkpoint,
            &previous,
        )
        .unwrap()
        .is_none());
        let refreshed = read(Some(initial.revision.as_str()));
        assert!(refreshed.reset && !refreshed.unchanged);
        assert_eq!(refreshed.events.len(), JOBS);
        assert_eq!(
            refreshed
                .events
                .iter()
                .map(|event| event.id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
        );
        assert!(refreshed.source_epoch.is_none() && refreshed.recent_admission.is_none());
        assert!(refreshed.revalidated_ids.is_none());

        // Reopen from the persisted checkpoint; no transient Store survives.
        assert!(!advance(&context).unwrap());
        assert_cold_body_drain_payloads(&context, &expected);
        let settled = read(None);
        assert_eq!(settled.events.len(), JOBS);
        let quiet = read(Some(settled.revision.as_str()));
        assert!(quiet.unchanged && quiet.events.is_empty() && !quiet.reset);
        let settled_pointer = fs::read(&pointer).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&pointer).unwrap(), settled_pointer);
        for (path, bytes) in protected {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn cold_body_metadata_pressure_reserves_four_chunks_then_restarts_exactly() {
        use crate::state::conversation_archive::tests::isolated_home;

        let (_guard, _temp) = isolated_home();
        let (context, rows, expected) = cold_body_drain_fixture(&[20 * BODY_BYTES], &[0]);
        let (mut saved, _) = cold_body_drain_rows(&context);
        let template = cold_body_boundary_records(&rows).pop().unwrap();
        let records = (0..ADMIT_ROWS)
            .map(|index| {
                let mut record = template.clone();
                record.seq = index as u64 + 1;
                record.event_refs = (0..16)
                    .map(|reference| format!("metadata-proof-{index:02}-{reference:02}"))
                    .collect();
                record.artifact_refs.clear();
                if index == ADMIT_ROWS - 1 {
                    record.event_refs[0] = rows[0].event.id.clone();
                    record.artifact_refs = template.artifact_refs.clone();
                }
                record
            })
            .collect::<Vec<_>>();
        cold_body_boundary_cohort(&context, &mut saved, &records);
        let directory = super::super::super::storage::conversation_dir(
            &context.agent_id,
            &saved.conversation_id,
        )
        .unwrap();
        let narrative_path = directory.join("conversation.jsonl");
        let progress = saved.bootstrap.as_mut().unwrap();
        progress.narrative_before = progress.narrative.length;
        assert_eq!(progress.event_before, 0);
        assert!(progress.narrative.length <= WINDOW_BYTES);
        let metadata_window = window(&narrative_path, progress.narrative_before).unwrap();
        assert_eq!(metadata_window.len(), ADMIT_ROWS);
        cold_body_boundary_publish(&context, &saved);

        assert!(advance(&context).unwrap());
        let (first, first_rows) = cold_body_drain_rows(&context);
        let progress = first.bootstrap.as_ref().unwrap();
        // All 24 narratives fit in this window. Only checkpoint_due can stop
        // the successful proof loop before its final offset reaches zero.
        assert!(progress.narrative_before > 0);
        let processed = metadata_window
            .iter()
            .position(|(offset, _)| *offset == progress.narrative_before)
            .unwrap()
            + 1;
        assert!(processed < metadata_window.len());
        let (pointer, objects) = locations(&context.agent_id).unwrap();
        let mut reader = Store::new(&objects);
        let proofs = reader
            .page(&progress.proofs, None, ADMIT_ROWS * 16)
            .unwrap();
        assert_eq!(proofs.len(), processed * 16);
        let proof_keys = metadata_window
            .iter()
            .take(processed)
            .flat_map(|(_, bytes)| {
                serde_json::from_slice::<ConversationNarrativeRecord>(bytes)
                    .unwrap()
                    .event_refs
            })
            .map(|id| digest(id.as_bytes()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            proofs
                .into_iter()
                .map(|(key, _)| key)
                .collect::<std::collections::BTreeSet<_>>(),
            proof_keys
        );
        let body = first_rows[0].body.as_ref().unwrap();
        assert_eq!(
            body.bytes,
            4 * BODY_BYTES as u64,
            "chunk five must yield to real metadata pressure"
        );
        assert!(!body.complete);
        assert!(progress.body_before.is_none());
        let queued_ref = reader.get(&progress.bodies, &rows[0].key).unwrap().unwrap();
        let queued: BodyJob = reader.read(&queued_ref).unwrap();
        assert_eq!(queued.offset, body.bytes);
        assert_eq!(queued.row.body.as_ref().unwrap().root, body.root);
        drop(reader);

        let mut previous_bytes = body.bytes;
        let mut previous_narrative = progress.narrative_before;
        let mut settled = false;
        for _ in 0..ADMIT_ROWS + 24 {
            // Every advance reconstructs its Store and queued source from disk.
            let pending = advance(&context).unwrap();
            let (checkpoint, current_rows) = cold_body_drain_rows(&context);
            let progress = checkpoint.bootstrap.as_ref().unwrap();
            let body = current_rows[0].body.as_ref().unwrap();
            assert_eq!(checkpoint.generation, saved.generation);
            assert!(scope_matches(&checkpoint, &context));
            assert!(progress.narrative_before <= previous_narrative);
            assert!(
                body.bytes >= previous_bytes
                    && body.bytes <= expected[&rows[0].event.id].len() as u64
            );
            if !body.complete {
                assert!(progress.body_before.is_none());
                let mut reader = Store::new(&objects);
                let reference = reader.get(&progress.bodies, &rows[0].key).unwrap().unwrap();
                let job: BodyJob = reader.read(&reference).unwrap();
                assert_eq!(job.offset, body.bytes);
            } else {
                assert_eq!(progress.body_before.as_ref(), Some(&rows[0].key));
            }
            previous_bytes = body.bytes;
            previous_narrative = progress.narrative_before;
            if !pending {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "real metadata pressure and the finite long body must drain"
        );
        assert_cold_body_drain_payloads(&context, &expected);
        let settled_pointer = fs::read(&pointer).unwrap();
        assert!(!advance(&context).unwrap());
        assert_eq!(fs::read(&pointer).unwrap(), settled_pointer);
    }
}

#[cfg(test)]
include!("chat_archive_bootstrap_legacy_fixture.inc.rs");
