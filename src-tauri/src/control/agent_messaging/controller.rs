//! Explicit external-controller ingress. Never converts a controller to an agent.
use super::*;
use wardian_core::control::ControllerSecret;
use wardian_core::controllers::{ControllerPrincipal, ControllerScope as Scope, ControllerStore};

fn authenticate_controller(
    token: &ControllerSecret,
    resource: &str,
) -> Result<ControllerPrincipal, ControlError> {
    ControllerStore::current()
        .and_then(|store| store.authenticate_token_for_resource(token.expose(), resource))
        .map_err(|_| {
            ControlError::coded(
                "unauthorized",
                "Controller credential is invalid, expired or revoked.",
            )
        })
}
fn authorize(
    token: &ControllerSecret,
    resource: &str,
    scope: Scope,
    target: Option<&str>,
) -> Result<ControllerPrincipal, ControlError> {
    let principal = authenticate_controller(token, resource)?;
    principal.authorize(scope, target).map_err(|_| {
        ControlError::coded(
            "forbidden",
            "Controller grant does not permit this operation or target.",
        )
    })?;
    Ok(principal)
}

pub(crate) async fn handle(
    app: &AppHandle,
    request: Request,
    token: ControllerSecret,
    resource: String,
) -> Result<String, ControlError> {
    ok_json(
        &handle_in_state(
            Some(app),
            &app.state::<AppState>(),
            request,
            &token,
            &resource,
        )
        .await?,
    )
}

async fn handle_in_state(
    app: Option<&AppHandle>,
    state: &AppState,
    request: Request,
    token: &ControllerSecret,
    resource: &str,
) -> Result<Response, ControlError> {
    let principal = authenticate_controller(token, resource)?;
    let sender = principal.mailbox_owner();
    if let Request::SendMessage {
        idempotency_key: Some(key),
        ..
    }
    | Request::FollowupTask {
        idempotency_key: Some(key),
        ..
    } = &request
    {
        let valid = key
            .strip_prefix(&format!("{sender}:"))
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        if !valid {
            return Err(ControlError::coded(
                "invalid_idempotency_key",
                "Controller request key must belong to the authenticated principal.",
            ));
        }
    }
    match request {
        Request::ListAgents => {
            let principal = authorize(token, resource, Scope::AgentsRead, None)?;
            let sources = {
                let agents = state.agents.lock().await;
                collect_agent_snapshot_sources(&agents)
            };
            let leases = wardian_core::conversation_lease::load_leases();
            let now = chrono::Utc::now().to_rfc3339();
            let mut agents = sources
                .iter()
                .filter(|(id, _)| principal.agent_ids.contains(id))
                .map(|(_, source)| snapshot_agent_source(source, &leases, &now))
                .collect::<Vec<_>>();
            agents.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            Ok(Response::ListAgents { agents })
        }
        Request::SendMessage {
            target,
            message,
            idempotency_key,
        } => {
            authorize(token, resource, Scope::MessagesSend, Some(&target))?;
            let recipient = resolve_exact(state, &target).await?.id;
            admit(
                app,
                state,
                AdmissionInput {
                    sender: &sender,
                    target: &recipient,
                    message: &message,
                    key: idempotency_key.as_deref(),
                    task: false,
                },
            )
            .await
        }
        Request::FollowupTask {
            target,
            message,
            idempotency_key,
        } => {
            authorize(token, resource, Scope::TasksSubmit, Some(&target))?;
            let recipient = resolve_exact(state, &target).await?.id;
            // Task admission can wake an off worker; this capability is explicit.
            authorize(token, resource, Scope::AgentsWake, Some(&recipient))?;
            admit(
                app,
                state,
                AdmissionInput {
                    sender: &sender,
                    target: &recipient,
                    message: &message,
                    key: idempotency_key.as_deref(),
                    task: true,
                },
            )
            .await
        }
        Request::InterruptAgent { target } => {
            authorize(token, resource, Scope::AgentsInterrupt, Some(&target))?;
            let recipient = resolve_exact(state, &target).await?.id;
            native::interrupt(state, &recipient).await
        }
        Request::Reply { .. } => Err(ControlError::coded(
            "forbidden",
            "Controllers cannot reply as managed workers.",
        )),
        Request::ReceiveMessages {
            cursor,
            ack_cursor,
            limit,
            timeout_ms,
        } => {
            let limit = limit.unwrap_or(MAX_RECEIVE_ITEMS);
            let timeout = timeout_ms.unwrap_or(0);
            if limit == 0 || limit > MAX_RECEIVE_ITEMS || timeout > MAX_RECEIVE_TIMEOUT_MS {
                return Err(ControlError::coded(
                    "invalid_receive_bounds",
                    "Limit must be 1..100 and timeout_ms 0..60000.",
                ));
            }
            let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
            let mut mailbox = state.interactions.subscribe_agent_mailbox(&sender).await;
            loop {
                mailbox.borrow_and_update();
                authorize(token, resource, Scope::ControllerReceive, None)?;
                if ack_cursor.is_some() {
                    authorize(token, resource, Scope::ControllerAck, None)?;
                }
                state
                    .interactions
                    .recover_agent_task_results()
                    .await
                    .map_err(control_error)?;
                let mut page = state
                    .interactions
                    .receive_agent_messages(
                        &sender,
                        cursor.as_deref(),
                        ack_cursor.as_deref(),
                        limit,
                    )
                    .await
                    .map_err(control_error)?;
                authorize(token, resource, Scope::ControllerReceive, None)?;
                if !page.messages.is_empty() || timeout == 0 {
                    return Ok(Response::ReceiveMessages { page });
                }
                if tokio::time::Instant::now() >= deadline {
                    page.timed_out = true;
                    return Ok(Response::ReceiveMessages { page });
                }
                // Revocation/expiry must end a quiet wait too, without provider I/O.
                let _ = tokio::time::timeout(Duration::from_millis(250), mailbox.changed()).await;
            }
        }
        Request::WaitAgent { timeout_ms } => {
            let timeout = timeout_ms.unwrap_or(MAX_WAIT_AGENT_TIMEOUT_MS);
            if timeout > MAX_WAIT_AGENT_TIMEOUT_MS {
                return Err(ControlError::coded(
                    "invalid_wait_bounds",
                    "timeout_ms must be 0..60000.",
                ));
            }
            let mut mailbox = state.interactions.subscribe_agent_mailbox(&sender).await;
            let observed = mailbox.borrow().revision;
            let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
            loop {
                authorize(token, resource, Scope::ControllerWait, None)?;
                state
                    .interactions
                    .recover_agent_task_results()
                    .await
                    .map_err(control_error)?;
                let unread = state
                    .interactions
                    .has_unacknowledged_agent_messages(&sender)
                    .await
                    .map_err(control_error)?;
                if unread || mailbox.borrow().revision != observed {
                    return Ok(Response::WaitAgent { timed_out: false });
                }
                if tokio::time::Instant::now() >= deadline {
                    return Ok(Response::WaitAgent { timed_out: true });
                }
                let _ = tokio::time::timeout(Duration::from_millis(250), mailbox.changed()).await;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::test_support::TestWardianHome;
    use wardian_core::control::ReplyStatus;
    const WORKER: &str = "ea503f0d-e934-4c72-9185-3e780b889c29";
    const OTHER: &str = "99a6e1cf-d765-4282-8fc5-d6a1bececc69";
    async fn fixture(state: &AppState) {
        for id in [WORKER, OTHER] {
            let agent = crate::control::tests::test_agent(id, id, "Test");
            *agent.current_status.lock().unwrap() = "Off".into();
            state.agents.lock().await.insert(id.into(), agent);
        }
    }
    fn receive_request(ack_cursor: Option<String>, timeout: u64) -> Request {
        Request::ReceiveMessages {
            cursor: None,
            ack_cursor,
            limit: None,
            timeout_ms: Some(timeout),
        }
    }
    fn issue(scopes: Vec<Scope>) -> (ControllerStore, wardian_core::controllers::IssuedController) {
        let store = ControllerStore::current().unwrap();
        let grant = store
            .create(
                "Test controller",
                "https://bridge.example/mcp",
                vec![WORKER.into()],
                scopes,
                None,
            )
            .unwrap();
        (store, grant)
    }
    #[tokio::test]
    async fn controller_task_worker_reply_and_ack_use_canonical_mailbox_without_agent() {
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        fixture(&state).await;
        let (_, grant) = issue(vec![
            Scope::TasksSubmit,
            Scope::AgentsWake,
            Scope::ControllerReceive,
            Scope::ControllerAck,
            Scope::AgentsRead,
        ]);
        let owner = grant.principal.mailbox_owner();
        let token = ControllerSecret::new(grant.token);
        assert!(!format!("{token:?}").contains(token.expose()));
        let listed = handle_in_state(
            None,
            &state,
            Request::ListAgents,
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap();
        assert!(
            matches!(listed, Response::ListAgents { agents } if agents.len() == 1 && agents[0].uuid == WORKER)
        );
        let response = handle_in_state(
            None,
            &state,
            Request::FollowupTask {
                target: WORKER.into(),
                message: "do work".into(),
                idempotency_key: Some(wardian_core::control::controller_request_key(
                    &owner, "task-1",
                )),
            },
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap();
        let Response::FollowupTask { request_id, .. } = response else {
            panic!("task")
        };
        let claim = state
            .interactions
            .claim_agent_task(WORKER, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            claim.record.sender_session_id.as_deref(),
            Some(owner.as_str())
        );
        assert_eq!(claim.record.id, request_id);
        state
            .interactions
            .finish_agent_task(&claim, "provider_visible")
            .await
            .unwrap();
        super::super::handle_in_state(
            None,
            &state,
            Request::Reply {
                request_id: request_id.clone(),
                status: ReplyStatus::Done,
                message: "done".into(),
            },
            MessageOrigin::WardianAgent {
                session_id: WORKER.into(),
            },
        )
        .await
        .unwrap();
        let Response::ReceiveMessages { page } = handle_in_state(
            None,
            &state,
            receive_request(None, 0),
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap() else {
            panic!("receive")
        };
        assert_eq!(page.messages.len(), 1);
        assert_eq!(
            page.messages[0].parent_interaction_id.as_deref(),
            Some(request_id.as_str())
        );
        assert_eq!(page.messages[0].sender, WORKER);
        let Response::ReceiveMessages { page } = handle_in_state(
            None,
            &state,
            receive_request(Some(page.next_cursor), 0),
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap() else {
            panic!("ack")
        };
        assert!(page.messages.is_empty());
        assert_eq!(state.agents.lock().await.len(), 2);
        drop(state);
        drop(_home);
        // Qualification explicitly opts into HTTP/real CLI; baseline remains canonical DB coverage.
        if std::env::var_os("WARDIAN_BRIDGE_E2E_DRIVER").is_some() {
            super::e2e::run_http_real_cli_canonical_task_reply().await;
        }
    }
    #[tokio::test]
    async fn controller_permissions_nonwake_and_revocation_are_enforced() {
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        fixture(&state).await;
        let (store, grant) = issue(vec![
            Scope::MessagesSend,
            Scope::TasksSubmit,
            Scope::ControllerReceive,
            Scope::ControllerWait,
        ]);
        let token = ControllerSecret::new(grant.token);
        handle_in_state(
            None,
            &state,
            Request::SendMessage {
                target: WORKER.into(),
                message: "info".into(),
                idempotency_key: None,
            },
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap();
        assert!(state
            .interactions
            .claim_agent_task(WORKER, 0)
            .await
            .unwrap()
            .is_none());
        for request in [
            Request::SendMessage {
                target: OTHER.into(),
                message: "no".into(),
                idempotency_key: None,
            },
            Request::FollowupTask {
                target: WORKER.into(),
                message: "no wake".into(),
                idempotency_key: None,
            },
            Request::InterruptAgent {
                target: WORKER.into(),
            },
            Request::Reply {
                request_id: "ask-fake".into(),
                status: ReplyStatus::Done,
                message: "fake".into(),
            },
            receive_request(Some("fake-ack".into()), 0),
        ] {
            assert_eq!(
                handle_in_state(None, &state, request, &token, "https://bridge.example/mcp")
                    .await
                    .unwrap_err()
                    .code,
                "forbidden"
            );
        }
        let waiting = handle_in_state(
            None,
            &state,
            Request::WaitAgent {
                timeout_ms: Some(5000),
            },
            &token,
            "https://bridge.example/mcp",
        );
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            store.revoke(&grant.principal.id).unwrap();
        };
        let (result, _) = tokio::join!(waiting, revoke);
        assert_eq!(result.unwrap_err().code, "unauthorized");
        assert!(store.registered(&grant.principal.mailbox_owner()).unwrap());
        assert_eq!(
            handle_in_state(
                None,
                &state,
                receive_request(None, 0),
                &token,
                "https://bridge.example/mcp"
            )
            .await
            .unwrap_err()
            .code,
            "unauthorized"
        );
    }
    #[tokio::test]
    async fn selected_worker_information_is_a_nonwaking_controller_event() {
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        fixture(&state).await;
        let (store, grant) = issue(vec![
            Scope::ControllerReceive,
            Scope::TasksSubmit,
            Scope::AgentsWake,
        ]);
        let owner = grant.principal.mailbox_owner();
        let token = ControllerSecret::new(grant.token);
        async fn send(
            state: &AppState,
            owner: &str,
            sender: &str,
            task: bool,
        ) -> Result<Response, ControlError> {
            let request = if task {
                Request::FollowupTask {
                    target: owner.into(),
                    message: "event".into(),
                    idempotency_key: None,
                }
            } else {
                Request::SendMessage {
                    target: owner.into(),
                    message: "event".into(),
                    idempotency_key: None,
                }
            };
            super::super::handle_in_state(
                None,
                state,
                request,
                MessageOrigin::WardianAgent {
                    session_id: sender.into(),
                },
            )
            .await
        }
        send(&state, &owner, WORKER, false).await.unwrap();
        assert!(state
            .interactions
            .claim_agent_task(&owner, 0)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            send(&state, &owner, OTHER, false).await.unwrap_err().code,
            "forbidden"
        );
        assert_eq!(
            send(&state, &owner, WORKER, true).await.unwrap_err().code,
            "invalid_target"
        );
        let Response::ReceiveMessages { page } = handle_in_state(
            None,
            &state,
            receive_request(None, 0),
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap() else {
            panic!("event")
        };
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].sender, WORKER);
        assert_eq!(page.messages[0].message, "event");
        assert_eq!(state.agents.lock().await.len(), 2);
        let task = handle_in_state(
            None,
            &state,
            Request::FollowupTask {
                target: WORKER.into(),
                message: "accepted work".into(),
                idempotency_key: None,
            },
            &token,
            "https://bridge.example/mcp",
        )
        .await
        .unwrap();
        let Response::FollowupTask { request_id, .. } = task else {
            panic!("task")
        };
        let claim = state
            .interactions
            .claim_agent_task(WORKER, 0)
            .await
            .unwrap()
            .unwrap();
        state
            .interactions
            .finish_agent_task(&claim, "provider_visible")
            .await
            .unwrap();
        store.revoke(&grant.principal.id).unwrap();
        assert_eq!(
            send(&state, &owner, WORKER, false).await.unwrap_err().code,
            "forbidden"
        );
        // Revocation prevents fresh events/access, while an already accepted task
        // still records its canonical result without spawning a controller provider.
        super::super::handle_in_state(
            None,
            &state,
            Request::Reply {
                request_id: request_id.clone(),
                status: ReplyStatus::Done,
                message: "durable after revoke".into(),
            },
            MessageOrigin::WardianAgent {
                session_id: WORKER.into(),
            },
        )
        .await
        .unwrap();
        let page = state
            .interactions
            .receive_agent_messages(&owner, None, None, 100)
            .await
            .unwrap();
        assert!(page
            .messages
            .iter()
            .any(
                |message| message.parent_interaction_id.as_deref() == Some(request_id.as_str())
                    && message.message == "durable after revoke"
            ));
    }
}
#[cfg(test)]
#[path = "controller_e2e.rs"]
mod e2e;
