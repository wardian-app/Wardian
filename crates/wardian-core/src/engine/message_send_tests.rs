//! Frozen-review integration: real driver, file boundary and canonical store;
//! no provider, Wardian home, shell transport or live recipient.
use super::*;
use crate::{automation::Blueprint, db::agent_messaging as store};
use async_trait::async_trait;
use std::result::Result;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

const BODY: &str = "Review basis: transport\r\n日本語 'quotes' \"double\" `$() & | ; {{run.id}}\r\nVerdict: blocked\r\n";

fn blueprint() -> Blueprint {
    serde_json::from_value(serde_json::json!({
        "schema": 2, "id": "delivery-fixture", "name": "Frozen review delivery",
        "nodes": [
            {"id":"trigger","type":"manual_trigger"},
            {"id":"review","type":"task","fields":{"agent":"role:reviewer","prompt":"reviews/{{run.id}}/review.md"}},
            {"id":"deliver","type":"message_send","fields":{"recipient":"{{trigger.output.recipient}}","artifact_path":"reviews/{{run.id}}/review.md"}},
            {"id":"notice","type":"notify","fields":{"message":"Delivered"}}
        ],
        "edges": [{"from":"trigger","to":"review"},{"from":"review","to":"deliver"},{"from":"deliver","to":"notice"}]
    })).unwrap()
}

struct Fixture {
    workspace: PathBuf,
    conn: Mutex<rusqlite::Connection>,
    reviews: AtomicUsize,
    notices: AtomicUsize,
    crash_after_commit: AtomicBool,
    host_available: bool,
    reassign_after_review: bool,
    delete_after_review: bool,
    names: Mutex<std::collections::HashMap<String, String>>,
}

impl Fixture {
    fn new(workspace: PathBuf) -> Self {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        Self {
            workspace,
            conn: Mutex::new(conn),
            reviews: AtomicUsize::new(0),
            notices: AtomicUsize::new(0),
            crash_after_commit: AtomicBool::new(false),
            host_available: true,
            reassign_after_review: false,
            delete_after_review: false,
            names: Mutex::new(std::collections::HashMap::from([
                ("Requester".into(), "recipient".into()),
                ("recipient".into(), "recipient".into()),
                ("replacement".into(), "replacement".into()),
            ])),
        }
    }
}

#[async_trait]
impl StepExecutor for Fixture {
    async fn run_agent_task(&self, req: AgentTaskRequest) -> Result<StepOutput, StepError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        std::fs::write(self.workspace.join(req.prompt), BODY).unwrap();
        if self.reassign_after_review {
            self.names
                .lock()
                .unwrap()
                .insert("Requester".into(), "replacement".into());
        }
        if self.delete_after_review {
            self.names.lock().unwrap().remove("recipient");
        }
        Ok(StepOutput(
            serde_json::json!({"finding_count":0,"verdict":"blocked"}),
        ))
    }
    async fn preflight_message_send(
        &self,
        req: MessageSendRequest,
        fresh: bool,
    ) -> Result<String, StepError> {
        if !self.host_available {
            return Err(StepError::new("unavailable host"));
        }
        let recipient_id = self
            .names
            .lock()
            .unwrap()
            .get(&req.recipient)
            .cloned()
            .ok_or_else(|| StepError::new("missing recipient"))?;
        message_artifact::prepare(&self.workspace, &req.artifact_path, fresh)?;
        Ok(recipient_id)
    }
    async fn message_send(&self, req: MessageSendRequest) -> Result<StepOutput, StepError> {
        if self.names.lock().unwrap().get(&req.recipient) != Some(&req.recipient) {
            return Err(StepError::new("missing bound recipient"));
        }
        let (body, hash) = message_artifact::read(&self.workspace, &req.artifact_path)?;
        let admitted = store::admit_host_automation_message(
            &self.conn.lock().unwrap(),
            "durable-run",
            &req.node,
            &req.recipient,
            &body,
            0,
        )
        .map_err(|error| StepError::new(error.to_string()))?;
        // Model process loss after the DB commit but before NodeCompleted/event
        // checkpointing. The store lock is released before the injected panic.
        if self.crash_after_commit.swap(false, Ordering::SeqCst) {
            panic!("injected crash after commit");
        }
        Ok(StepOutput(
            serde_json::json!({"interaction_id":admitted.record.id,"recipient_id":req.recipient,"artifact_sha256":hash,"duplicate":admitted.duplicate,"delivery_state":admitted.delivery_state}),
        ))
    }
    async fn notify(&self, _req: NotifyRequest) -> Result<(), StepError> {
        self.notices.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn run_decision(&self, _req: DecisionRequest) -> Result<ChosenPort, StepError> {
        unreachable!()
    }
    async fn run_shell(&self, _req: ShellRequest) -> Result<StepOutput, StepError> {
        unreachable!()
    }
    async fn run_script(&self, _req: ScriptRequest) -> Result<StepOutput, StepError> {
        unreachable!()
    }
    async fn memory_commit(&self, _req: MemoryCommitRequest) -> Result<StepOutput, StepError> {
        unreachable!()
    }
}

#[tokio::test]
async fn frozen_review_delivers_exactly_once_after_commit_checkpoint_crash() {
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::new(workspace.path().into());
    fixture.reassign_after_review = true;
    let fixture = Arc::new(fixture);
    fixture.crash_after_commit.store(true, Ordering::SeqCst);
    let (task_fixture, task_root) = (fixture.clone(), root.path().to_owned());
    let crashed = tokio::spawn(async move {
        Engine::start_with_id(
            &blueprint(),
            "durable-run",
            serde_json::json!({"recipient":"Requester","run":{"id":"spoofed"}}),
            &task_root,
            task_fixture.as_ref(),
        )
        .await
    })
    .await;
    assert!(crashed.unwrap_err().is_panic());
    let state = Engine::resume(&blueprint(), root.path(), fixture.as_ref())
        .await
        .unwrap();
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.registry["run"]["id"], "durable-run");
    assert_eq!(
        state.message_deliveries["deliver"].recipient_id,
        "recipient"
    );
    assert_eq!(fixture.names.lock().unwrap()["Requester"], "replacement");
    assert_eq!(fixture.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.notices.load(Ordering::SeqCst), 1);
    assert_eq!(state.node_output("deliver").unwrap()["duplicate"], true);
    let conn = fixture.conn.lock().unwrap();
    let page = store::receive(&conn, "recipient", None, None, 100).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].message.as_bytes(), BODY.as_bytes());
    assert_eq!(
        state.node_output("deliver").unwrap()["interaction_id"],
        page.messages[0].interaction_id
    );
    assert_eq!(
        state.node_output("deliver").unwrap()["artifact_sha256"],
        message_artifact::read(workspace.path(), "reviews/durable-run/review.md")
            .unwrap()
            .1
    );
    assert_eq!(Engine::replay(&blueprint(), root.path()).unwrap(), state);
}

#[tokio::test]
async fn failed_delivery_blocks_notice_preserves_completed_review_and_never_retries_failed_run() {
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(workspace.path().into());
    fixture.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_delivery BEFORE INSERT ON agent_message_availability BEGIN SELECT RAISE(ABORT, 'disk failure'); END;").unwrap();
    let state = Engine::start_with_id(
        &blueprint(),
        "durable-run",
        serde_json::json!({"recipient":"recipient"}),
        root.path(),
        &fixture,
    )
    .await
    .unwrap();
    assert_eq!(state.status, RunStatus::Failed);
    assert_eq!(state.node_status("review"), Some(NodeStatus::Completed));
    assert_eq!(state.node_status("deliver"), Some(NodeStatus::Failed));
    assert_eq!(state.node_output("review").unwrap()["verdict"], "blocked");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("reviews/durable-run/review.md")).unwrap(),
        BODY
    );
    assert_eq!(fixture.notices.load(Ordering::SeqCst), 0);
    fixture
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_delivery")
        .unwrap();
    assert_eq!(
        Engine::resume(&blueprint(), root.path(), &fixture)
            .await
            .unwrap()
            .status,
        RunStatus::Failed
    );
    assert_eq!(fixture.reviews.load(Ordering::SeqCst), 1);
    assert!(
        store::receive(&fixture.conn.lock().unwrap(), "recipient", None, None, 100)
            .unwrap()
            .messages
            .is_empty()
    );
}

#[tokio::test]
async fn preflight_rejects_unavailable_host_bad_recipient_and_stale_artifact_before_review() {
    for case in ["host", "recipient", "stale"] {
        let workspace = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new(workspace.path().into());
        fixture.host_available = case != "host";
        if case == "stale" {
            message_artifact::prepare(workspace.path(), "reviews/durable-run/review.md", true)
                .unwrap();
            std::fs::write(
                workspace.path().join("reviews/durable-run/review.md"),
                "old run",
            )
            .unwrap();
        }
        let recipient = if case == "recipient" {
            "missing"
        } else {
            "recipient"
        };
        let state = Engine::start_with_id(
            &blueprint(),
            "durable-run",
            serde_json::json!({"recipient":recipient}),
            root.path(),
            &fixture,
        )
        .await
        .unwrap();
        assert_eq!(state.status, RunStatus::Failed, "{case}");
        assert_eq!(fixture.reviews.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.notices.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn delivery_template_contract_supports_reserved_run_id_but_rejects_task_selected_fields() {
    let mut bp = blueprint();
    assert!(crate::automation::validate(&bp).is_valid());
    for template in [
        "{{nodes.review.output.recipient}}",
        "{{run.other}}",
        "{{run.id",
        "",
    ] {
        bp.nodes[2]
            .fields
            .insert("recipient".into(), template.into());
        assert!(!crate::automation::validate(&bp).is_valid(), "{template}");
    }
    bp.nodes[2].fields.remove("recipient");
    bp.nodes[2]
        .fields
        .insert("recipient_id".into(), "recipient".into());
    assert!(!crate::automation::validate(&bp).is_valid());
}

#[tokio::test]
async fn bound_recipient_survives_name_takeover_and_deletion_fails_closed() {
    for delete in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new(workspace.path().into());
        fixture.reassign_after_review = true;
        fixture.delete_after_review = delete;
        let state = Engine::start_with_id(
            &blueprint(),
            "durable-run",
            serde_json::json!({"recipient":"Requester"}),
            root.path(),
            &fixture,
        )
        .await
        .unwrap();
        assert_eq!(
            state.status,
            if delete {
                RunStatus::Failed
            } else {
                RunStatus::Completed
            }
        );
        assert_eq!(
            state.message_deliveries["deliver"].recipient_id,
            "recipient"
        );
        assert_eq!(state.node_status("review"), Some(NodeStatus::Completed));
        let replayed = Engine::replay(&blueprint(), root.path()).unwrap();
        assert_eq!(replayed, state);
        let events = super::store::read_events(root.path()).unwrap();
        let prepared = events
            .iter()
            .position(|event| matches!(event.kind, EventKind::MessageSendPrepared { .. }))
            .unwrap();
        let review = events
            .iter()
            .position(
                |event| matches!(&event.kind, EventKind::NodeStarted {node} if node == "review"),
            )
            .unwrap();
        assert!(prepared < review);
        let conn = fixture.conn.lock().unwrap();
        assert!(
            crate::db::agent_messaging::receive(&conn, "replacement", None, None, 100)
                .unwrap()
                .messages
                .is_empty()
        );
        let original =
            crate::db::agent_messaging::receive(&conn, "recipient", None, None, 100).unwrap();
        assert_eq!(original.messages.len(), usize::from(!delete));
        assert_eq!(fixture.notices.load(Ordering::SeqCst), usize::from(!delete));
    }
}

#[tokio::test]
async fn approval_continuation_reuses_prepared_recipient_before_first_delivery() {
    for delete in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(workspace.path().into());
        let mut bp = blueprint();
        bp.nodes.push(
            serde_json::from_value(serde_json::json!({"id":"gate","type":"approval"})).unwrap(),
        );
        bp.edges[1].to = "gate".into();
        bp.edges.push(
            serde_json::from_value(serde_json::json!({"from":"gate","to":"deliver"})).unwrap(),
        );
        let parked = Engine::start_with_id(
            &bp,
            "durable-run",
            serde_json::json!({"recipient":"Requester"}),
            root.path(),
            &fixture,
        )
        .await
        .unwrap();
        assert_eq!(parked.status, RunStatus::AwaitingApproval);
        fixture
            .names
            .lock()
            .unwrap()
            .insert("Requester".into(), "replacement".into());
        if delete {
            fixture.names.lock().unwrap().remove("recipient");
        }
        let approved =
            Engine::record_approval_granted(&bp, root.path(), "gate", "fixture", None).unwrap();
        assert_eq!(
            approved.message_deliveries["deliver"].recipient_id,
            "recipient"
        );
        let state = Engine::drive_from_state(&bp, approved, root.path(), &fixture)
            .await
            .unwrap();
        assert_eq!(
            state.status,
            if delete {
                RunStatus::Failed
            } else {
                RunStatus::Completed
            }
        );
        assert_eq!(fixture.reviews.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.notices.load(Ordering::SeqCst), usize::from(!delete));
        assert!(crate::db::agent_messaging::receive(
            &fixture.conn.lock().unwrap(),
            "replacement",
            None,
            None,
            100
        )
        .unwrap()
        .messages
        .is_empty());
    }
}
