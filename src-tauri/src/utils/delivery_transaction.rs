use crate::utils::delivery_profile::DeliveryProfile;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::mpsc::Sender;

pub trait TerminalInputSink: Send + Sync {
    fn send_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

#[cfg(test)]
impl TerminalInputSink for Sender<Vec<u8>> {
    fn send_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            self.send(bytes)
                .await
                .map_err(|error| format!("input channel closed: {error}"))
        })
    }
}

pub struct BrokerTerminalInputSink {
    broker: Arc<crate::state::terminal_session::TerminalSessionBroker>,
    session_id: String,
}

impl BrokerTerminalInputSink {
    pub fn new(
        broker: Arc<crate::state::terminal_session::TerminalSessionBroker>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            broker,
            session_id: session_id.into(),
        }
    }
}

impl TerminalInputSink for BrokerTerminalInputSink {
    fn send_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            self.broker
                .send_privileged_input(&self.session_id, bytes)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

pub const DELIVERY_STATE_SUBMIT_SENT_UNCONFIRMED: &str = "submit_sent_unconfirmed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Literal,
    BracketedPaste,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPayloadPlan {
    pub payload_kind: PayloadKind,
    pub payload_bytes: Vec<u8>,
    pub submit_key: Vec<u8>,
    pub submit_delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalDeliveryOutcome {
    pub delivery_state: String,
    pub delivery_phase: String,
    pub observed_state: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalDeliveryError {
    pub phase: &'static str,
    pub message: String,
    pub retry_safe: bool,
    pub observed_state: Option<String>,
    pub reason: Option<String>,
}

impl TerminalDeliveryError {
    pub(crate) fn terminal_state_unknown(phase: &'static str, message: String) -> Self {
        Self {
            phase,
            message,
            retry_safe: false,
            observed_state: None,
            reason: None,
        }
    }

    pub(crate) fn with_observation(
        mut self,
        observed_state: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        self.observed_state = Some(observed_state.into());
        self.reason = Some(reason.into());
        self
    }
}

impl fmt::Display for TerminalDeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TerminalDeliveryError {}

pub fn bracketed_paste_bytes(prompt: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(prompt.len() + b"\x1b[200~\x1b[201~".len());
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(prompt.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

pub fn plan_terminal_payload(profile: &DeliveryProfile, prompt: &str) -> TerminalPayloadPlan {
    let use_bracketed_paste = !prompt.is_empty()
        && profile.bracketed_paste.enabled
        && (prompt.contains('\n') || prompt.len() >= profile.bracketed_paste.min_bytes);
    let payload_kind = if use_bracketed_paste {
        PayloadKind::BracketedPaste
    } else {
        PayloadKind::Literal
    };
    let payload_bytes = if use_bracketed_paste {
        bracketed_paste_bytes(prompt)
    } else {
        prompt.as_bytes().to_vec()
    };

    TerminalPayloadPlan {
        payload_kind,
        payload_bytes,
        submit_key: profile.submit_key.bytes().to_vec(),
        submit_delay_ms: profile.submit_delay_ms,
    }
}

pub async fn submit_terminal_transaction<S: TerminalInputSink + ?Sized>(
    tx: &S,
    profile: &DeliveryProfile,
    prompt: &str,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError> {
    submit_terminal_transaction_with_payload_hook(tx, profile, prompt, || async { Ok(()) }).await
}

pub async fn submit_terminal_transaction_with_payload_hook<S, F, Fut>(
    tx: &S,
    profile: &DeliveryProfile,
    prompt: &str,
    on_payload_sent: F,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError>
where
    S: TerminalInputSink + ?Sized,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), TerminalDeliveryError>>,
{
    submit_terminal_transaction_with_hooks(tx, profile, prompt, on_payload_sent, || async {
        Ok(())
    })
    .await
}

/// Submit a terminal prompt while exposing ordering boundaries around its
/// payload and submit key. The second hook runs after any provider-specific
/// submit delay, immediately before the key which starts the provider turn.
pub async fn submit_terminal_transaction_with_hooks<S, F, Fut, G, Gfut>(
    tx: &S,
    profile: &DeliveryProfile,
    prompt: &str,
    on_payload_sent: F,
    on_before_submit: G,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError>
where
    S: TerminalInputSink + ?Sized,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), TerminalDeliveryError>>,
    G: FnOnce() -> Gfut,
    Gfut: Future<Output = Result<(), TerminalDeliveryError>>,
{
    let plan = plan_terminal_payload(profile, prompt);
    if plan.payload_bytes.is_empty() {
        return Ok(TerminalDeliveryOutcome {
            delivery_state: "empty".to_string(),
            delivery_phase: "empty".to_string(),
            observed_state: None,
            reason: Some("prompt normalized to empty".to_string()),
        });
    }

    tx.send_bytes(plan.payload_bytes).await.map_err(|e| {
        // Once a broker accepts an input request, a timeout or native writer
        // error cannot prove that zero payload bytes reached the compositor.
        // Treat that state as ambiguous so mailbox recovery never injects the
        // prompt a second time.
        TerminalDeliveryError::terminal_state_unknown(
            "payload_send_failed",
            format!("Failed to send prompt payload: {e}"),
        )
    })?;
    on_payload_sent().await?;

    tokio::time::sleep(std::time::Duration::from_millis(plan.submit_delay_ms)).await;
    on_before_submit().await?;

    tx.send_bytes(plan.submit_key).await.map_err(|e| {
        TerminalDeliveryError::terminal_state_unknown(
            "payload_sent_submit_failed",
            format!("Failed to send prompt submit key after payload send: {e}"),
        )
    })?;

    Ok(TerminalDeliveryOutcome {
        delivery_state: DELIVERY_STATE_SUBMIT_SENT_UNCONFIRMED.to_string(),
        delivery_phase: "submit_key_sent".to_string(),
        observed_state: Some("bytes_sent".to_string()),
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::delivery_profile::{delivery_profile, DeliveryProfile};

    fn zero_delay_profile(provider: &str) -> DeliveryProfile {
        let mut profile = delivery_profile(provider);
        profile.submit_delay_ms = 0;
        profile
    }

    #[test]
    fn bracketed_paste_wraps_payload() {
        let bytes = bracketed_paste_bytes("alpha\nbeta");

        assert_eq!(bytes, b"\x1b[200~alpha\nbeta\x1b[201~".to_vec());
    }

    #[test]
    fn plan_uses_bracketed_paste_for_short_codex_input() {
        let profile = delivery_profile("codex");
        let plan = plan_terminal_payload(&profile, "hello");

        assert_eq!(plan.payload_kind, PayloadKind::BracketedPaste);
        assert_eq!(plan.payload_bytes, b"\x1b[200~hello\x1b[201~".to_vec());
        assert_eq!(plan.submit_key, b"\r".to_vec());
        assert_eq!(plan.submit_delay_ms, profile.submit_delay_ms);
    }

    #[test]
    fn plan_keeps_empty_prompt_empty_when_bracketed_paste_threshold_is_zero() {
        let mut profile = delivery_profile("codex");
        profile.bracketed_paste.min_bytes = 0;
        let plan = plan_terminal_payload(&profile, "");

        assert_eq!(plan.payload_kind, PayloadKind::Literal);
        assert!(plan.payload_bytes.is_empty());
    }

    #[test]
    fn plan_uses_bracketed_paste_for_multiline_when_enabled() {
        let profile = delivery_profile("codex");
        let plan = plan_terminal_payload(&profile, "hello\nworld");

        assert_eq!(plan.payload_kind, PayloadKind::BracketedPaste);
        assert_eq!(
            plan.payload_bytes,
            b"\x1b[200~hello\nworld\x1b[201~".to_vec()
        );
    }

    #[test]
    fn plan_uses_bracketed_paste_for_large_payload_when_enabled() {
        let profile = delivery_profile("codex");
        let prompt = "x".repeat(profile.bracketed_paste.min_bytes);
        let plan = plan_terminal_payload(&profile, &prompt);

        assert_eq!(plan.payload_kind, PayloadKind::BracketedPaste);
    }

    #[test]
    fn plan_uses_literal_when_provider_disables_bracketed_paste() {
        // Uses a provider Wardian does not support, so this keeps covering the
        // conservative fallback profile rather than any one provider's current
        // capability. Antigravity previously stood in here; it is now known to
        // support bracketed paste, so it can no longer prove this branch.
        let profile = delivery_profile("unsupported-provider");
        assert!(!profile.bracketed_paste.enabled);

        let plan = plan_terminal_payload(&profile, "hello\nworld");

        assert_eq!(plan.payload_kind, PayloadKind::Literal);
        assert_eq!(plan.payload_bytes, b"hello\nworld".to_vec());
    }

    /// Antigravity 1.1.27 retained a 6886-byte, 285-line prompt in its editor
    /// and never produced a turn, because Wardian sent it literally and the
    /// editor treated the embedded newlines as submits. A native protocol
    /// experiment against the same build showed raw
    /// `ESC[200~ payload ESC[201~` collapsing into one editor paste entry,
    /// followed by a single Return producing a provider-native answer that
    /// contained all three independent random labels from the payload's
    /// beginning, middle and end.
    #[test]
    fn antigravity_pastes_a_long_multiline_prompt_with_exact_delimiters_and_one_submit() {
        let profile = delivery_profile("antigravity");
        let prompt = (0..285)
            .map(|line| format!("line {line} of the retained prompt"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt.contains('\n'));
        assert!(prompt.len() > 2048);

        let plan = plan_terminal_payload(&profile, &prompt);

        assert_eq!(plan.payload_kind, PayloadKind::BracketedPaste);
        // Exact delimiters, payload byte-for-byte between them, and nothing else.
        assert_eq!(&plan.payload_bytes[..6], b"\x1b[200~");
        assert_eq!(
            &plan.payload_bytes[plan.payload_bytes.len() - 6..],
            b"\x1b[201~"
        );
        assert_eq!(
            &plan.payload_bytes[6..plan.payload_bytes.len() - 6],
            prompt.as_bytes()
        );
        assert_eq!(plan.payload_bytes.len(), prompt.len() + 12);
        // One Return, sent separately from the payload, never inside it.
        assert_eq!(plan.submit_key, b"\r".to_vec());
        assert!(!plan.payload_bytes.ends_with(b"\r"));
        assert_eq!(plan.submit_delay_ms, 500);
    }

    #[test]
    fn antigravity_keeps_simple_input_for_a_short_single_line_prompt() {
        let profile = delivery_profile("antigravity");

        let plan = plan_terminal_payload(&profile, "hello");

        assert_eq!(plan.payload_kind, PayloadKind::Literal);
        assert_eq!(plan.payload_bytes, b"hello".to_vec());
    }

    #[test]
    fn antigravity_pastes_a_short_multiline_prompt() {
        let profile = delivery_profile("antigravity");

        let plan = plan_terminal_payload(&profile, "alpha\nbeta");

        assert_eq!(
            plan.payload_kind,
            PayloadKind::BracketedPaste,
            "an embedded newline must not reach the editor as a literal submit"
        );
        assert_eq!(plan.payload_bytes, bracketed_paste_bytes("alpha\nbeta"));
    }

    /// A prompt that merely ends with a newline is not multiline once
    /// `normalize_prompt_for_terminal_submit` trims it, so it stays on the
    /// simple literal path. Asserted through the real entry point, because
    /// `plan_terminal_payload` alone would see the untrimmed text and wrongly
    /// suggest such prompts get pasted.
    #[test]
    fn antigravity_trailing_newline_prompt_normalizes_to_simple_input_and_one_return() {
        let chunks = crate::utils::terminal_input::provider_submit_chunks("antigravity", "alpha\n")
            .expect("submit chunks");

        assert_eq!(chunks, vec![b"alpha".to_vec(), b"\r".to_vec()]);
    }

    /// The real entry point must emit exactly two writes for a long multiline
    /// prompt: the bracketed payload, then one carriage return.
    #[test]
    fn antigravity_long_multiline_prompt_sends_one_paste_then_one_return() {
        let prompt = (0..285)
            .map(|line| format!("line {line} of the retained prompt"))
            .collect::<Vec<_>>()
            .join("\n");

        let chunks = crate::utils::terminal_input::provider_submit_chunks("antigravity", &prompt)
            .expect("submit chunks");

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], bracketed_paste_bytes(&prompt));
        assert_eq!(chunks[1], b"\r".to_vec());
    }

    #[tokio::test]
    async fn submit_transaction_sends_payload_waits_and_then_submit_key() {
        let profile = zero_delay_profile("opencode");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);

        let outcome = submit_terminal_transaction(&tx, &profile, "hello")
            .await
            .expect("submit");

        assert_eq!(rx.recv().await.expect("payload"), b"hello".to_vec());
        assert_eq!(rx.recv().await.expect("submit key"), b"\x1b[13u".to_vec());
        assert_eq!(
            outcome.delivery_state,
            DELIVERY_STATE_SUBMIT_SENT_UNCONFIRMED
        );
        assert_eq!(outcome.delivery_phase, "submit_key_sent");
        assert_eq!(outcome.observed_state.as_deref(), Some("bytes_sent"));
        assert_eq!(outcome.reason, None);
    }

    #[tokio::test]
    async fn submit_transaction_runs_before_submit_hook_before_the_submit_key() {
        let profile = zero_delay_profile("gemini");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = RecordingSink {
            events: Arc::clone(&events),
        };
        let payload_events = Arc::clone(&events);
        let before_submit_events = Arc::clone(&events);

        submit_terminal_transaction_with_hooks(
            &sink,
            &profile,
            "hello",
            move || async move {
                payload_events
                    .lock()
                    .expect("events lock")
                    .push("payload_hook");
                Ok(())
            },
            move || async move {
                before_submit_events
                    .lock()
                    .expect("events lock")
                    .push("before_submit_hook");
                Ok(())
            },
        )
        .await
        .expect("submit");

        assert_eq!(
            events.lock().expect("events lock").as_slice(),
            [
                "payload",
                "payload_hook",
                "before_submit_hook",
                "submit_key"
            ]
        );
    }

    #[tokio::test]
    async fn submit_transaction_does_not_press_submit_after_payload_receipt_persistence_fails() {
        let profile = zero_delay_profile("gemini");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = RecordingSink {
            events: Arc::clone(&events),
        };
        let payload_events = Arc::clone(&events);

        let error = submit_terminal_transaction_with_payload_hook(
            &sink,
            &profile,
            "hello",
            move || async move {
                payload_events
                    .lock()
                    .expect("events lock")
                    .push("payload_hook_failed");
                Err(TerminalDeliveryError::terminal_state_unknown(
                    "payload_receipt_persist_failed",
                    "delivery state could not be persisted".to_string(),
                ))
            },
        )
        .await
        .expect_err("persistence failure must stop before submit key");

        assert_eq!(error.phase, "payload_receipt_persist_failed");
        assert!(!error.retry_safe);
        assert_eq!(
            events.lock().expect("events lock").as_slice(),
            ["payload", "payload_hook_failed"]
        );
    }

    #[tokio::test]
    async fn submit_transaction_does_not_press_submit_when_provider_did_not_apply_payload() {
        let profile = zero_delay_profile("codex");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = RecordingSink {
            events: Arc::clone(&events),
        };
        let before_submit_events = Arc::clone(&events);

        let error = submit_terminal_transaction_with_hooks(
            &sink,
            &profile,
            "hello",
            || async { Ok(()) },
            move || async move {
                before_submit_events
                    .lock()
                    .expect("events lock")
                    .push("payload_apply_unconfirmed");
                Err(TerminalDeliveryError::terminal_state_unknown(
                    "payload_apply_unconfirmed",
                    "Codex did not apply payload; Return was not sent".to_string(),
                ))
            },
        )
        .await
        .expect_err("missing provider evidence must stop before submit key");

        assert_eq!(error.phase, "payload_apply_unconfirmed");
        assert!(!error.retry_safe);
        assert_eq!(
            events.lock().expect("events lock").as_slice(),
            ["payload", "payload_apply_unconfirmed"]
        );
    }

    #[tokio::test]
    async fn submit_transaction_treats_empty_prompt_as_non_error() {
        let profile = zero_delay_profile("codex");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);

        let outcome = submit_terminal_transaction(&tx, &profile, "")
            .await
            .expect("submit");

        assert!(rx.try_recv().is_err());
        assert_eq!(outcome.delivery_state, "empty");
        assert_eq!(outcome.delivery_phase, "empty");
        assert_eq!(outcome.observed_state, None);
        assert_eq!(
            outcome.reason.as_deref(),
            Some("prompt normalized to empty")
        );
    }

    #[tokio::test]
    async fn submit_transaction_marks_submit_key_failure_as_unsafe_to_retry() {
        let profile = zero_delay_profile("antigravity");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);

        let submit =
            tokio::spawn(async move { submit_terminal_transaction(&tx, &profile, "hello").await });
        assert_eq!(rx.recv().await.expect("payload"), b"hello".to_vec());
        drop(rx);

        let error = submit
            .await
            .expect("task")
            .expect_err("submit key send should fail");
        assert_eq!(error.phase, "payload_sent_submit_failed");
        assert!(!error.retry_safe);
        assert!(error
            .message
            .contains("Failed to send prompt submit key after payload send"));
    }

    struct RecordingSink {
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl TerminalInputSink for RecordingSink {
        fn send_bytes(
            &self,
            bytes: Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async move {
                let event = if bytes == b"\r" {
                    "submit_key"
                } else {
                    "payload"
                };
                self.events.lock().expect("events lock").push(event);
                Ok(())
            })
        }
    }
}
