use tokio::sync::mpsc::Sender;

const TERMINAL_SUBMIT_DELAY_MS: u64 = 100;
const TERMINAL_SUBMIT_KEY: &[u8] = b"\r";

pub fn normalize_prompt_for_terminal_submit(prompt: &str) -> String {
    prompt
        .replace("\r\n", " ")
        .replace('\n', " ")
        .trim()
        .to_string()
}

pub async fn submit_prompt_via_sender(
    tx: &Sender<Vec<u8>>,
    prompt: &str,
    provider_name: &str,
) -> Result<(), String> {
    let normalized = normalize_prompt_for_terminal_submit(prompt);
    if normalized.is_empty() {
        return Ok(());
    }

    tx.send(normalized.into_bytes())
        .await
        .map_err(|e| format!("Failed to send prompt text: {}", e))?;

    tokio::time::sleep(std::time::Duration::from_millis(TERMINAL_SUBMIT_DELAY_MS)).await;

    let submit_key = if provider_name == "codex" {
        b"\x1b\r".as_slice()
    } else {
        TERMINAL_SUBMIT_KEY
    };

    tx.send(submit_key.to_vec())
        .await
        .map_err(|e| format!("Failed to send prompt submit key: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{normalize_prompt_for_terminal_submit, submit_prompt_via_sender};

    #[test]
    fn normalize_prompt_flattens_newlines_and_trims() {
        assert_eq!(
            normalize_prompt_for_terminal_submit("  alpha\nbeta\r\ngamma  "),
            "alpha beta gamma"
        );
    }

    #[tokio::test]
    async fn submit_prompt_sends_text_then_submit_key() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

        submit_prompt_via_sender(&tx, "hello\nworld", "gemini")
            .await
            .expect("submit prompt");

        let first = rx.recv().await.expect("first payload");
        let second = rx.recv().await.expect("second payload");

        assert_eq!(String::from_utf8(first).expect("utf8"), "hello world");
        assert_eq!(second, b"\r".to_vec());
    }
}
