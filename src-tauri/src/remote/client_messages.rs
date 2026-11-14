use crate::remote::models::{
    DeviceCredentialKind, RemoteClientMessageOperation, RemoteClientSessionRecord,
    RemoteCredentialAudience, RemoteSendGrant,
};
use crate::state::AppState;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

const IDEMPOTENCY_NAMESPACE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MESSAGE_DEADLINE_MS: i64 = 30 * 1000;

#[derive(Debug, Clone)]
pub(crate) struct ClientMessageOperationRecord {
    pub operation_id: String,
    pub request_id: String,
    pub device_id: String,
    pub client_origin: String,
    pub audience: String,
    pub gateway_origin: String,
    pub session_id: String,
    pub payload_hash: String,
    pub text: String,
    pub input_mode: String,
    pub delivery_state: String,
    pub accepted_at: String,
    pub updated_at: String,
    pub delivery_deadline_at: String,
    pub idempotency_expires_at_ms: i64,
    pub error_code: Option<String>,
    pub native_intent_id: String,
}

impl ClientMessageOperationRecord {
    pub(crate) fn to_dto(&self) -> RemoteClientMessageOperation {
        RemoteClientMessageOperation {
            operation_id: self.operation_id.clone(),
            request_id: self.request_id.clone(),
            session_id: self.session_id.clone(),
            delivery_state: self.delivery_state.clone(),
            accepted_at: self.accepted_at.clone(),
            updated_at: self.updated_at.clone(),
            idempotency_expires_at: millis_to_rfc3339(self.idempotency_expires_at_ms),
            error_code: self.error_code.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ClientSendAuthority {
    config_lock: Arc<Mutex<()>>,
    device_store_lock: Arc<Mutex<()>>,
    pub device_id: String,
    pub client_origin: String,
    pub gateway_origin: String,
    pub audience: String,
    pub deadline_at: String,
}

pub(crate) struct ClientSendAuthorityPermit {
    _config_guard: OwnedMutexGuard<()>,
    _device_guard: OwnedMutexGuard<()>,
}

pub(crate) fn is_authority_failure_code(code: &str) -> bool {
    matches!(
        code,
        "service_unavailable"
            | "deadline_expired"
            | "origin_forbidden"
            | "device_revoked"
            | "grant_required"
    )
}

pub(crate) fn authority_failure_code_in_message(message: &str) -> Option<&'static str> {
    let candidate = message
        .strip_prefix("OpenCode HTTP TransportUnavailable: ")
        .unwrap_or(message);
    match candidate {
        "service_unavailable" => Some("service_unavailable"),
        "deadline_expired" => Some("deadline_expired"),
        "origin_forbidden" => Some("origin_forbidden"),
        "device_revoked" => Some("device_revoked"),
        "grant_required" => Some("grant_required"),
        _ => None,
    }
}

impl ClientSendAuthority {
    /// Rechecks the original deadline and live grants, returning guards that
    /// serialize the send against origin removal and device revocation. The
    /// deadline is checked again after persisted authority validation; the
    /// final path holds the permit only through provider submission.
    pub(crate) async fn acquire(&self) -> Result<ClientSendAuthorityPermit, &'static str> {
        self.acquire_with_clock(chrono::Utc::now).await
    }

    async fn acquire_with_clock<F>(
        &self,
        mut now: F,
    ) -> Result<ClientSendAuthorityPermit, &'static str>
    where
        F: FnMut() -> chrono::DateTime<chrono::Utc>,
    {
        let config_guard = self.config_lock.clone().lock_owned().await;
        let device_guard = self.device_store_lock.clone().lock_owned().await;
        let deadline = chrono::DateTime::parse_from_rfc3339(&self.deadline_at)
            .map_err(|_| "service_unavailable")?;
        if now() >= deadline {
            return Err("deadline_expired");
        }
        let config = crate::remote::storage::load_remote_config()
            .map_err(|_| "service_unavailable")?
            .ok_or("service_unavailable")?;
        if !config.enabled
            || config.canonical_origin != self.gateway_origin
            || !crate::remote::policy::is_approved_client_origin(
                &config.client_origin_allowlist,
                &self.client_origin,
            )
        {
            return Err("origin_forbidden");
        }
        let store =
            crate::remote::storage::load_device_store().map_err(|_| "service_unavailable")?;
        let device = store
            .devices
            .iter()
            .find(|device| device.device_id == self.device_id)
            .ok_or("device_revoked")?;
        if device.revoked_at.is_some() {
            return Err("device_revoked");
        }
        if device.credential_kind != DeviceCredentialKind::PairedClient
            || device.audience != RemoteCredentialAudience::PairedClientV1
            || device.audience.as_str() != self.audience
            || device.client_origin.as_deref() != Some(self.client_origin.as_str())
            || !device.send_grants.contains(&RemoteSendGrant::ChatSend)
        {
            return Err("grant_required");
        }
        if now() >= deadline {
            return Err("deadline_expired");
        }
        Ok(ClientSendAuthorityPermit {
            _config_guard: config_guard,
            _device_guard: device_guard,
        })
    }
}

/// Remote text cannot contain terminal control instructions. Newlines are
/// bracketed by provider profiles; tabs remain ordinary message data.
pub(crate) fn valid_message_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 65_536
        && !text
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
}

/// Checks live authority at every terminal write, including payload writes
/// that could otherwise execute embedded provider control sequences.
pub(crate) struct AuthorityInputSink<'a> {
    input: &'a dyn crate::utils::delivery_transaction::TerminalInputSink,
    authority: ClientSendAuthority,
}

impl<'a> AuthorityInputSink<'a> {
    pub(crate) fn new(
        input: &'a dyn crate::utils::delivery_transaction::TerminalInputSink,
        authority: ClientSendAuthority,
    ) -> Self {
        Self { input, authority }
    }
}

impl crate::utils::delivery_transaction::TerminalInputSink for AuthorityInputSink<'_> {
    fn send_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(), crate::utils::delivery_transaction::TerminalInputError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let _permit = self.authority.acquire().await.map_err(|code| {
                crate::utils::delivery_transaction::TerminalInputError {
                    message: code.to_string(),
                    retry_safe: true,
                }
            })?;
            self.input.send_bytes(bytes).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{RemoteReadGrant, RemoteSendGrant};
    use crate::utils::delivery_transaction::{TerminalInputError, TerminalInputSink};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ControlledTerminalInputSink {
        writes: std::sync::Mutex<Vec<Vec<u8>>>,
        submit_key: Vec<u8>,
        submit_entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release_submit: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl ControlledTerminalInputSink {
        fn new(
            submit_key: Vec<u8>,
            submit_entered: Option<tokio::sync::oneshot::Sender<()>>,
            release_submit: Option<tokio::sync::oneshot::Receiver<()>>,
        ) -> Self {
            Self {
                writes: std::sync::Mutex::new(Vec::new()),
                submit_key,
                submit_entered: tokio::sync::Mutex::new(submit_entered),
                release_submit: tokio::sync::Mutex::new(release_submit),
            }
        }

        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().expect("recorded sink writes").clone()
        }
    }

    impl TerminalInputSink for ControlledTerminalInputSink {
        fn send_bytes(
            &self,
            bytes: Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Result<(), TerminalInputError>> + Send + '_>> {
            Box::pin(async move {
                let is_submit_key = bytes == self.submit_key;
                self.writes
                    .lock()
                    .expect("recorded sink writes")
                    .push(bytes);
                if is_submit_key {
                    if let Some(entered) = self.submit_entered.lock().await.take() {
                        let _ = entered.send(());
                    }
                    let release = self.release_submit.lock().await.take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                }
                Ok(())
            })
        }
    }

    fn configure_send_authority_home(home: &std::path::Path) {
        let config = crate::remote::models::RemoteGatewayConfig {
            schema_version: crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://gateway.example".to_string(),
            client_origin_allowlist: vec!["https://client.example".to_string()],
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 4040,
            gateway_identity_public_key: "gateway-public-key".to_string(),
            gateway_identity_fingerprint: "gateway-fingerprint".to_string(),
        };
        crate::remote::storage::save_remote_config_at(home, &config)
            .expect("save bound gateway config");
        crate::remote::storage::update_device_store_at(home, |store| {
            store.devices.push(crate::remote::models::DeviceRecord {
                device_id: "client-device".to_string(),
                label: "Client".to_string(),
                public_key_spki_der_base64: "public-key".to_string(),
                public_key_fingerprint: "key-fingerprint".to_string(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: Vec::new(),
                send_grants: vec![RemoteSendGrant::ChatSend],
                created_at: "2026-10-31T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("save send-enabled device");
    }

    fn bound_send_authority(state: &AppState, deadline_at: String) -> ClientSendAuthority {
        ClientSendAuthority::from_binding(
            state,
            "client-device".to_string(),
            "https://client.example".to_string(),
            "https://gateway.example".to_string(),
            RemoteCredentialAudience::PairedClientV1
                .as_str()
                .to_string(),
            deadline_at,
        )
    }

    #[test]
    fn terminal_control_text_is_rejected_without_changing_ordinary_text() {
        assert!(valid_message_text("hello\nworld\t!"));
        for text in ["hi\u{1b}[13u", "hi\r", "hi\u{009b}13u", "hi\0"] {
            assert!(!valid_message_text(text));
        }
    }

    #[test]
    fn dispatch_errors_preserve_typed_uncertainty_and_the_actual_delivery_envelope() {
        for details in [
            serde_json::json!({"retry_safe": true, "delivery": [{"delivery_state": "submitted_unconfirmed"}]}),
            serde_json::json!({"retry_safe": false, "delivery": [{"delivery_state": "failed", "delivery_phase": "provider_turn_start_timeout"}]}),
            serde_json::json!({"delivery": [{"delivery_state": "failed"}]}),
        ] {
            assert_eq!(
                classify_dispatch_error(Some(&details)),
                ClientMessageDispatchOutcome::SubmittedUnconfirmed
            );
        }
        assert_eq!(
            classify_dispatch_error(None),
            ClientMessageDispatchOutcome::SubmittedUnconfirmed
        );
        let rejected =
            serde_json::json!({"retry_safe": true, "delivery": [{"delivery_state": "rejected"}]});
        assert_eq!(
            classify_dispatch_error(Some(&rejected)),
            ClientMessageDispatchOutcome::FailedBeforeSubmit(None)
        );
    }

    #[tokio::test]
    async fn revoked_authority_blocks_payload_with_actual_opencode_submit_bytes() {
        let temp = tempfile::tempdir().expect("isolated authority home");
        let _home = IsolatedWardianHome::set(temp.path()).await;
        wardian_core::db::init_db().expect("initialize isolated state database");
        configure_send_authority_home(temp.path());
        let state = AppState::new();
        let authority = bound_send_authority(
            &state,
            millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 60_000),
        );
        let sink = ControlledTerminalInputSink::new(b"\x1b[13u".to_vec(), None, None);
        let input = AuthorityInputSink::new(&sink, authority);
        input
            .send_bytes(b"ordinary text".to_vec())
            .await
            .expect("authorized payload");
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices[0].revoked_at = Some("2026-11-13T00:00:00Z".to_string());
            Ok(())
        })
        .expect("publish revocation before submission-capable payload");
        let error = input
            .send_bytes(b"hi\x1b[13u".to_vec())
            .await
            .expect_err("revoked payload blocked");
        assert!(error.retry_safe);
        assert_eq!(error.message, "device_revoked");
        assert_eq!(sink.writes(), [b"ordinary text".to_vec()]);
        assert!(input.send_bytes(b"\x1b[13u".to_vec()).await.is_err());
        assert_eq!(sink.writes().len(), 1);
    }

    fn controlled_headless_command(
        started_path: &std::path::Path,
        release_path: &std::path::Path,
    ) -> tokio::process::Command {
        let script = "const fs=require('node:fs'); fs.writeFileSync(process.env.WARDIAN_AUTHORITY_TEST_STARTED,'started'); const timer=setInterval(()=>{ if(fs.existsSync(process.env.WARDIAN_AUTHORITY_TEST_RELEASE)){ clearInterval(timer); process.exit(0); } },10);";
        let mut command = tokio::process::Command::new("node");
        command
            .arg("-e")
            .arg(script)
            .env("WARDIAN_AUTHORITY_TEST_STARTED", started_path)
            .env("WARDIAN_AUTHORITY_TEST_RELEASE", release_path)
            .kill_on_drop(true);
        command
    }

    async fn wait_for_file(path: &std::path::Path) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("controlled provider process reached its start marker");
    }

    struct IsolatedWardianHome {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
    }

    impl IsolatedWardianHome {
        async fn set(home: &std::path::Path) -> Self {
            let guard = Self {
                _lock: crate::utils::wardian_test_env_lock_async().await,
                previous_home: std::env::var_os("WARDIAN_HOME"),
            };
            std::env::set_var("WARDIAN_HOME", home);
            guard
        }
    }

    impl Drop for IsolatedWardianHome {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn test_session(namespace_id: String, expires_at_ms: i64) -> RemoteClientSessionRecord {
        RemoteClientSessionRecord {
            session_id: "client-session".to_string(),
            device_id: "client-device".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "gateway-fingerprint".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: vec![RemoteSendGrant::ChatSend],
            idempotency_namespace: Some(namespace_id),
            idempotency_expires_at_ms: Some(expires_at_ms),
            expires_at_ms,
            revoked: false,
        }
    }

    #[test]
    fn authority_failure_code_recognizes_only_known_final_recheck_errors() {
        assert_eq!(
            authority_failure_code_in_message("device_revoked"),
            Some("device_revoked")
        );
        assert_eq!(
            authority_failure_code_in_message(
                "OpenCode HTTP TransportUnavailable: deadline_expired"
            ),
            Some("deadline_expired")
        );
        assert_eq!(
            authority_failure_code_in_message("provider reported device_revoked"),
            None
        );
    }

    #[tokio::test]
    async fn deadline_expiring_during_persisted_authority_validation_blocks_submit() {
        let temp = tempfile::tempdir().expect("isolated authority home");
        let _home = IsolatedWardianHome::set(temp.path()).await;
        wardian_core::db::init_db().expect("initialize isolated state database");
        configure_send_authority_home(temp.path());
        let state = AppState::new();

        let deadline = chrono::Utc::now() + chrono::Duration::seconds(30);
        let deadline_at = deadline.to_rfc3339();
        let before_validation = deadline - chrono::Duration::seconds(1);
        let after_validation = deadline + chrono::Duration::milliseconds(1);
        let expired_before_validation = before_validation;
        let expired_after_validation = after_validation;
        let mut profile = crate::utils::delivery_profile::delivery_profile("gemini");
        profile.submit_delay_ms = 0;
        let expired_plan = crate::utils::delivery_transaction::plan_terminal_payload(
            &profile,
            "expires during persisted validation",
        );
        let expired_sink =
            ControlledTerminalInputSink::new(expired_plan.submit_key.clone(), None, None);
        let expired_authority = bound_send_authority(&state, deadline_at.clone());
        let expired_clock_calls = Arc::new(AtomicUsize::new(0));
        let expired_clock_count = expired_clock_calls.clone();
        let expired_clock = move || {
            if expired_clock_count.fetch_add(1, Ordering::SeqCst) == 0 {
                expired_before_validation
            } else {
                expired_after_validation
            }
        };
        let expired_result =
            crate::utils::delivery_transaction::submit_terminal_transaction_with_submit_guard(
                &expired_sink,
                &profile,
                "expires during persisted validation",
                || async { Ok(()) },
                || async { Ok(()) },
                move || async move {
                    expired_authority
                        .acquire_with_clock(expired_clock)
                        .await
                        .map(Some)
                        .map_err(
                            |code| crate::utils::delivery_transaction::TerminalDeliveryError {
                                phase: code,
                                message: code.to_string(),
                                retry_safe: true,
                                observed_state: Some("no_submit_key_sent".to_string()),
                                reason: Some(
                                    "authority deadline expired during persisted validation"
                                        .to_string(),
                                ),
                            },
                        )
                },
            )
            .await
            .expect_err("final deadline recheck rejects an expired authority");
        assert_eq!(expired_result.phase, "deadline_expired");
        assert_eq!(expired_clock_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            expired_sink.writes(),
            vec![expired_plan.payload_bytes],
            "validation expiry writes no provider submit key"
        );

        let control_plan = crate::utils::delivery_transaction::plan_terminal_payload(
            &profile,
            "still within deadline",
        );
        let control_sink =
            ControlledTerminalInputSink::new(control_plan.submit_key.clone(), None, None);
        let control_authority = bound_send_authority(&state, deadline_at);
        let control_clock_calls = Arc::new(AtomicUsize::new(0));
        let control_clock_count = control_clock_calls.clone();
        let control_time = before_validation;
        let control_clock = move || {
            control_clock_count.fetch_add(1, Ordering::SeqCst);
            control_time
        };
        crate::utils::delivery_transaction::submit_terminal_transaction_with_submit_guard(
            &control_sink,
            &profile,
            "still within deadline",
            || async { Ok(()) },
            || async { Ok(()) },
            move || async move {
                control_authority
                    .acquire_with_clock(control_clock)
                    .await
                    .map(Some)
                    .map_err(
                        |code| crate::utils::delivery_transaction::TerminalDeliveryError {
                            phase: code,
                            message: code.to_string(),
                            retry_safe: true,
                            observed_state: Some("no_submit_key_sent".to_string()),
                            reason: Some("authority rejected before provider submit".to_string()),
                        },
                    )
            },
        )
        .await
        .expect("authority still valid after persisted validation permits submit");
        assert_eq!(control_clock_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            control_sink.writes(),
            vec![control_plan.payload_bytes, control_plan.submit_key],
            "still-valid control reaches the provider submit boundary"
        );
    }

    #[tokio::test]
    async fn durable_idempotency_and_uncertain_recovery_never_repeat_the_sink() {
        let temp = tempfile::tempdir().expect("isolated database home");
        let _home = IsolatedWardianHome::set(temp.path()).await;
        wardian_core::db::init_db().expect("initialize isolated state database");

        let now_ms = chrono::Utc::now().timestamp_millis();
        let (namespace_id, expires_at_ms) = get_or_create_namespace(
            "client-device".to_string(),
            "https://client.example".to_string(),
            RemoteCredentialAudience::PairedClientV1,
            now_ms,
        )
        .await
        .expect("durable idempotency namespace");
        let session = test_session(namespace_id.clone(), expires_at_ms);
        let config = crate::remote::models::RemoteGatewayConfig {
            schema_version: crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://gateway.example".to_string(),
            client_origin_allowlist: vec!["https://client.example".to_string()],
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 4040,
            gateway_identity_public_key: "gateway-public-key".to_string(),
            gateway_identity_fingerprint: "gateway-fingerprint".to_string(),
        };
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save bound gateway config");
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices.push(crate::remote::models::DeviceRecord {
                device_id: "client-device".to_string(),
                label: "Client".to_string(),
                public_key_spki_der_base64: "public-key".to_string(),
                public_key_fingerprint: "key-fingerprint".to_string(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
                send_grants: vec![RemoteSendGrant::ChatSend],
                created_at: "2026-10-31T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("save send-enabled device");
        let (rotated_namespace, rotated_expiry) = get_or_create_namespace(
            "client-device".to_string(),
            "https://client.example".to_string(),
            RemoteCredentialAudience::PairedClientV1,
            now_ms + 60_000,
        )
        .await
        .expect("reauthenticated namespace");
        assert_eq!(rotated_namespace, namespace_id);
        assert_eq!(rotated_expiry, expires_at_ms);
        let key_request_id = uuid::Uuid::new_v4().to_string();
        let key = format!("{namespace_id}.{key_request_id}");
        assert_eq!(
            parse_idempotency_key(&session, &key).expect("canonical key"),
            (namespace_id.clone(), key_request_id.clone())
        );
        assert_eq!(
            parse_idempotency_key(&session, &format!("{}.{key_request_id}", "u".repeat(43)),),
            Err("idempotency_namespace_expired_or_unknown")
        );
        assert_eq!(
            parse_idempotency_key(
                &session,
                &format!("{namespace_id}.{}", key_request_id.to_uppercase())
            ),
            Err("invalid_idempotency_key")
        );

        let request_id = uuid::Uuid::new_v4().to_string();
        let exact_text = "  exact text\nwith Unicode 🛰️  ";
        let (first, inserted) = reserve_operation(
            &session,
            &namespace_id,
            &request_id,
            "agent-one",
            exact_text,
            now_ms,
        )
        .expect("reserve the operation and outbox");
        assert!(inserted);
        let (duplicate, inserted) = reserve_operation(
            &session,
            &namespace_id,
            &request_id,
            "agent-one",
            exact_text,
            now_ms + 1,
        )
        .expect("same key and payload returns the operation");
        assert!(!inserted);
        assert_eq!(duplicate.operation_id, first.operation_id);
        assert_eq!(duplicate.text, exact_text);
        assert_eq!(duplicate.delivery_state, "admitted");
        assert!(reserve_operation(
            &session,
            &namespace_id,
            &request_id,
            "missing-agent",
            exact_text,
            now_ms + 2,
        )
        .unwrap_err()
        .contains("idempotency_conflict"));
        assert!(reserve_operation(
            &session,
            &namespace_id,
            &request_id,
            "agent-one",
            "changed text",
            now_ms + 2,
        )
        .unwrap_err()
        .contains("idempotency_conflict"));

        let concurrent_request_id = uuid::Uuid::new_v4().to_string();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let session = Arc::new(session);
        let mut workers = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            let session = session.clone();
            let namespace_id = namespace_id.clone();
            let request_id = concurrent_request_id.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                reserve_operation(
                    &session,
                    &namespace_id,
                    &request_id,
                    "agent-one",
                    exact_text,
                    now_ms + 3,
                )
                .expect("concurrent equal intent reservation")
            }));
        }
        let concurrent = workers
            .into_iter()
            .map(|worker| worker.join().expect("reservation worker"))
            .collect::<Vec<_>>();
        assert_eq!(
            concurrent
                .iter()
                .map(|(record, _)| record.operation_id.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert_eq!(
            concurrent.iter().filter(|(_, inserted)| *inserted).count(),
            1
        );

        let sink_submissions = Arc::new(AtomicUsize::new(0));
        let app_state = AppState::new();
        let admitted_id = uuid::Uuid::new_v4().to_string();
        let (admitted, _) = reserve_operation(
            &session,
            &namespace_id,
            &admitted_id,
            "agent-one",
            "dispatch after restart",
            now_ms + 4,
        )
        .expect("admitted restart intent");
        recover_interrupted_operations().expect("restart leaves an unclaimed admission retryable");
        let sink = sink_submissions.clone();
        let accepted = dispatch_if_admitted_with(
            &app_state,
            admitted.clone(),
            Box::new(
                move |_, _| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send>> {
                    Box::pin(async move {
                        sink.fetch_add(1, Ordering::SeqCst);
                        ClientMessageDispatchOutcome::Observed("provider_accepted".to_string())
                    })
                },
            ),
        )
        .await
        .expect("admitted operation dispatch");
        assert_eq!(accepted.delivery_state, "provider_accepted");
        let duplicate_dispatch = dispatch_if_admitted_with(
            &app_state,
            admitted,
            Box::new(
                |_, _| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send>> {
                    Box::pin(async {
                        panic!("a completed operation must not reach the provider sink")
                    })
                },
            ),
        )
        .await
        .expect("duplicate dispatch returns the durable operation");
        assert_eq!(duplicate_dispatch.delivery_state, "provider_accepted");
        assert_eq!(sink_submissions.load(Ordering::SeqCst), 1);

        let before_crossing_id = uuid::Uuid::new_v4().to_string();
        let (before_crossing, _) = reserve_operation(
            &session,
            &namespace_id,
            &before_crossing_id,
            "agent-one",
            "crash before provider boundary",
            now_ms + 5,
        )
        .expect("pre-boundary crash intent");
        assert!(claim_dispatch(&before_crossing.operation_id, now_ms + 6).expect("claim dispatch"));
        recover_interrupted_operations().expect("recover dispatching row before crossing");
        let sink = sink_submissions.clone();
        let recovered_before = dispatch_if_admitted_with(
            &app_state,
            before_crossing,
            Box::new(
                move |_, _| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send>> {
                    Box::pin(async move {
                        sink.fetch_add(1, Ordering::SeqCst);
                        ClientMessageDispatchOutcome::Observed("provider_accepted".to_string())
                    })
                },
            ),
        )
        .await
        .expect("uncertain pre-boundary recovery status");
        assert_eq!(recovered_before.delivery_state, "submitted_unconfirmed");
        assert_eq!(sink_submissions.load(Ordering::SeqCst), 1);

        let after_crossing_id = uuid::Uuid::new_v4().to_string();
        let (after_crossing, _) = reserve_operation(
            &session,
            &namespace_id,
            &after_crossing_id,
            "agent-one",
            "crash after provider boundary",
            now_ms + 7,
        )
        .expect("post-boundary crash intent");
        assert!(claim_dispatch(&after_crossing.operation_id, now_ms + 8).expect("claim dispatch"));
        sink_submissions.fetch_add(1, Ordering::SeqCst);
        recover_interrupted_operations().expect("recover dispatching row after crossing");
        let sink = sink_submissions.clone();
        let recovered_after = dispatch_if_admitted_with(
            &app_state,
            after_crossing,
            Box::new(
                move |_, _| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send>> {
                    Box::pin(async move {
                        sink.fetch_add(1, Ordering::SeqCst);
                        ClientMessageDispatchOutcome::Observed("provider_accepted".to_string())
                    })
                },
            ),
        )
        .await
        .expect("uncertain post-boundary recovery status");
        assert_eq!(recovered_after.delivery_state, "submitted_unconfirmed");
        assert_eq!(sink_submissions.load(Ordering::SeqCst), 2);
        let uncertainty_fence = load_operation_by_id_public(&recovered_after.operation_id)
            .expect("durable uncertain operation state");
        assert_eq!(uncertainty_fence.delivery_state, "submitted_unconfirmed");

        let target_ids = serde_json::to_string(&vec!["agent-one"]).expect("target ID JSON");
        let native_intent_id = recovered_after.native_intent_id.clone();
        wardian_core::db::get_db_conn(|connection| {
            connection.execute(
                "INSERT INTO interactions (
                    id, kind, sender_session_id, target_session_ids, status, trigger_policy,
                    body_ref, parent_interaction_id, created_at, updated_at
                 ) VALUES (?1, 'message', NULL, ?2, 'delivering', 'start_turn', '{}', NULL, ?3, ?3)",
                params![native_intent_id, target_ids, recovered_after.accepted_at],
            )?;
            connection.execute(
                "INSERT INTO native_deliveries (
                    interaction_id, target_agent_id, sender_agent_id, operation,
                    caller_idempotency_key, canonical_hash, generation, phase,
                    record_json, created_at, updated_at
                 ) VALUES (?1, 'agent-one', NULL, 'message', ?1, 'payload-hash', 1,
                           'submitted_unconfirmed', '{}', ?2, ?2)",
                params![native_intent_id, recovered_after.accepted_at],
            )?;
            Ok(())
        })
        .expect("seed native broker evidence for cleanup test");
        wardian_core::db::get_db_conn(|connection| {
            connection.execute(
                "UPDATE native_deliveries SET phase = 'queued', updated_at = ?2
                 WHERE interaction_id = ?1",
                params![native_intent_id, "2026-10-31T00:01:00Z"],
            )?;
            Ok(())
        })
        .expect("seed a pre-crossing broker phase after uncertain recovery");
        let uncertain_status = load_operation_for_key(
            &session,
            &namespace_id,
            &after_crossing_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .expect("uncertain status remains fenced from queued broker state");
        assert_eq!(uncertain_status.delivery_state, "submitted_unconfirmed");
        assert_eq!(uncertain_status.updated_at, uncertainty_fence.updated_at);
        wardian_core::db::get_db_conn(|connection| {
            connection.execute(
                "UPDATE native_deliveries SET phase = 'completed', updated_at = ?2
                 WHERE interaction_id = ?1",
                params![native_intent_id, "2026-10-31T00:02:00Z"],
            )?;
            Ok(())
        })
        .expect("advance the broker phase");
        let current_phase = load_operation_by_id_public(&native_intent_id)
            .expect("read current broker phase without changing operation");
        assert_eq!(current_phase.delivery_state, "completed");
        assert_eq!(current_phase.updated_at, "2026-10-31T00:02:00Z");
        let status_by_key = load_operation_for_key(
            &session,
            &namespace_id,
            &after_crossing_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .expect("status lookup reports current broker phase");
        assert_eq!(status_by_key.delivery_state, "completed");
        assert_eq!(status_by_key.updated_at, "2026-10-31T00:02:00Z");
        wardian_core::db::delete_agent("agent-one").expect("clean target native broker rows");
        let retained_uncertain = load_operation_by_id_public(&native_intent_id)
            .expect("outer proof remains after broker cleanup");
        assert_eq!(retained_uncertain.delivery_state, "submitted_unconfirmed");
        let retained_by_key = load_operation_for_key(
            &session,
            &namespace_id,
            &after_crossing_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .expect("status proof survives broker cleanup");
        assert_eq!(retained_by_key.delivery_state, "submitted_unconfirmed");

        let expired = load_operation_for_key(&session, &namespace_id, &request_id, expires_at_ms);
        assert!(expired
            .unwrap_err()
            .contains("idempotency_namespace_expired_or_unknown"));
        let retained: i64 = wardian_core::db::get_db_conn(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM remote_client_message_operations WHERE namespace_id = ?1 AND request_uuid = ?2",
                params![namespace_id, request_id],
                |row| row.get(0),
            )?)
        })
        .expect("expired operation proof remains persisted");
        assert_eq!(retained, 1);
    }

    #[tokio::test]
    async fn live_submit_rechecks_authority_at_write_and_releases_before_receipt() {
        let temp = tempfile::tempdir().expect("isolated authority home");
        let _home = IsolatedWardianHome::set(temp.path()).await;
        wardian_core::db::init_db().expect("initialize isolated state database");
        configure_send_authority_home(temp.path());
        let state = AppState::new();
        let authority = bound_send_authority(
            &state,
            millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 60_000),
        );
        let submit_key = crate::utils::terminal_input::provider_submit_chunks("gemini", "message")
            .expect("provider submit chunks")[1]
            .clone();
        let (submit_entered_tx, submit_entered_rx) = tokio::sync::oneshot::channel();
        let (release_submit_tx, release_submit_rx) = tokio::sync::oneshot::channel();
        let sink = Arc::new(ControlledTerminalInputSink::new(
            submit_key.clone(),
            Some(submit_entered_tx),
            Some(release_submit_rx),
        ));
        let (submitted_tx, submitted_rx) = tokio::sync::oneshot::channel();
        let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
        let transaction_sink = sink.clone();
        let transaction_authority = authority.clone();
        let transaction = tokio::spawn(async move {
            let result = crate::utils::terminal_input::submit_prompt_via_sender_with_submit_guard(
                transaction_sink.as_ref(),
                "gemini",
                "message",
                || async { Ok(()) },
                || async { Ok(()) },
                move || async move {
                    transaction_authority
                        .acquire()
                        .await
                        .map(Some)
                        .map_err(
                            |code| crate::utils::delivery_transaction::TerminalDeliveryError {
                                phase: code,
                                message: code.to_string(),
                                retry_safe: true,
                                observed_state: Some("no_submit_key_sent".to_string()),
                                reason: Some(
                                    "authority changed before the provider submit key was written"
                                        .to_string(),
                                ),
                            },
                        )
                },
            )
            .await;
            let _ = submitted_tx.send(());
            if result.is_ok() {
                let _ = receipt_rx.await;
            }
            result
        });
        submit_entered_rx
            .await
            .expect("controlled sink reached provider submit write");

        let device_store_lock = state.remote_device_store_lock.clone();
        let runtime = Arc::new(Mutex::new(
            crate::remote::models::RemoteRuntimeState::default(),
        ));
        let revoke_runtime = runtime.clone();
        let (revoke_started_tx, revoke_started_rx) = tokio::sync::oneshot::channel();
        let mut revocation = tokio::spawn(async move {
            let _ = revoke_started_tx.send(());
            crate::remote::auth::revoke_device_and_sessions(
                device_store_lock,
                &revoke_runtime,
                "client-device".to_string(),
                "2026-10-31T00:01:00Z".to_string(),
            )
            .await
        });
        revoke_started_rx.await.expect("revocation task started");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut revocation)
                .await
                .is_err()
        );
        release_submit_tx
            .send(())
            .expect("release controlled provider write");
        submitted_rx.await.expect("submit write returned");
        tokio::time::timeout(std::time::Duration::from_secs(5), revocation)
            .await
            .expect("revocation completes before provider receipt")
            .expect("revocation task")
            .expect("revocation publishes");
        assert!(
            !transaction.is_finished(),
            "provider receipt is still pending"
        );
        assert_eq!(sink.writes(), vec![b"message".to_vec(), submit_key.clone()]);
        receipt_tx.send(()).expect("release provider receipt wait");
        transaction
            .await
            .expect("live transaction task")
            .expect("provider submit succeeds");

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let revoked_result =
            crate::utils::terminal_input::submit_prompt_via_sender_with_submit_guard(
                &tx,
                "gemini",
                "revoked message",
                || async { Ok(()) },
                || async { Ok(()) },
                move || async move {
                    authority.acquire().await.map(Some).map_err(|code| {
                        crate::utils::delivery_transaction::TerminalDeliveryError {
                            phase: code,
                            message: code.to_string(),
                            retry_safe: true,
                            observed_state: Some("no_submit_key_sent".to_string()),
                            reason: Some("authority rejected before provider submit".to_string()),
                        }
                    })
                },
            )
            .await
            .expect_err("revoked authority blocks final provider submission");
        assert_eq!(revoked_result.phase, "device_revoked");
        assert_eq!(rx.recv().await, Some(b"revoked message".to_vec()));
        assert!(rx.try_recv().is_err(), "submit key was not written");

        let deadline_authority = bound_send_authority(
            &state,
            millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 10),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let expired_result =
            crate::utils::terminal_input::submit_prompt_via_sender_with_submit_guard(
                &tx,
                "gemini",
                "late message",
                || async { Ok(()) },
                || async {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    Ok(())
                },
                move || async move {
                    deadline_authority
                        .acquire()
                        .await
                        .map(Some)
                        .map_err(
                            |code| crate::utils::delivery_transaction::TerminalDeliveryError {
                                phase: code,
                                message: code.to_string(),
                                retry_safe: true,
                                observed_state: Some("no_submit_key_sent".to_string()),
                                reason: Some("deadline expired before provider submit".to_string()),
                            },
                        )
                },
            )
            .await
            .expect_err("deadline expiry during preparation blocks final provider submission");
        assert_eq!(expired_result.phase, "deadline_expired");
        assert_eq!(rx.recv().await, Some(b"late message".to_vec()));
        assert!(
            rx.try_recv().is_err(),
            "expired request wrote no submit key"
        );
    }

    #[tokio::test]
    async fn headless_spawn_rechecks_deadline_after_prep_and_releases_before_exit() {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            let temp = tempfile::tempdir().expect("isolated authority home");
            let _home = IsolatedWardianHome::set(temp.path()).await;
            wardian_core::db::init_db().expect("initialize isolated state database");
            configure_send_authority_home(temp.path());
            let state = AppState::new();

            let expired_started = temp.path().join("expired-started");
            let expired_release = temp.path().join("expired-release");
            let expired_authority = bound_send_authority(
                &state,
                millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 10),
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let mut expired_command =
                controlled_headless_command(&expired_started, &expired_release);
            let expired_result = crate::manager::headless::spawn_headless_process(
                &mut expired_command,
                Some(&expired_authority),
            )
            .await;
            let expired_error = expired_result.expect_err("expired deadline blocks spawn");
            assert_eq!(expired_error.message(), "deadline_expired");
            assert!(
                !expired_started.exists(),
                "expired request spawned no process"
            );

            let started = temp.path().join("started");
            let release = temp.path().join("release");
            let authority = bound_send_authority(
                &state,
                millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 60_000),
            );
            let mut command = controlled_headless_command(&started, &release);
            let mut child =
                crate::manager::headless::spawn_headless_process(&mut command, Some(&authority))
                    .await
                    .expect("authorized process starts");
            wait_for_file(&started).await;
            assert!(child.try_wait().expect("check blocked child").is_none());

            let device_store_lock = state.remote_device_store_lock.clone();
            let runtime = Arc::new(Mutex::new(
                crate::remote::models::RemoteRuntimeState::default(),
            ));
            crate::remote::auth::revoke_device_and_sessions(
                device_store_lock,
                &runtime,
                "client-device".to_string(),
                chrono::Utc::now().to_rfc3339(),
            )
            .await
            .expect("revocation completes while child is blocked");
            assert!(
                child
                    .try_wait()
                    .expect("check blocked child after revocation")
                    .is_none(),
                "the provider process remains blocked after authority locks are released"
            );

            std::fs::write(&release, "release").expect("release controlled process");
            let status = child.wait().await.expect("wait for controlled process");
            assert!(status.success());
        })
        .await
        .expect("headless authority boundary test completes");
    }

    #[tokio::test]
    async fn revocation_waits_for_the_provider_boundary_authority_permit() {
        let temp = tempfile::tempdir().expect("isolated authority home");
        let _home = IsolatedWardianHome::set(temp.path()).await;
        wardian_core::db::init_db().expect("initialize isolated state database");
        let config = crate::remote::models::RemoteGatewayConfig {
            schema_version: crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://gateway.example".to_string(),
            client_origin_allowlist: vec!["https://client.example".to_string()],
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 4040,
            gateway_identity_public_key: "gateway-public-key".to_string(),
            gateway_identity_fingerprint: "gateway-fingerprint".to_string(),
        };
        crate::remote::storage::save_remote_config_at(temp.path(), &config)
            .expect("save bound gateway config");
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices.push(crate::remote::models::DeviceRecord {
                device_id: "client-device".to_string(),
                label: "Client".to_string(),
                public_key_spki_der_base64: "public-key".to_string(),
                public_key_fingerprint: "key-fingerprint".to_string(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: Vec::new(),
                send_grants: vec![RemoteSendGrant::ChatSend],
                created_at: "2026-10-31T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("save send-enabled device");

        let now_ms = chrono::Utc::now().timestamp_millis();
        let (namespace_id, expires_at_ms) = get_or_create_namespace(
            "client-device".to_string(),
            "https://client.example".to_string(),
            RemoteCredentialAudience::PairedClientV1,
            now_ms,
        )
        .await
        .expect("idempotency namespace");
        let session = test_session(namespace_id.clone(), expires_at_ms);
        let (operation, _) = reserve_operation(
            &session,
            &namespace_id,
            &uuid::Uuid::new_v4().to_string(),
            "agent-one",
            "revoke before provider submission",
            now_ms,
        )
        .expect("durable admitted operation");

        let state = AppState::new();
        let authority = ClientSendAuthority::from_binding(
            &state,
            "client-device".to_string(),
            "https://client.example".to_string(),
            "https://gateway.example".to_string(),
            RemoteCredentialAudience::PairedClientV1
                .as_str()
                .to_string(),
            millis_to_rfc3339(chrono::Utc::now().timestamp_millis() + 60_000),
        );
        let permit = authority.acquire().await.expect("active send authority");
        let device_store_lock = state.remote_device_store_lock.clone();
        let runtime = Arc::new(Mutex::new(
            crate::remote::models::RemoteRuntimeState::default(),
        ));
        let revoke_runtime = runtime.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut revocation = tokio::spawn(async move {
            let _ = started_tx.send(());
            crate::remote::auth::revoke_device_and_sessions(
                device_store_lock,
                &revoke_runtime,
                "client-device".to_string(),
                "2026-10-31T00:01:00Z".to_string(),
            )
            .await
        });
        started_rx.await.expect("revocation task started");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut revocation)
                .await
                .is_err()
        );
        drop(permit);
        tokio::time::timeout(std::time::Duration::from_secs(5), revocation)
            .await
            .expect("revocation proceeds after boundary permit release")
            .expect("revocation task")
            .expect("revocation publishes");
        let sink_submissions = Arc::new(AtomicUsize::new(0));
        let sink = sink_submissions.clone();
        let revoked_operation = dispatch_if_admitted_with(
            &state,
            operation,
            Box::new(
                move |_, _| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send>> {
                    Box::pin(async move {
                        sink.fetch_add(1, Ordering::SeqCst);
                        ClientMessageDispatchOutcome::Observed("provider_accepted".to_string())
                    })
                },
            ),
        )
        .await
        .expect("revoked operation result");
        assert_eq!(revoked_operation.delivery_state, "failed_before_submit");
        assert_eq!(
            revoked_operation.error_code.as_deref(),
            Some("device_revoked")
        );
        assert_eq!(sink_submissions.load(Ordering::SeqCst), 0);
        assert_eq!(
            authority
                .acquire()
                .await
                .err()
                .expect("revoked authority error"),
            "device_revoked"
        );
    }
}

pub(crate) async fn get_or_create_namespace(
    device_id: String,
    client_origin: String,
    audience: RemoteCredentialAudience,
    now_ms: i64,
) -> Result<(String, i64), String> {
    wardian_core::db::get_db_conn(|connection| {
        Ok(with_immediate_transaction(connection, |connection| {
            if let Some(existing) = connection
                .query_row(
                    "SELECT namespace_id, expires_at_ms
                     FROM remote_client_idempotency_namespaces
                     WHERE device_id = ?1 AND client_origin = ?2 AND audience = ?3
                       AND expires_at_ms > ?4
                     ORDER BY created_at_ms DESC LIMIT 1",
                    params![device_id, client_origin, audience.as_str(), now_ms],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?
            {
                return Ok(existing);
            }
            let namespace_id = crate::remote::crypto::random_url_token(32);
            let expires_at_ms = now_ms.saturating_add(IDEMPOTENCY_NAMESPACE_TTL_MS);
            connection.execute(
                "INSERT INTO remote_client_idempotency_namespaces
                    (namespace_id, device_id, client_origin, audience, created_at_ms, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    namespace_id,
                    device_id,
                    client_origin,
                    audience.as_str(),
                    now_ms,
                    expires_at_ms
                ],
            )?;
            Ok((namespace_id, expires_at_ms))
        })?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn parse_idempotency_key(
    session: &RemoteClientSessionRecord,
    value: &str,
) -> Result<(String, String), &'static str> {
    let namespace_id = session
        .idempotency_namespace
        .as_deref()
        .filter(|_| session.send_grants.contains(&RemoteSendGrant::ChatSend))
        .ok_or("grant_required")?;
    let Some((key_namespace, request_id)) = value.split_once('.') else {
        return Err("invalid_idempotency_key");
    };
    if key_namespace.len() != 43
        || !key_namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid_idempotency_key");
    }
    let parsed = uuid::Uuid::parse_str(request_id).map_err(|_| "invalid_idempotency_key")?;
    if parsed.get_version_num() != 4 || parsed.to_string() != request_id {
        return Err("invalid_idempotency_key");
    }
    if key_namespace != namespace_id {
        return Err("idempotency_namespace_expired_or_unknown");
    }
    Ok((namespace_id.to_string(), request_id.to_string()))
}

pub(crate) fn payload_hash(target: &str, text: &str, input_mode: &str) -> String {
    let mut hasher = Sha256::new();
    for field in [target.as_bytes(), text.as_bytes(), input_mode.as_bytes()] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

pub(crate) fn find_existing_operation(
    session: &RemoteClientSessionRecord,
    namespace_id: &str,
    request_id: &str,
    now_ms: i64,
) -> Result<Option<ClientMessageOperationRecord>, String> {
    let device_id = session.device_id.clone();
    let client_origin = session.client_origin.clone();
    let audience = session.audience.as_str();
    wardian_core::db::get_db_conn(|connection| {
        validate_namespace(
            connection,
            namespace_id,
            &device_id,
            &client_origin,
            audience,
            now_ms,
        )?;
        Ok(load_operation(connection, namespace_id, request_id)?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn reserve_operation(
    session: &RemoteClientSessionRecord,
    namespace_id: &str,
    request_id: &str,
    target: &str,
    text: &str,
    now_ms: i64,
) -> Result<(ClientMessageOperationRecord, bool), String> {
    let expected_hash = payload_hash(target, text, "message");
    let operation_id = uuid::Uuid::new_v4().to_string();
    let accepted_at = millis_to_rfc3339(now_ms);
    let updated_at = accepted_at.clone();
    let deadline_at = millis_to_rfc3339(now_ms.saturating_add(MESSAGE_DEADLINE_MS));
    let device_id = session.device_id.clone();
    let client_origin = session.client_origin.clone();
    let audience = session.audience.as_str();
    wardian_core::db::get_db_conn(|connection| {
        Ok(with_immediate_transaction(connection, |connection| {
            let expires_at_ms = validate_namespace(
                connection,
                namespace_id,
                &device_id,
                &client_origin,
                audience,
                now_ms,
            )?;
            if let Some(existing) = load_operation(connection, namespace_id, request_id)? {
                if existing.session_id != target
                    || existing.text != text
                    || existing.input_mode != "message"
                    || existing.payload_hash != expected_hash
                {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "idempotency_conflict".to_string(),
                    ));
                }
                return Ok((existing, false));
            }
            connection.execute(
                "INSERT INTO remote_client_message_operations (
                    operation_id, namespace_id, request_uuid, device_id, client_origin, audience,
                    gateway_origin, target_session_id, payload_hash, text, input_mode, delivery_state,
                    accepted_at, updated_at, idempotency_expires_at_ms, error_code, native_intent_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'message', 'admitted',
                           ?11, ?12, ?13, NULL, ?1)",
                params![
                    operation_id,
                    namespace_id,
                    request_id,
                    device_id,
                    client_origin,
                    audience,
                    session.gateway_origin,
                    target,
                    expected_hash,
                    text,
                    accepted_at,
                    updated_at,
                    expires_at_ms
                ],
            )?;
            connection.execute(
                "INSERT INTO remote_client_message_outbox
                    (operation_id, state, delivery_deadline_at, updated_at_ms)
                 VALUES (?1, 'admitted', ?2, ?3)",
                params![operation_id, deadline_at, now_ms],
            )?;
            let record = load_operation_by_id(connection, &operation_id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            Ok((record, true))
        })?)
    })
    .map_err(|error| {
        if error.to_string().contains("idempotency_conflict") {
            "idempotency_conflict".to_string()
        } else {
            error.to_string()
        }
    })
}

pub(crate) fn claim_dispatch(operation_id: &str, now_ms: i64) -> Result<bool, String> {
    let operation_id = operation_id.to_string();
    wardian_core::db::get_db_conn(|connection| {
        Ok(with_immediate_transaction(connection, |connection| {
            let changed = connection.execute(
                "UPDATE remote_client_message_operations
                 SET delivery_state = 'dispatching', updated_at = ?2
                 WHERE operation_id = ?1 AND delivery_state = 'admitted'",
                params![operation_id, millis_to_rfc3339(now_ms)],
            )?;
            if changed == 1 {
                connection.execute(
                    "UPDATE remote_client_message_outbox
                     SET state = 'dispatching', updated_at_ms = ?2 WHERE operation_id = ?1",
                    params![operation_id, now_ms],
                )?;
            }
            Ok(changed == 1)
        })?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn update_operation(
    operation_id: &str,
    state: &str,
    error_code: Option<&str>,
    now_ms: i64,
) -> Result<ClientMessageOperationRecord, String> {
    let operation_id = operation_id.to_string();
    let state = state.to_string();
    let error_code = error_code.map(str::to_string);
    wardian_core::db::get_db_conn(|connection| {
        Ok(with_immediate_transaction(connection, |connection| {
            connection.execute(
                "UPDATE remote_client_message_operations
                 SET delivery_state = ?2, error_code = ?3, updated_at = ?4
                 WHERE operation_id = ?1",
                params![operation_id, state, error_code, millis_to_rfc3339(now_ms)],
            )?;
            connection.execute(
                "UPDATE remote_client_message_outbox
                 SET state = ?2, updated_at_ms = ?3 WHERE operation_id = ?1",
                params![operation_id, state, now_ms],
            )?;
            load_operation_by_id(connection, &operation_id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)
        })?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn load_operation_for_key(
    session: &RemoteClientSessionRecord,
    namespace_id: &str,
    request_id: &str,
    now_ms: i64,
) -> Result<ClientMessageOperationRecord, String> {
    find_existing_operation(session, namespace_id, request_id, now_ms)?
        .ok_or_else(|| "operation_not_found".to_string())
}

pub(crate) fn recover_interrupted_operations() -> Result<usize, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    wardian_core::db::get_db_conn(|connection| {
        Ok(with_immediate_transaction(connection, |connection| {
            let changed = connection.execute(
                "UPDATE remote_client_message_operations
                 SET delivery_state = 'submitted_unconfirmed',
                     error_code = NULL, updated_at = ?1
                 WHERE delivery_state = 'dispatching'",
                params![millis_to_rfc3339(now_ms)],
            )?;
            connection.execute(
                "UPDATE remote_client_message_outbox
                 SET state = 'submitted_unconfirmed', updated_at_ms = ?1
                 WHERE state = 'dispatching'",
                params![now_ms],
            )?;
            Ok(changed)
        })?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn admitted_operations_for_recovery() -> Result<Vec<ClientMessageOperationRecord>, String>
{
    wardian_core::db::get_db_conn(|connection| {
        let mut statement = connection.prepare(
            "SELECT operation_id FROM remote_client_message_operations
             WHERE delivery_state = 'admitted' ORDER BY accepted_at, operation_id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids
            .into_iter()
            .map(|operation_id| {
                load_operation_by_id(connection, &operation_id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)
            })
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .map_err(|error| error.to_string())
}

pub(crate) async fn dispatch_if_admitted(
    state: &AppState,
    operation: ClientMessageOperationRecord,
) -> Result<ClientMessageOperationRecord, String> {
    dispatch_if_admitted_with(
        state,
        operation,
        Box::new(move |authority: ClientSendAuthority, operation: ClientMessageOperationRecord| -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send + '_>> {
            Box::pin(async move {
                let result = crate::control::deliver_remote_client_message(
                    state,
                    &operation.session_id,
                    &operation.text,
                    &operation.native_intent_id,
                    &operation.delivery_deadline_at,
                    authority,
                )
                .await;
                match result {
                    Ok(details) => {
                        if details.iter().any(|detail| {
                            detail.delivery_state == "submitted_unconfirmed"
                                || detail.delivery_phase.as_deref() == Some("submitted_unconfirmed")
                        }) {
                            ClientMessageDispatchOutcome::SubmittedUnconfirmed
                        } else {
                            let first_detail = details.first();
                            let authority_failure = first_detail
                                .and_then(|detail| detail.error.as_ref())
                                .and_then(|error| {
                                    authority_failure_code_in_message(&error.code)
                                });
                            if let Some(code) = authority_failure {
                                ClientMessageDispatchOutcome::FailedBeforeSubmit(Some(
                                    code.to_string(),
                                ))
                            } else {
                                ClientMessageDispatchOutcome::Observed(
                                    first_detail.map_or_else(
                                        || "provider_accepted".to_string(),
                                        |detail| {
                                            detail
                                                .delivery_phase
                                                .clone()
                                                .unwrap_or_else(|| detail.delivery_state.clone())
                                        },
                                    ),
                                )
                            }
                        }
                    }
                    Err(error) => {
                        classify_dispatch_error(error.details())
                    }
                }
            })
        }),
    )
    .await
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClientMessageDispatchOutcome {
    Observed(String),
    SubmittedUnconfirmed,
    FailedBeforeSubmit(Option<String>),
}

fn classify_dispatch_error(details: Option<&serde_json::Value>) -> ClientMessageDispatchOutcome {
    let Some(details) = details else {
        return ClientMessageDispatchOutcome::SubmittedUnconfirmed;
    };
    let items = details
        .get("delivery")
        .and_then(serde_json::Value::as_array);
    let uncertain = details
        .get("retry_safe")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || items.is_none_or(|items| {
            items.iter().any(|item| {
                item.get("delivery_state")
                    .and_then(serde_json::Value::as_str)
                    == Some("submitted_unconfirmed")
                    || item
                        .get("delivery_phase")
                        .and_then(serde_json::Value::as_str)
                        == Some("submitted_unconfirmed")
            })
        });
    if uncertain {
        ClientMessageDispatchOutcome::SubmittedUnconfirmed
    } else {
        ClientMessageDispatchOutcome::FailedBeforeSubmit(None)
    }
}

async fn dispatch_if_admitted_with<'a>(
    state: &'a AppState,
    operation: ClientMessageOperationRecord,
    dispatch: impl FnOnce(
            ClientSendAuthority,
            ClientMessageOperationRecord,
        ) -> Pin<Box<dyn Future<Output = ClientMessageDispatchOutcome> + Send + 'a>>
        + Send
        + 'a,
) -> Result<ClientMessageOperationRecord, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    if !claim_dispatch(&operation.operation_id, now_ms)? {
        return load_operation_by_id_public(&operation.operation_id);
    }
    if !valid_message_text(&operation.text) {
        return update_operation(
            &operation.operation_id,
            "failed_before_submit",
            Some("invalid_request"),
            now_ms,
        );
    }
    let authority = ClientSendAuthority::from_binding(
        state,
        operation.device_id.clone(),
        operation.client_origin.clone(),
        operation.gateway_origin.clone(),
        operation.audience.clone(),
        operation.delivery_deadline_at.clone(),
    );
    let authority_check = match authority.acquire().await {
        Ok(permit) => permit,
        Err(code) => {
            return update_operation(
                &operation.operation_id,
                "failed_before_submit",
                Some(code),
                chrono::Utc::now().timestamp_millis(),
            );
        }
    };
    drop(authority_check);
    let outcome = dispatch(authority, operation.clone()).await;
    let (delivery_state, error_code) = match outcome {
        ClientMessageDispatchOutcome::Observed(state) => (state, None),
        ClientMessageDispatchOutcome::SubmittedUnconfirmed => {
            ("submitted_unconfirmed".to_string(), None)
        }
        ClientMessageDispatchOutcome::FailedBeforeSubmit(error_code) => (
            "failed_before_submit".to_string(),
            Some(error_code.unwrap_or_else(|| "delivery_failed".to_string())),
        ),
    };
    update_operation(
        &operation.operation_id,
        &delivery_state,
        error_code.as_deref(),
        chrono::Utc::now().timestamp_millis(),
    )
}

fn load_operation_by_id_public(operation_id: &str) -> Result<ClientMessageOperationRecord, String> {
    wardian_core::db::get_db_conn(|connection| {
        Ok(load_operation_by_id(connection, operation_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?)
    })
    .map_err(|_| "service_unavailable".to_string())
}

fn validate_namespace(
    connection: &Connection,
    namespace_id: &str,
    device_id: &str,
    client_origin: &str,
    audience: &str,
    now_ms: i64,
) -> rusqlite::Result<i64> {
    let expires_at_ms = connection
        .query_row(
            "SELECT expires_at_ms FROM remote_client_idempotency_namespaces
             WHERE namespace_id = ?1 AND device_id = ?2 AND client_origin = ?3 AND audience = ?4",
            params![namespace_id, device_id, client_origin, audience],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .filter(|expires_at_ms| now_ms < *expires_at_ms)
        .ok_or(rusqlite::Error::InvalidParameterName(
            "idempotency_namespace_expired_or_unknown".to_string(),
        ))?;
    Ok(expires_at_ms)
}

fn load_operation(
    connection: &Connection,
    namespace_id: &str,
    request_id: &str,
) -> rusqlite::Result<Option<ClientMessageOperationRecord>> {
    connection
        .query_row(
            "SELECT remote_client_message_operations.operation_id, request_uuid, device_id, client_origin, audience,
                    target_session_id, text, input_mode,
                    CASE
                        WHEN remote_client_message_operations.delivery_state = 'submitted_unconfirmed'
                         AND (SELECT phase FROM native_deliveries
                              WHERE interaction_id = remote_client_message_operations.native_intent_id)
                             IN ('queued', 'dispatching')
                        THEN 'submitted_unconfirmed'
                        ELSE COALESCE(
                            (SELECT phase FROM native_deliveries
                             WHERE interaction_id = remote_client_message_operations.native_intent_id),
                            remote_client_message_operations.delivery_state
                        )
                    END, accepted_at,
                    CASE
                        WHEN remote_client_message_operations.delivery_state = 'submitted_unconfirmed'
                         AND (SELECT phase FROM native_deliveries
                              WHERE interaction_id = remote_client_message_operations.native_intent_id)
                             IN ('queued', 'dispatching')
                        THEN remote_client_message_operations.updated_at
                        ELSE COALESCE(
                            (SELECT updated_at FROM native_deliveries
                             WHERE interaction_id = remote_client_message_operations.native_intent_id),
                            remote_client_message_operations.updated_at
                        )
                    END, idempotency_expires_at_ms, error_code, native_intent_id,
                    gateway_origin, payload_hash, o.delivery_deadline_at
             FROM remote_client_message_operations
             JOIN remote_client_message_outbox o
               ON o.operation_id = remote_client_message_operations.operation_id
             WHERE namespace_id = ?1 AND request_uuid = ?2",
            params![namespace_id, request_id],
            operation_from_row,
        )
        .optional()
}

fn load_operation_by_id(
    connection: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<ClientMessageOperationRecord>> {
    connection
        .query_row(
            "SELECT remote_client_message_operations.operation_id, request_uuid, device_id, client_origin, audience,
                    target_session_id, text, input_mode,
                    CASE
                        WHEN remote_client_message_operations.delivery_state = 'submitted_unconfirmed'
                         AND (SELECT phase FROM native_deliveries
                              WHERE interaction_id = remote_client_message_operations.native_intent_id)
                             IN ('queued', 'dispatching')
                        THEN 'submitted_unconfirmed'
                        ELSE COALESCE(
                            (SELECT phase FROM native_deliveries
                             WHERE interaction_id = remote_client_message_operations.native_intent_id),
                            remote_client_message_operations.delivery_state
                        )
                    END, accepted_at,
                    CASE
                        WHEN remote_client_message_operations.delivery_state = 'submitted_unconfirmed'
                         AND (SELECT phase FROM native_deliveries
                              WHERE interaction_id = remote_client_message_operations.native_intent_id)
                             IN ('queued', 'dispatching')
                        THEN remote_client_message_operations.updated_at
                        ELSE COALESCE(
                            (SELECT updated_at FROM native_deliveries
                             WHERE interaction_id = remote_client_message_operations.native_intent_id),
                            remote_client_message_operations.updated_at
                        )
                    END, idempotency_expires_at_ms, error_code, native_intent_id,
                    gateway_origin, payload_hash, o.delivery_deadline_at
             FROM remote_client_message_operations
             JOIN remote_client_message_outbox o
               ON o.operation_id = remote_client_message_operations.operation_id
             WHERE remote_client_message_operations.operation_id = ?1",
            params![operation_id],
            operation_from_row,
        )
        .optional()
}

fn operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClientMessageOperationRecord> {
    Ok(ClientMessageOperationRecord {
        operation_id: row.get(0)?,
        request_id: row.get(1)?,
        device_id: row.get(2)?,
        client_origin: row.get(3)?,
        audience: row.get(4)?,
        session_id: row.get(5)?,
        text: row.get(6)?,
        input_mode: row.get(7)?,
        delivery_state: row.get(8)?,
        accepted_at: row.get(9)?,
        updated_at: row.get(10)?,
        idempotency_expires_at_ms: row.get(11)?,
        error_code: row.get(12)?,
        native_intent_id: row.get(13)?,
        gateway_origin: row.get(14)?,
        payload_hash: row.get(15)?,
        delivery_deadline_at: row.get(16)?,
    })
}

fn with_immediate_transaction<T>(
    connection: &Connection,
    action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    connection.execute_batch("BEGIN IMMEDIATE")?;
    match action(connection) {
        Ok(value) => {
            connection.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn millis_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl ClientSendAuthority {
    fn from_binding(
        state: &AppState,
        device_id: String,
        client_origin: String,
        gateway_origin: String,
        audience: String,
        deadline_at: String,
    ) -> Self {
        Self {
            config_lock: state.remote_gateway_config_lock.clone(),
            device_store_lock: state.remote_device_store_lock.clone(),
            device_id,
            client_origin,
            gateway_origin,
            audience,
            deadline_at,
        }
    }
}
