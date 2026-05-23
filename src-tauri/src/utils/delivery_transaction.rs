use crate::utils::delivery_profile::DeliveryProfile;
use std::fmt;
use std::future::Future;
use tokio::sync::mpsc::Sender;

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
}

impl TerminalDeliveryError {
    fn retry_safe(phase: &'static str, message: String) -> Self {
        Self {
            phase,
            message,
            retry_safe: true,
        }
    }

    fn terminal_state_unknown(phase: &'static str, message: String) -> Self {
        Self {
            phase,
            message,
            retry_safe: false,
        }
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
    let use_bracketed_paste = profile.bracketed_paste.enabled
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

pub async fn submit_terminal_transaction(
    tx: &Sender<Vec<u8>>,
    profile: &DeliveryProfile,
    prompt: &str,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError> {
    submit_terminal_transaction_with_payload_hook(tx, profile, prompt, || async {}).await
}

pub async fn submit_terminal_transaction_with_payload_hook<F, Fut>(
    tx: &Sender<Vec<u8>>,
    profile: &DeliveryProfile,
    prompt: &str,
    on_payload_sent: F,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
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

    tx.send(plan.payload_bytes).await.map_err(|e| {
        TerminalDeliveryError::retry_safe(
            "payload_send_failed",
            format!("Failed to send prompt payload: {e}"),
        )
    })?;
    on_payload_sent().await;

    tokio::time::sleep(std::time::Duration::from_millis(plan.submit_delay_ms)).await;

    tx.send(plan.submit_key).await.map_err(|e| {
        TerminalDeliveryError::terminal_state_unknown(
            "payload_sent_submit_failed",
            format!("Failed to send prompt submit key after payload send: {e}"),
        )
    })?;

    Ok(TerminalDeliveryOutcome {
        delivery_state: "submit_sent_unverified".to_string(),
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
    fn plan_uses_literal_for_short_single_line() {
        let profile = delivery_profile("codex");
        let plan = plan_terminal_payload(&profile, "hello");

        assert_eq!(plan.payload_kind, PayloadKind::Literal);
        assert_eq!(plan.payload_bytes, b"hello".to_vec());
        assert_eq!(plan.submit_key, b"\r".to_vec());
        assert_eq!(plan.submit_delay_ms, profile.submit_delay_ms);
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
        let profile = delivery_profile("antigravity");
        let plan = plan_terminal_payload(&profile, "hello\nworld");

        assert_eq!(plan.payload_kind, PayloadKind::Literal);
        assert_eq!(plan.payload_bytes, b"hello\nworld".to_vec());
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
        assert_eq!(outcome.delivery_state, "submit_sent_unverified");
        assert_eq!(outcome.delivery_phase, "submit_key_sent");
        assert_eq!(outcome.observed_state.as_deref(), Some("bytes_sent"));
        assert_eq!(outcome.reason, None);
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
        let profile = zero_delay_profile("codex");
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
}
