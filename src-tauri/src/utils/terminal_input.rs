use crate::utils::delivery_transaction::TerminalInputSink;
use std::future::Future;

pub fn normalize_prompt_for_terminal_submit(prompt: &str) -> String {
    prompt
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// Payload and submit-key bytes a provider receives for `prompt`, so tests can
/// assert the wire format without a terminal.
#[cfg(test)]
pub fn provider_submit_chunks(provider_name: &str, prompt: &str) -> Result<Vec<Vec<u8>>, String> {
    let normalized = normalize_prompt_for_terminal_submit(prompt);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let profile = crate::utils::delivery_profile::delivery_profile(provider_name);
    let plan = crate::utils::delivery_transaction::plan_terminal_payload(&profile, &normalized);
    Ok(vec![plan.payload_bytes, plan.submit_key])
}

/// Submits `prompt` through the provider's delivery profile.
///
/// `on_payload_sent` runs after the payload bytes are written and
/// `on_before_submit` immediately before the submit key.
pub async fn submit_prompt_via_sender<S, F, Fut, G, Gfut>(
    tx: &S,
    provider_name: &str,
    prompt: &str,
    on_payload_sent: F,
    on_before_submit: G,
) -> Result<
    crate::utils::delivery_transaction::TerminalDeliveryOutcome,
    crate::utils::delivery_transaction::TerminalDeliveryError,
>
where
    S: TerminalInputSink + ?Sized,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), crate::utils::delivery_transaction::TerminalDeliveryError>>,
    G: FnOnce() -> Gfut,
    Gfut: Future<Output = Result<(), crate::utils::delivery_transaction::TerminalDeliveryError>>,
{
    let normalized = normalize_prompt_for_terminal_submit(prompt);
    let profile = crate::utils::delivery_profile::delivery_profile(provider_name);
    if normalized.is_empty() {
        return crate::utils::delivery_transaction::submit_terminal_transaction(
            tx,
            &profile,
            &normalized,
        )
        .await;
    }

    crate::utils::delivery_transaction::submit_terminal_transaction_with_hooks(
        tx,
        &profile,
        &normalized,
        on_payload_sent,
        on_before_submit,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_prompt_for_terminal_submit, provider_submit_chunks, submit_prompt_via_sender,
    };

    #[test]
    fn normalize_prompt_preserves_newlines_and_trims() {
        assert_eq!(
            normalize_prompt_for_terminal_submit("  alpha\nbeta\r\ngamma  "),
            "alpha\nbeta\ngamma"
        );
    }

    #[tokio::test]
    async fn submit_prompt_sends_text_then_submit_key() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

        submit_prompt_via_sender(
            &tx,
            "gemini",
            "hello\nworld",
            || async { Ok(()) },
            || async { Ok(()) },
        )
        .await
        .expect("submit prompt");

        let first = rx.recv().await.expect("first payload");
        let second = rx.recv().await.expect("second payload");

        assert_eq!(String::from_utf8(first).expect("utf8"), "hello\nworld");
        assert_eq!(second, b"\r".to_vec());
    }

    #[tokio::test]
    async fn submit_prompt_with_outcome_returns_terminal_delivery_outcome() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

        let outcome = submit_prompt_via_sender(
            &tx,
            "gemini",
            "hello",
            || async { Ok(()) },
            || async { Ok(()) },
        )
        .await
        .expect("submit prompt");

        assert_eq!(rx.recv().await.expect("payload"), b"hello".to_vec());
        assert_eq!(rx.recv().await.expect("submit key"), b"\r".to_vec());
        assert_eq!(
            outcome.delivery_state,
            crate::utils::delivery_transaction::DELIVERY_STATE_SUBMIT_SENT_UNCONFIRMED
        );
        assert_eq!(outcome.delivery_phase, "submit_key_sent");
        assert_eq!(outcome.observed_state.as_deref(), Some("bytes_sent"));
    }

    #[test]
    fn opencode_submit_uses_kitty_protocol_return_key() {
        let chunks = provider_submit_chunks("opencode", "hello").expect("chunks");

        assert_eq!(chunks, vec![b"hello".to_vec(), b"\x1b[13u".to_vec()]);
    }

    #[test]
    fn provider_submit_chunks_reports_codex_bracketed_paste_payload() {
        let chunks = provider_submit_chunks("codex", "hello").expect("chunks");

        assert_eq!(
            chunks,
            vec![b"\x1b[200~hello\x1b[201~".to_vec(), b"\r".to_vec()]
        );
    }
}
