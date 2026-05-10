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
    for (index, chunk) in terminal_submit_chunks(prompt, provider_name)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(TERMINAL_SUBMIT_DELAY_MS)).await;
        }

        tx.send(chunk)
            .await
            .map_err(|e| format!("Failed to send prompt input: {}", e))?;
    }

    Ok(())
}

pub fn terminal_submit_chunks(prompt: &str, provider_name: &str) -> Vec<Vec<u8>> {
    let normalized = normalize_prompt_for_terminal_submit(prompt);
    if normalized.is_empty() {
        return Vec::new();
    }

    let submit_key = if provider_name == "codex" {
        b"\x1b\r".as_slice()
    } else {
        TERMINAL_SUBMIT_KEY
    };

    vec![normalized.into_bytes(), submit_key.to_vec()]
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_prompt_for_terminal_submit, submit_prompt_via_sender, terminal_submit_chunks,
    };

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

    #[test]
    fn terminal_submit_chunks_uses_codex_escape_enter_submit_key() {
        assert_eq!(
            terminal_submit_chunks("hello", "codex"),
            vec![b"hello".to_vec(), b"\x1b\r".to_vec()]
        );
    }
}
