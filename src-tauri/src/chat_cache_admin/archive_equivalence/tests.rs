use std::collections::BTreeMap;
use std::fs;

use serde_json::{json, Value};
use tempfile::TempDir;
use wardian_core::models::chat::AgentChatEvent;

use super::*;

const AGENT: &str = "synthetic-agent";
const CONVERSATION: &str = "synthetic-conversation";
const SECRET: &str = "synthetic private detail sentinel";
const KEY: &str = "00000000000000000000";

struct Fixture {
    temp: TempDir,
    agent: PathBuf,
    archive: PathBuf,
    objects: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("agents").join(AGENT);
        let archive = agent.join("conversations").join(CONVERSATION);
        let objects = agent.join("chat-read-objects");
        fs::create_dir_all(archive.join("artifacts")).unwrap();
        fs::create_dir_all(&objects).unwrap();
        Self {
            temp,
            agent,
            archive,
            objects,
        }
    }

    fn put(&self, bytes: &[u8]) -> String {
        let reference = digest(bytes);
        fs::write(self.objects.join(&reference), bytes).unwrap();
        reference
    }

    fn json(&self, value: Value) -> String {
        self.put(&serde_json::to_vec(&value).unwrap())
    }

    fn node(&self, key: &str, value: &str) -> String {
        self.json(json!({ "key": key, "value": value, "left": null, "right": null, "height": 1 }))
    }

    fn event(&self, id: &str) -> AgentChatEvent {
        serde_json::from_value(json!({
            "id": id, "session_id": AGENT, "provider": "mock", "kind": "message",
            "role": "assistant", "sequence": 1, "metadata": {
                "text_excerpt": "saved excerpt", "text_artifact_refs": ["body.txt"],
                "conversation_archive_id": CONVERSATION, "provider_session_id": "provider-session",
            }
        }))
        .unwrap()
    }

    fn archive(&self, events: &[AgentChatEvent], body: &[u8]) {
        let manifest = json!({
            "schema": 1, "conversation_id": CONVERSATION, "agent_id": AGENT, "agent_name": "",
            "agent_class": "", "workspace": "", "provider": "mock", "provider_session_ids": ["provider-session"],
            "provider_source_key": "mock:scope", "effective_logging": "enabled",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "status": "open", "boundary_reason": "spawn",
            "format_versions": {"manifest":1,"conversation":1,"events":1,"sources":1,"turns":3},
            "record_count": events.len()
        });
        fs::write(
            self.archive.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        write_jsonl(&self.archive.join("events.jsonl"), events);
        let narrative = events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                json!({
                    "schema": 1, "seq": index + 1, "at": "2026-01-01T00:00:00Z", "kind": "message",
                    "role": "assistant", "excerpt": "saved excerpt", "event_refs": [event.id],
                    "source_refs": ["source-key"], "artifact_refs": ["body.txt"],
                })
            })
            .collect::<Vec<_>>();
        write_jsonl(&self.archive.join("conversation.jsonl"), &narrative);
        write_jsonl(
            &self.archive.join("sources.jsonl"),
            &[json!({
                "schema": 1, "source_id": "source-key", "provider": "mock",
                "provider_session_id": "provider-session", "source_kind": "provider_session"
            })],
        );
        fs::write(self.archive.join("artifacts/body.txt"), body).unwrap();
    }

    fn graph(
        &self,
        event: &AgentChatEvent,
        body: &[u8],
        complete: bool,
    ) -> (Value, String, String) {
        let (first, rest) = body.split_at(body.len().min(OBJECT_BYTES));
        let chunk = self.put(first);
        let body_root = if rest.is_empty() {
            self.node(KEY, &chunk)
        } else {
            assert!(rest.len() <= OBJECT_BYTES);
            let second = self.node(&format!("{OBJECT_BYTES:020}"), &self.put(rest));
            self.json(json!({
                "key": KEY, "value": chunk, "left": null, "right": second, "height": 2,
            }))
        };
        let header = crate::state::conversation_archive::chat_read::header(event);
        let row = self.json(json!({
            "agent_id": AGENT, "conversation_id": CONVERSATION, "key": KEY, "event": header,
            "body": {"binding": digest(body), "root": body_root, "bytes": body.len(), "complete": complete},
        }));
        let root = self.node(KEY, &row);
        let identities = self.node(&digest(event.id.as_bytes()), &row);
        let head = json!({
            "agent_id": AGENT, "conversation_id": CONVERSATION, "source_key": "mock:scope",
            "generation": "stable-generation", "source_epoch": null, "root": root, "identities": identities,
            "committed_output_bytes": 0, "capture_stamp": "synthetic-current", "progress": "ready",
            "parent": null, "changes": [row], "recent_start": 0, "row_count": 1
        });
        (head, row, chunk)
    }

    fn set_head(&self, value: Value) -> String {
        let reference = self.json(value);
        fs::write(
            self.agent.join("chat-read-head.json"),
            serde_json::to_vec(&reference).unwrap(),
        )
        .unwrap();
        reference
    }

    fn report(&self) -> Report {
        verify(self.temp.path().canonicalize().unwrap(), AGENT)
    }

    fn snapshots(&self) -> BTreeMap<PathBuf, String> {
        fn walk(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, String>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        digest(&fs::read(path).unwrap()),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(self.temp.path(), self.temp.path(), &mut files);
        files
    }
}

fn write_jsonl<T: Serialize>(path: &Path, rows: &[T]) {
    let text: String = rows
        .iter()
        .map(|row| format!("{}\n", serde_json::to_string(row).unwrap()))
        .collect();
    fs::write(path, text).unwrap();
}

#[test]
fn matches_current_parent_and_detached_owned_rows_and_complete_artifact_bytes() {
    let fixture = Fixture::new();
    let event = fixture.event("saved-row");
    let body = SECRET.repeat(900);
    fixture.archive(std::slice::from_ref(&event), body.as_bytes());
    let (mut current, _, _) = fixture.graph(&event, body.as_bytes(), true);
    let mut parent = current.clone();
    parent["capture_stamp"] = json!("parent");
    let parent = fixture.json(parent);
    let mut detached = current.clone();
    detached["capture_stamp"] = json!("detached");
    fixture.json(detached);
    current["parent"] = json!(parent);
    fixture.set_head(current);
    let before = fixture.snapshots();
    let report = fixture.report();
    assert!(
        report.measured_equivalent,
        "{}",
        serde_json::to_string(&report).unwrap()
    );
    assert!(report.scan_complete && report.deletion_blocked);
    assert_eq!(report.heads_checked, 3);
    assert_eq!(report.detached_heads, 1);
    assert_eq!(report.rows.matched, 3);
    assert_eq!(report.bodies.matched, 3);
    assert!(!serde_json::to_string(&report).unwrap().contains(SECRET));
    assert_eq!(fixture.snapshots(), before);

    let mut mismatch = body.into_bytes();
    *mismatch.last_mut().unwrap() = b'!';
    fs::write(fixture.archive.join("artifacts/body.txt"), mismatch).unwrap();
    let report = fixture.report();
    assert_eq!(report.rows.matched, 3);
    assert_eq!(report.bodies.unverifiable, 3);
    assert!(!report.measured_equivalent);
}

#[test]
fn validates_protected_v1_pack_without_trusting_a_corrupt_binding() {
    let fixture = Fixture::new();
    let event = fixture.event("pack-row");
    let body = SECRET.repeat(100);
    fixture.archive(std::slice::from_ref(&event), body.as_bytes());
    let (mut current, row, _) = fixture.graph(&event, body.as_bytes(), true);
    let raw = serde_json::to_vec(&current).unwrap();
    let typed = Published::decode(&raw, AGENT).unwrap();
    let binding = digest(&serde_json::to_vec(&typed.head).unwrap());
    let header = fs::read_to_string(fixture.objects.join(&row)).unwrap();
    let mut pack = json!({
        "binding": {"version": 1, "head_binding": binding, "agent_id": AGENT, "conversation_id": CONVERSATION,
            "generation": current["generation"], "source_key": current["source_key"], "source_epoch": null,
            "root": current["root"], "row_count": 1},
        "entries": [{"key": KEY, "reference": row, "header": header}],
        "has_older": false, "lookahead": null,
    });
    current["recent_page"] = json!(fixture.json(pack.clone()));
    let prior = fixture.set_head(current.clone());
    fs::remove_file(fixture.objects.join(&row)).unwrap();
    let report = fixture.report();
    assert!(
        report.measured_equivalent,
        "{}",
        serde_json::to_string(&report).unwrap()
    );
    assert!(report.deletion_blocked);
    assert_eq!(report.authenticated_legacy_packs, 1);
    assert_eq!(report.protected_pack_rows, 1);
    assert_eq!(report.bodies.matched, 1);

    // Model a foreign Head binding with an otherwise intact embedded row.
    fs::remove_file(fixture.objects.join(prior)).unwrap();
    pack["binding"]["head_binding"] = json!("0".repeat(64));
    current["recent_page"] = json!(fixture.json(pack));
    fixture.set_head(current);
    let report = fixture.report();
    assert!(!report.scan_complete && !report.measured_equivalent);
    assert_eq!(report.protected_pack_rows, 0);
    assert_eq!(report.rows.unverifiable, 1);
}

#[test]
fn separates_cas_only_row_and_artifact_from_unverifiable_or_partial_body() {
    let fixture = Fixture::new();
    let event = fixture.event("cas-only-row");
    let body = SECRET.repeat(100);
    fixture.archive(&[], body.as_bytes());
    let (head, _, _) = fixture.graph(&event, body.as_bytes(), true);
    let first = fixture.set_head(head);
    let report = fixture.report();
    assert!(report.scan_complete && !report.measured_equivalent);
    assert_eq!(report.rows.missing, 1);
    assert_eq!(report.bodies.missing, 1);

    fixture.archive(std::slice::from_ref(&event), body.as_bytes());
    fs::remove_file(fixture.archive.join("artifacts/body.txt")).unwrap();
    let report = fixture.report();
    assert_eq!(report.rows.matched, 1);
    assert_eq!(report.bodies.missing, 1);

    fixture.archive(std::slice::from_ref(&event), body.as_bytes());
    fs::remove_file(fixture.objects.join(first)).unwrap();
    let (head, _, _) = fixture.graph(&event, &body.as_bytes()[..64], false);
    fixture.set_head(head);
    let report = fixture.report();
    assert!(!report.measured_equivalent);
    assert_eq!(report.rows.matched, 1);
    assert_eq!(report.bodies.unverifiable, 1);
    assert_eq!(report.incomplete_old_bodies, 1);
}

#[test]
fn null_top_level_artifact_cannot_hide_unresolved_legacy_body() {
    let fixture = Fixture::new();
    let mut event = fixture.event("legacy-no-body");
    event.text = Some("saved inline excerpt".into());
    event
        .metadata
        .as_object_mut()
        .unwrap()
        .remove("text_excerpt");
    event.metadata["text_artifact_refs"] = Value::Null;
    event.metadata["archive_record"] = json!({"artifact_refs": ["body.txt"]});
    let body = SECRET.as_bytes();
    fixture.archive(std::slice::from_ref(&event), body);
    let (mut head, complete_row, _) = fixture.graph(&event, body, true);
    let mut row: Value =
        serde_json::from_slice(&fs::read(fixture.objects.join(complete_row)).unwrap()).unwrap();
    row["body"] = Value::Null;
    let row = fixture.json(row);
    head["root"] = json!(fixture.node(KEY, &row));
    head["identities"] = json!(fixture.node(&digest(event.id.as_bytes()), &row));
    head["changes"] = json!([row]);
    fixture.set_head(head);

    let artifact = fixture.archive.join("artifacts/body.txt");
    fs::remove_file(&artifact).unwrap();
    let report = fixture.report();
    assert_eq!(report.rows.matched, 1);
    assert!(
        !report.measured_equivalent && report.deletion_blocked,
        "{}",
        serde_json::to_string(&report).unwrap()
    );
    assert_eq!(report.bodies.missing, 1);

    // A readable archive artifact still cannot establish parity for an old
    // row that carries neither complete detail nor a body binding.
    fs::write(&artifact, body).unwrap();
    let report = fixture.report();
    assert!(!report.measured_equivalent && report.deletion_blocked);
    assert_eq!(report.bodies.unverifiable, 1);

    // A narrative-only declaration is unresolved even when an event has text.
    event.metadata["archive_record"] = Value::Null;
    fixture.archive(&[event], body);
    let report = fixture.report();
    assert_eq!(report.rows.matched, 1);
    assert_eq!(report.bodies.unverifiable, 1);
    assert!(!report.measured_equivalent && report.deletion_blocked);
}

#[test]
fn rejects_ambiguous_or_foreign_owned_records_even_for_equal_text() {
    let fixture = Fixture::new();
    let event = fixture.event("owner-row");
    let body = SECRET.as_bytes();
    fixture.archive(&[event.clone(), event.clone()], body);
    let (head, _, _) = fixture.graph(&event, body, true);
    fixture.set_head(head);
    assert_eq!(fixture.report().rows.unverifiable, 1);
    let mut foreign = event;
    foreign.metadata["provider_session_id"] = json!("foreign-session");
    fixture.archive(&[foreign], body);
    let report = fixture.report();
    assert_eq!(report.rows.unverifiable, 1);
    assert_eq!(report.rows.matched, 0);
    assert!(report.deletion_blocked);
}

#[test]
fn reports_archive_events_absent_from_the_current_graph() {
    let fixture = Fixture::new();
    let event = fixture.event("covered");
    let body = SECRET.as_bytes();
    fixture.archive(&[event.clone(), fixture.event("unpaired")], body);
    let (head, _, _) = fixture.graph(&event, body, true);
    fixture.set_head(head);
    let report = fixture.report();
    assert_eq!(report.rows.matched, 1);
    assert_eq!(report.current_archive_unpaired_events, 1);
    assert!(!report.measured_equivalent);
}

#[test]
fn rejects_invalid_content_and_linked_inputs_and_reports_bounds() {
    let fixture = Fixture::new();
    let event = fixture.event("bounded-row");
    let body = SECRET.as_bytes();
    fixture.archive(std::slice::from_ref(&event), body);
    let (head, _, chunk) = fixture.graph(&event, body, true);
    fixture.set_head(head);
    let bounded = verify_with_limits(fixture.temp.path().canonicalize().unwrap(), AGENT, 4, 64);
    assert!(!bounded.scan_complete && !bounded.measured_equivalent);
    assert_eq!(bounded.scan_faults.budget_exhausted, 1);
    let chunk = fixture.objects.join(chunk);
    fs::write(&chunk, b"changed bytes").unwrap();
    let report = fixture.report();
    assert!(!report.scan_complete && !report.measured_equivalent);
    assert!(report.scan_faults.invalid_input > 0);

    let artifact = fixture.archive.join("artifacts/body.txt");
    fs::write(&chunk, body).unwrap();
    fs::remove_file(&artifact).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let original = elsewhere.path().join("outside.txt");
    fs::write(&original, body).unwrap();
    fs::hard_link(&original, &artifact).unwrap();
    let report = fixture.report();
    assert!(!report.scan_complete && !report.measured_equivalent);
    assert_eq!(report.scan_faults.unsafe_input, 1);
    assert_eq!(report.bodies.matched, 0);
    assert_eq!(fs::read(&original).unwrap(), body);
}
