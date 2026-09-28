use super::LiveStepExecutor;
use crate::{control::agent_messaging, state::AppState};
use tauri::Manager;
use wardian_core::{
    db::agent_messaging as store,
    engine::{message_artifact, MessageSendRequest, StepError, StepOutput},
};

impl LiveStepExecutor {
    fn information_host(&self) -> Result<(&tauri::AppHandle, &str), StepError> {
        let app = self
            .notification_app
            .as_ref()
            .ok_or_else(|| StepError::new("message_send requires the application host"))?;
        let (_, run_id) = self
            .automation_origin
            .as_ref()
            .ok_or_else(|| StepError::new("message_send requires trusted automation provenance"))?;
        Ok((app, run_id))
    }

    pub(super) async fn preflight_information(
        &self,
        req: MessageSendRequest,
        fresh: bool,
    ) -> Result<String, StepError> {
        let (app, _) = self.information_host()?;
        let state = app.state::<AppState>();
        let recipient_id = agent_messaging::resolve_automation_recipient(&state, &req.recipient)
            .await
            .map_err(|error| StepError::new(error.to_string()))?;
        message_artifact::prepare(&self.workspace, &req.artifact_path, fresh)?;
        Ok(recipient_id)
    }

    pub(super) async fn send_information(
        &self,
        req: MessageSendRequest,
    ) -> Result<StepOutput, StepError> {
        let (app, run_id) = self.information_host()?;
        let state = app.state::<AppState>();
        // The engine passes the UUID from MessageSendPrepared, never the authored name.
        let recipient_id = req.recipient.clone();
        let (body, artifact_sha256) = message_artifact::read(&self.workspace, &req.artifact_path)
            .map_err(|error| {
            // Even a previously admitted artifact must still be present.
            // Expose its canonical ID for recovery without sending again.
            let evidence = store::with_db(|conn| {
                store::reconcile_host_automation_message(conn, run_id, &req.node, None)
            });
            let recovery = match evidence {
                Ok(Some(admitted)) => format!(
                    "; prior admission {} ({}) retained",
                    admitted.record.id, admitted.delivery_state
                ),
                Ok(None) => "; no prior admission".into(),
                Err(err) => format!("; admission outcome uncertain: {err}"),
            };
            StepError::new(format!("{error}{recovery}"))
        })?;
        let admitted = agent_messaging::admit_automation_information(
            &state,
            run_id,
            &req.node,
            &recipient_id,
            &body,
        )
        .await
        .map_err(|error| StepError::new(error.to_string()))?;
        agent_messaging::notify_information_admitted(app, &recipient_id, &admitted);
        Ok(StepOutput(serde_json::json!({
            "interaction_id": admitted.record.id,
            "run_id": run_id,
            "node": req.node,
            "recipient_id": recipient_id,
            "artifact_path": req.artifact_path,
            "artifact_sha256": artifact_sha256,
            "idempotency_key": store::host_automation_message_key(&req.node),
            "delivery_state": admitted.delivery_state,
            "duplicate": admitted.duplicate,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::runner::FakeAgentRunner;
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn continuation_provenance_comes_from_replayed_state_not_directory_or_caller() {
        use wardian_core::engine::Engine;
        let root = tempfile::tempdir().unwrap();
        let blueprint = wardian_core::automation::parse_str(
            "---\nschema: 2\nid: fixture\nname: Fixture\nnodes: []\nedges: []\n---\n",
        )
        .unwrap();
        Engine::initialize_with_id(
            &blueprint,
            "persisted-id",
            serde_json::json!({"run_id":"spoofed"}),
            root.path(),
        )
        .unwrap();
        let state = Engine::replay(&blueprint, root.path()).unwrap();
        let exec = LiveStepExecutor::new(
            Arc::new(FakeAgentRunner::new()),
            root.path().into(),
            "mock".into(),
            HashMap::new(),
            HashMap::new(),
        )
        .with_run_state(&state);
        assert_eq!(
            exec.automation_origin,
            Some(("fixture".into(), "persisted-id".into()))
        );
        assert_eq!(exec.owner_id, "fixture/persisted-id");
        assert_ne!(
            state.run_id,
            root.path().file_name().unwrap().to_str().unwrap()
        );
    }

    #[tokio::test]
    async fn live_executor_requires_host_before_preparing_artifact_or_reviewing() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeAgentRunner::new());
        let exec = LiveStepExecutor::new(
            runner.clone(),
            dir.path().into(),
            "mock".into(),
            HashMap::new(),
            HashMap::new(),
        )
        .with_automation_origin("fixture".into(), "run".into());
        let req = MessageSendRequest {
            node: "deliver".into(),
            recipient: "recipient".into(),
            artifact_path: "reviews/run/review.md".into(),
        };
        assert!(exec
            .preflight_information(req.clone(), true)
            .await
            .unwrap_err()
            .to_string()
            .contains("application host"));
        assert!(exec.send_information(req).await.is_err());
        assert!(!dir.path().join("reviews").exists());
        assert!(runner.calls().is_empty());
    }
}
