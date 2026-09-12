use super::*;
use crate::state::terminal_session::{
    forward_terminal_output, NativeTerminalWriteRequest, TerminalRuntimeHandles,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use wardian_core::models::TerminalGeometry;

fn responder(default_colors: DefaultColorReplies) -> CodexTerminalThemeProbeResponder {
    CodexTerminalThemeProbeResponder {
        default_colors,
        ..Default::default()
    }
}

#[test]
fn default_color_policy_is_scoped_to_the_windows_console_fallback() {
    assert_eq!(
        DefaultColorReplies::default(),
        if cfg!(windows) {
            DefaultColorReplies::NativeConsoleFallback
        } else {
            DefaultColorReplies::Respond
        }
    );
    for policy in [
        DefaultColorReplies::Respond,
        DefaultColorReplies::NativeConsoleFallback,
    ] {
        for theme in ["light", "dark", "unknown"] {
            for provider in ["claude", "opencode", "antigravity", "pi", "gemini", ""] {
                assert!(responder(policy)
                    .responses_for_chunk(
                        provider,
                        b"\x1b[?996n\x1b]10;?\x07\x1b]11;?\x07\x1b]4;0;?\x07",
                        theme
                    )
                    .is_empty());
            }
            let replies =
                responder(policy).responses_for_chunk("codex", b"\x1b[?996n\x1b]4;0;?\x07", theme);
            assert_eq!(
                replies,
                if theme == "light" {
                    vec![
                        b"\x1b[?997;2n".to_vec(),
                        b"\x1b]4;0;rgb:fc/fa/f5\x07".to_vec(),
                    ]
                } else {
                    vec![
                        b"\x1b[?997;1n".to_vec(),
                        b"\x1b]4;0;rgb:02/04/02\x07".to_vec(),
                    ]
                }
            );
        }
    }
}

/// Observes the acknowledged native-writer channel, not a response generator
/// alone. Feed the production PTY output dispatch before/after user paste.
async fn broker_writes_around_query(
    policy: DefaultColorReplies,
    theme: &'static str,
    query: Vec<u8>,
    split: usize,
    prompt: Vec<u8>,
) -> Vec<Vec<u8>> {
    let broker = Arc::new(TerminalSessionBroker::default());
    let (input_tx, mut input_rx) = mpsc::channel::<NativeTerminalWriteRequest>(256);
    let writer = tokio::spawn(async move {
        let mut writes = Vec::new();
        while let Some(request) = input_rx.recv().await {
            writes.push(request.bytes);
            request
                .completion
                .send(Ok(()))
                .expect("acknowledge native write");
        }
        writes
    });
    let generation = broker
        .start_or_replace_runtime(
            "theme-probe",
            TerminalRuntimeHandles::new_with_write_ack(input_tx, |_| Ok(())),
            TerminalGeometry { cols: 80, rows: 24 },
        )
        .await
        .expect("runtime");
    let output_broker = broker.clone();
    tokio::task::spawn_blocking(move || {
        let mut responder = responder(policy);
        let mut output = |bytes: &[u8]| {
            forward_terminal_output(&output_broker, "theme-probe", generation, bytes)
                .expect("output");
            responder.respond_to_output(
                &output_broker,
                "theme-probe",
                generation,
                "codex",
                bytes,
                theme,
            );
        };
        output(&query[..split]);
        output_broker
            .send_privileged_input_blocking("theme-probe", generation, prompt)
            .expect("paste");
        // Delay/interleaving is deterministic: remaining bytes arrive only after paste.
        for byte in &query[split..] {
            output(&[*byte]);
        }
        output_broker
            .send_privileged_input_blocking("theme-probe", generation, b"\r".to_vec())
            .expect("submit");
        // Repaint and repeated probes must not introduce a later response either.
        output(&query);
    })
    .await
    .expect("reader thread");
    broker
        .pause_runtime("theme-probe", generation)
        .await
        .expect("pause");
    writer.await.expect("native writer")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_default_color_queries_cannot_add_bytes_to_native_user_input() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/codex-v7-request.json"))
            .expect("genuine sanitized capture");
    let prompt = fixture["submitted_prompt"].as_str().expect("prompt");
    assert_eq!(
        fixture["native_prompt"].as_str().unwrap(),
        format!("{prompt}{}", fixture["suffix"].as_str().unwrap())
    );
    let paste = format!("\x1b[200~{prompt}\x1b[201~").into_bytes();
    for theme in ["light", "dark"] {
        for query in [
            b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\".to_vec(),
            b"\x1b]10;?\x07\x1b]11;?\x07".to_vec(),
        ] {
            for split in 0..=query.len() {
                assert_eq!(
                    broker_writes_around_query(
                        DefaultColorReplies::NativeConsoleFallback,
                        theme,
                        query.clone(),
                        split,
                        paste.clone()
                    )
                    .await,
                    vec![paste.clone(), b"\r".to_vec()],
                    "{theme} query split {split}"
                );
            }
        }
    }
    // User-supplied report-looking text is still exact input, never stripped.
    let literal = fixture["native_prompt"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec();
    assert_eq!(
        broker_writes_around_query(
            DefaultColorReplies::NativeConsoleFallback,
            "light",
            b"\x1b]10;?\x07".to_vec(),
            3,
            literal.clone()
        )
        .await,
        vec![literal, b"\r".to_vec()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unix_policy_preserves_palette_and_exposes_old_windows_contamination() {
    for (theme, foreground, background) in [
        ("light", "11/18/27", "fc/fa/f5"),
        ("dark", "ee/f2/ee", "02/04/02"),
    ] {
        let paste = b"\x1b[200~scratch prompt\x1b[201~".to_vec();
        assert_eq!(
            broker_writes_around_query(
                DefaultColorReplies::Respond,
                theme,
                b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\".to_vec(),
                3,
                paste.clone()
            )
            .await,
            vec![
                paste,
                format!("\x1b]10;rgb:{foreground}\x1b\\").into_bytes(),
                format!("\x1b]11;rgb:{background}\x1b\\").into_bytes(),
                b"\r".to_vec()
            ]
        );
    }
}

#[test]
fn codex_terminal_theme_probe_responder_answers_light_theme_queries() {
    let mut responder = responder(DefaultColorReplies::Respond);

    let responses = responder.responses_for_chunk(
        "codex",
        b"\x1b[?996n\x1b]10;?\x1b\\\x1b]11;?\x1b\\",
        "light",
    );

    let responses: Vec<String> = responses
        .into_iter()
        .map(|response| String::from_utf8(response).expect("utf8 response"))
        .collect();
    assert_eq!(
        responses,
        vec![
            "\x1b[?997;2n".to_string(),
            "\x1b]10;rgb:11/18/27\x1b\\".to_string(),
            "\x1b]11;rgb:fc/fa/f5\x1b\\".to_string(),
        ]
    );
}

#[test]
fn codex_terminal_theme_probe_responder_handles_split_background_query() {
    let mut responder = responder(DefaultColorReplies::Respond);

    assert!(responder
        .responses_for_chunk("codex", b"\x1b]11", "dark")
        .is_empty());
    let responses = responder.responses_for_chunk("codex", b";?\x1b\\", "dark");

    assert_eq!(responses, vec![b"\x1b]11;rgb:02/04/02\x1b\\".to_vec()]);
    assert!(responder
        .responses_for_chunk("codex", b"\x1b]11;?\x1b\\", "dark")
        .is_empty());
}

#[test]
fn codex_terminal_theme_probe_responder_ignores_other_providers() {
    let mut responder = responder(DefaultColorReplies::Respond);

    let responses = responder.responses_for_chunk("opencode", b"\x1b]11;?\x1b\\", "light");

    assert!(responses.is_empty());
}
