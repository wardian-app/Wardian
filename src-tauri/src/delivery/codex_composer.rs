use std::sync::{Arc, Mutex};

use crate::state::{AgentWatchState, AppState, TerminalSessionBroker};
use crate::utils::delivery_transaction::TerminalDeliveryError;
use crate::utils::strip_ansi_controls;

const PAYLOAD_APPLY_TIMEOUT_MS: u64 = 15_000;
// The canonical snapshot clones the screen and formats scrollback, so it is far
// more costly than a watch delta. Poll it on its own slower cadence instead of
// once per 25ms delta iteration; payload application is a multi-second event.
const CANONICAL_POLL_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationScope {
    TransactionDelta,
    ActiveTerminalSnapshot,
    ActivePromptFallback,
}

/// Canonical composer facts captured *before* Wardian writes the payload.
///
/// A canonical observation taken after the write only proves that this write
/// landed when both hold: the runtime was not replaced underneath it, and the
/// payload was not already sitting in the composer. Without the generation
/// fence a foreign runtime could satisfy the gate; without the stale fence a
/// leftover draft from an earlier attempt could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposerWriteBaseline {
    runtime_generation: u64,
    payload_already_applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerObservation {
    literal_match_bytes: usize,
    normalized_payload_bytes: usize,
    marker_format: &'static str,
    marker_chars: Option<usize>,
    codex_version: Option<String>,
    source: &'static str,
}

impl ComposerObservation {
    fn confirms_payload(&self) -> bool {
        self.marker_chars.is_some()
            || (self.normalized_payload_bytes > 0
                && self.literal_match_bytes == self.normalized_payload_bytes)
    }

    fn observed_state(&self) -> String {
        format!(
            "literal_match_bytes={};normalized_payload_bytes={};marker_format={};marker_chars={};codex_version={};observation_source={}",
            self.literal_match_bytes,
            self.normalized_payload_bytes,
            self.marker_format,
            self.marker_chars
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.codex_version.as_deref().unwrap_or("unknown"),
            self.source,
        )
    }
}

async fn terminal_output_snapshot(state: &AppState, session_id: &str) -> Result<String, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| format!("Agent {session_id} not found or is off"))?
            .watch_state
            .clone()
    };
    let snapshot = watch_state
        .lock()
        .map_err(|_| format!("Agent {session_id} watch state lock poisoned"))?
        .snapshot_since(None, None)
        .map(|snapshot| snapshot.output.text)
        .map_err(|error| format!("watch state error: {}", error.code()));
    snapshot
}

pub async fn session_has_stalled_composer(
    state: &AppState,
    session_id: &str,
) -> Result<bool, String> {
    terminal_output_snapshot(state, session_id)
        .await
        .map(|output| pending_paste_chars(&output).is_some())
}

/// Record the canonical composer state before the payload write so a later
/// canonical observation can be treated as positive proof that *this* write
/// landed. Returns `None` when no canonical snapshot is available, which keeps
/// the gate on transaction-delta evidence alone rather than guessing.
pub async fn capture_composer_write_baseline(
    state: &AppState,
    session_id: &str,
    prompt: &str,
) -> Option<ComposerWriteBaseline> {
    let snapshot = state.terminal_sessions.snapshot(session_id).await.ok()?;
    Some(ComposerWriteBaseline {
        runtime_generation: snapshot.runtime_generation,
        payload_already_applied: observe_payload_application(
            &snapshot.visible_grid,
            prompt,
            ObservationScope::ActiveTerminalSnapshot,
        )
        .confirms_payload(),
    })
}

/// Wait for Codex to prove that it has applied the bracketed paste to its
/// composer before Wardian sends Return. ConPTY's write receipt only proves
/// that bytes reached the PTY, not that Codex's event loop consumed them.
pub async fn wait_for_payload_applied_before_submit(
    state: &AppState,
    session_id: &str,
    since_cursor: &str,
    prompt: &str,
    baseline: Option<ComposerWriteBaseline>,
) -> Result<(), TerminalDeliveryError> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .ok_or_else(|| {
                TerminalDeliveryError::terminal_state_unknown(
                    "payload_apply_unconfirmed",
                    format!("Agent {session_id} not found or is off after payload write"),
                )
            })?
            .watch_state
            .clone()
    };
    wait_for_watch_payload_applied(
        watch_state,
        state.terminal_sessions.clone(),
        session_id,
        since_cursor,
        prompt,
        baseline,
    )
    .await
}

async fn wait_for_watch_payload_applied(
    watch_state: Arc<Mutex<AgentWatchState>>,
    terminal_sessions: Arc<TerminalSessionBroker>,
    session_id: &str,
    since_cursor: &str,
    prompt: &str,
    baseline: Option<ComposerWriteBaseline>,
) -> Result<(), TerminalDeliveryError> {
    let started = tokio::time::Instant::now();
    let mut last_canonical_poll: Option<tokio::time::Instant> = None;
    let mut best_observation = ComposerObservation {
        literal_match_bytes: 0,
        normalized_payload_bytes: normalize_echo_text(prompt).len(),
        marker_format: "absent",
        marker_chars: None,
        codex_version: None,
        source: "transaction_delta",
    };
    while started.elapsed() < std::time::Duration::from_millis(PAYLOAD_APPLY_TIMEOUT_MS) {
        let (output, scope) = {
            let watch_state = watch_state.lock().map_err(|_| {
                TerminalDeliveryError::terminal_state_unknown(
                    "payload_apply_unconfirmed",
                    format!("Agent {session_id} watch state lock poisoned after payload write"),
                )
            })?;
            // Do not tail-cap this transaction delta. Codex can emit enough
            // startup/repaint traffic to discard the collapsed-paste marker.
            // If churn expires the cursor, the delivery lock plus active-prompt
            // parser safely scope the retained composer fallback.
            match watch_state.snapshot_since(Some(since_cursor), None) {
                Ok(snapshot) => (snapshot.output.text, ObservationScope::TransactionDelta),
                Err(error) if error.code() == "cursor_expired" => watch_state
                    .snapshot_since(None, None)
                    .map(|snapshot| (snapshot.output.text, ObservationScope::ActivePromptFallback))
                    .map_err(|fallback_error| {
                        TerminalDeliveryError::terminal_state_unknown(
                            "payload_apply_unconfirmed",
                            format!(
                                "watch state fallback error after payload write: {}",
                                fallback_error.code()
                            ),
                        )
                    })?,
                Err(error) => {
                    return Err(TerminalDeliveryError::terminal_state_unknown(
                        "payload_apply_unconfirmed",
                        format!("watch state error after payload write: {}", error.code()),
                    ));
                }
            }
        };
        let observation = observe_payload_application(&output, prompt, scope);
        // A partial diff repaint can redraw the composer without ever emitting
        // the whole payload contiguously into this transaction delta, so the
        // delta alone must not decide whether the canonical screen is worth
        // reading. Poll the canonical screen on its own cadence as well.
        let canonical_due = last_canonical_poll
            .map(|polled_at: tokio::time::Instant| {
                polled_at.elapsed() >= std::time::Duration::from_millis(CANONICAL_POLL_INTERVAL_MS)
            })
            .unwrap_or(true);
        if observation.confirms_payload() || canonical_due {
            if let Ok(snapshot) = terminal_sessions.snapshot(session_id).await {
                last_canonical_poll = Some(tokio::time::Instant::now());
                let active_observation = observe_payload_application(
                    &snapshot.visible_grid,
                    prompt,
                    ObservationScope::ActiveTerminalSnapshot,
                );
                let confirms_current_composer = observation.confirms_payload()
                    && match scope {
                        ObservationScope::TransactionDelta => {
                            transaction_evidence_matches_active_composer(
                                &observation,
                                &active_observation,
                            )
                        }
                        ObservationScope::ActivePromptFallback
                        | ObservationScope::ActiveTerminalSnapshot => {
                            active_observation.confirms_payload()
                        }
                    };
                let confirms_post_write_canonical = canonical_proof_confirms_write(
                    &active_observation,
                    baseline,
                    snapshot.runtime_generation,
                );
                if confirms_current_composer || confirms_post_write_canonical {
                    return Ok(());
                }
                if active_observation.literal_match_bytes > best_observation.literal_match_bytes
                    || (best_observation.marker_format == "absent"
                        && active_observation.marker_format != "absent")
                {
                    best_observation = active_observation;
                }
            }
        }
        if observation.literal_match_bytes > best_observation.literal_match_bytes
            || (best_observation.marker_format == "absent" && observation.marker_format != "absent")
        {
            best_observation = observation;
        } else if best_observation.codex_version.is_none() && observation.codex_version.is_some() {
            best_observation.codex_version = observation.codex_version;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    if best_observation.codex_version.is_none() {
        best_observation.codex_version =
            crate::providers::models::installed_provider_version("codex").await;
    }

    Err(TerminalDeliveryError::terminal_state_unknown(
        "payload_apply_unconfirmed",
        format!(
            "Timed out waiting for {session_id} Codex composer to apply the payload; Return was not sent"
        ),
    )
    .with_observation(
        best_observation.observed_state(),
        "Codex composer evidence remained incomplete; diagnostics are counts and provider format only and do not include prompt content",
    ))
}

pub fn output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    if crate::delivery::codex_menu::current_screen_requires_choice(&cleaned)
        || output_has_workspace_trust_prompt(&cleaned)
        || pending_paste_chars(&cleaned).is_some()
    {
        return false;
    }
    let mut trailing_metadata_lines = 0usize;
    for line in cleaned.lines().rev().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('›') {
            return true;
        }
        if trailing_metadata_lines < 3 && ready_prompt_trailing_metadata_line(line) {
            trailing_metadata_lines += 1;
            continue;
        }
        return false;
    }
    false
}

pub fn output_has_workspace_trust_prompt(output: &str) -> bool {
    output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .contains("do you trust the contents of this directory?")
}

fn observe_payload_application(
    output: &str,
    prompt: &str,
    scope: ObservationScope,
) -> ComposerObservation {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    // Codex can paint the complete draft while still loading a resumed session.
    // Composer application alone must not release Return on that startup screen.
    // Check only the canonical screen, never retained raw startup history.
    let starting =
        scope == ObservationScope::ActiveTerminalSnapshot && active_screen_is_starting(&cleaned);
    let observed = match scope {
        // A transaction delta can nominate evidence, but the caller must
        // cross-check it against the broker's canonical active screen before
        // treating it as current-composer proof.
        ObservationScope::TransactionDelta => cleaned.as_str(),
        ObservationScope::ActiveTerminalSnapshot if starting => "",
        ObservationScope::ActiveTerminalSnapshot => {
            cleaned.rsplit_once('›').map_or("", |(_, tail)| tail)
        }
        ObservationScope::ActivePromptFallback => cleaned
            .rsplit_once('›')
            .map_or(cleaned.as_str(), |(_, tail)| tail),
    };
    let marker_chars = paste_marker_chars(observed);
    let normalized_observed = normalize_echo_text(observed);
    let token = normalize_echo_text(prompt);
    ComposerObservation {
        literal_match_bytes: longest_prefix_match_bytes(&normalized_observed, &token),
        normalized_payload_bytes: token.len(),
        marker_format: if starting {
            "startup_in_progress"
        } else if marker_chars.is_some() {
            "pasted_content_chars"
        } else if marker_like_text(observed) {
            "unrecognized_marker_like"
        } else {
            "absent"
        },
        marker_chars,
        codex_version: codex_version(output),
        source: match scope {
            ObservationScope::TransactionDelta => "transaction_delta",
            ObservationScope::ActiveTerminalSnapshot => "active_terminal_snapshot",
            ObservationScope::ActivePromptFallback => "active_prompt_fallback",
        },
    }
}

pub(crate) fn active_screen_is_starting(screen: &str) -> bool {
    let Some((before_composer, _)) = screen.rsplit_once('›') else {
        return false;
    };
    before_composer.lines().map(str::trim).any(|line| {
        matches!(line, "Resuming session…" | "Resuming session...")
            || line
                .strip_prefix('│')
                .map(str::trim_start)
                .and_then(|line| line.strip_prefix("model:"))
                .and_then(|model| model.split_whitespace().next())
                == Some("loading")
    })
}

/// Positive post-write proof that this write reached the composer.
///
/// A partial diff repaint can omit payload bytes from the transaction delta
/// while the canonical screen holds the whole payload, so the delta cannot be
/// the only admissible evidence. The canonical screen is admissible only inside
/// these boundaries:
///
/// * `active.confirms_payload()` requires the exact, complete payload. It is
///   built from the text after the last prompt caret, so scrollback history and
///   an already-submitted turn cannot qualify, and it yields nothing while a
///   resumed session is still on its startup screen or a model menu.
/// * The snapshot must come from the runtime generation this write targeted, so
///   a replaced or foreign runtime cannot satisfy the gate.
/// * The payload must not already have been applied before the write, so a
///   stale draft left by an earlier attempt is not mistaken for new evidence.
///
/// Without a baseline there is nothing to fence against, so canonical-only
/// evidence is refused and the gate falls back to transaction-delta proof.
fn canonical_proof_confirms_write(
    active: &ComposerObservation,
    baseline: Option<ComposerWriteBaseline>,
    snapshot_runtime_generation: u64,
) -> bool {
    active.confirms_payload()
        && baseline.is_some_and(|baseline| {
            baseline.runtime_generation == snapshot_runtime_generation
                && !baseline.payload_already_applied
        })
}

fn transaction_evidence_matches_active_composer(
    transaction: &ComposerObservation,
    active_composer: &ComposerObservation,
) -> bool {
    match transaction.marker_chars {
        Some(marker_chars) => active_composer.marker_chars == Some(marker_chars),
        None => {
            transaction.normalized_payload_bytes > 0
                && transaction.literal_match_bytes == transaction.normalized_payload_bytes
                && active_composer.literal_match_bytes == active_composer.normalized_payload_bytes
        }
    }
}

fn pending_paste_chars(output: &str) -> Option<usize> {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let active_prompt = cleaned.rsplit_once('›')?.1;
    paste_marker_chars(active_prompt)
}

fn paste_marker_chars(output: &str) -> Option<usize> {
    const PREFIX: &str = "[Pasted Content ";
    const SUFFIX: &str = " chars]";

    // Cursor movement can wrap the marker between "Pasted" and "Content".
    let normalized = output.split_whitespace().collect::<Vec<_>>().join(" ");
    let marker_start = normalized.rfind(PREFIX)? + PREFIX.len();
    let remainder = &normalized[marker_start..];
    let marker_end = remainder.find(SUFFIX)?;
    remainder[..marker_end].trim().parse().ok()
}

fn marker_like_text(output: &str) -> bool {
    let normalized = output.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.contains("[Pasted") || normalized.contains("Pasted Content")
}

fn codex_version(output: &str) -> Option<String> {
    let marker = "OpenAI Codex (v";
    let start = output.rfind(marker)? + marker.len();
    let version = output[start..].split(')').next()?.trim();
    (!version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character)))
    .then(|| version.to_string())
}

fn longest_prefix_match_bytes(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let pattern = needle.as_bytes();
    let mut prefix = vec![0usize; pattern.len()];
    for index in 1..pattern.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    let mut matched = 0usize;
    let mut best = 0usize;
    for byte in haystack.bytes() {
        while matched > 0 && byte != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if byte == pattern[matched] {
            matched += 1;
            best = best.max(matched);
            if matched == pattern.len() {
                return matched;
            }
        }
    }
    best
}

fn normalize_echo_text(text: &str) -> String {
    strip_ansi_controls(text)
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn ready_prompt_trailing_metadata_line(line: &str) -> bool {
    if line.contains('•') {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    lower.starts_with("gpt-") && (line.contains('·') || lower.contains("context"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) async fn record_active_composer_repaint(
        state: &AppState,
        session_id: &str,
        output: &[u8],
    ) {
        let (watch_state, generation) = {
            let agents = state.agents.lock().await;
            let agent = agents.get(session_id).expect("test agent");
            (
                agent.watch_state.clone(),
                agent.runtime_generation.expect("test runtime generation"),
            )
        };
        watch_state.lock().unwrap().push_output(output);
        let terminal_sessions = state.terminal_sessions.clone();
        let session_id = session_id.to_string();
        let output = output.to_vec();
        tokio::task::spawn_blocking(move || {
            terminal_sessions.process_output_blocking(&session_id, generation, output)
        })
        .await
        .expect("terminal output task")
        .expect("canonical terminal repaint");
    }

    #[test]
    fn ready_prompt_detects_visible_compose_prompt() {
        assert!(output_has_ready_prompt("\r\n› Write tests for @filename"));
        assert!(output_has_ready_prompt(
            "\r\n›\u{1b}[22m Write tests for @filename"
        ));
        assert!(output_has_ready_prompt(
            "\r\n› Explain this codebase\r\n\r\n  gpt-5.5 high · Context 100% left · C:\\projects\\example\r\n"
        ));
        assert!(!output_has_ready_prompt("Booting MCP server"));
    }

    #[test]
    fn ready_prompt_rejects_active_or_historical_collapsed_paste() {
        let active =
            "\r\n› [Pasted Content 6479 chars]\r\n\r\n  gpt-5.5 high · Context 49% left\r\n";
        assert!(!output_has_ready_prompt(active));
        assert_eq!(pending_paste_chars(active), Some(6479));

        let historical =
            "\r\n› [Pasted Content 6479 chars]\r\nresponse\r\n› Ask Codex to do anything\r\n";
        assert_eq!(pending_paste_chars(historical), None);
        assert!(output_has_ready_prompt(historical));
    }

    #[test]
    fn pending_paste_accepts_marker_wrapped_by_cursor_movement() {
        let output = "\r\n› visible prefix [Pasted\x1b[23;1H  Content 5865 chars]\r\n";
        assert_eq!(pending_paste_chars(output), Some(5865));
        assert!(observe_payload_application(
            output,
            &"x".repeat(7_000),
            ObservationScope::ActivePromptFallback
        )
        .confirms_payload());
    }

    #[test]
    fn payload_application_requires_complete_current_payload() {
        assert!(observe_payload_application(
            "\r\n› hello",
            "hello",
            ObservationScope::ActivePromptFallback
        )
        .confirms_payload());
        assert!(observe_payload_application(
            "\r\n› visible prefix [Pasted Content 5890 chars]",
            &"x".repeat(7_000),
            ObservationScope::ActivePromptFallback
        )
        .confirms_payload());
        assert!(!observe_payload_application(
            "\r\n› first line",
            "first line\nsecond line",
            ObservationScope::ActivePromptFallback
        )
        .confirms_payload());
        assert!(!observe_payload_application(
            "\r\n› hello\r\nresponse\r\n› Ask Codex to do anything",
            "hello",
            ObservationScope::ActivePromptFallback
        )
        .confirms_payload());
    }

    #[test]
    fn startup_screen_does_not_release_submit_for_an_applied_draft() {
        for draft in ["verification marker", "[Pasted Content 6479 chars]"] {
            let transaction =
                observe_payload_application(draft, draft, ObservationScope::TransactionDelta);
            for startup in [
                "│ model:     loading   /model to change │\r\nResuming session…",
                "│ model:     loading   /model to change │",
                "Resuming session...",
            ] {
                let active = observe_payload_application(
                    &format!("OpenAI Codex (v0.153.4)\r\n{startup}\r\n› {draft}"),
                    draft,
                    ObservationScope::ActiveTerminalSnapshot,
                );
                assert!(!transaction_evidence_matches_active_composer(
                    &transaction,
                    &active
                ));
                assert_eq!(active.marker_format, "startup_in_progress");
            }
            // The same pending draft can qualify when the current screen has
            // finished startup; no second paste or fixed delay is necessary.
            let active = observe_payload_application(
                &format!("│ model: gpt-5.4-mini /model to change │\r\n› {draft}"),
                draft,
                ObservationScope::ActiveTerminalSnapshot,
            );
            assert!(transaction_evidence_matches_active_composer(
                &transaction,
                &active
            ));
        }
    }

    #[test]
    fn startup_history_and_prompt_text_do_not_block_the_current_composer() {
        let prompt = "Explain this message:\nResuming session…";
        let transaction = observe_payload_application(
            &format!("Resuming session…\r\n› {prompt}"),
            prompt,
            ObservationScope::TransactionDelta,
        );
        let active = observe_payload_application(
            &format!("│ model: gpt-5.4-mini /model to change │\r\n› {prompt}"),
            prompt,
            ObservationScope::ActiveTerminalSnapshot,
        );
        assert!(transaction_evidence_matches_active_composer(
            &transaction,
            &active
        ));
    }

    #[test]
    fn transaction_delta_accepts_cell_only_marker_only_on_active_composer() {
        let transaction = observe_payload_application(
            "\x1b[22;3H[Pasted Content 6323 chars]\x1b[K",
            &"x".repeat(7_000),
            ObservationScope::TransactionDelta,
        );
        let active_composer = observe_payload_application(
            "\r\n› [Pasted Content 6323 chars]",
            &"x".repeat(7_000),
            ObservationScope::ActiveTerminalSnapshot,
        );

        assert!(transaction.confirms_payload());
        assert!(transaction_evidence_matches_active_composer(
            &transaction,
            &active_composer
        ));
        assert_eq!(active_composer.source, "active_terminal_snapshot");
    }

    #[test]
    fn transaction_delta_rejects_unrelated_or_stale_marker() {
        let transaction = observe_payload_application(
            "MCP output: [Pasted Content 6323 chars]",
            &"x".repeat(7_000),
            ObservationScope::TransactionDelta,
        );
        let unrelated_active_screen = observe_payload_application(
            "\r\nlog replay [Pasted Content 6323 chars]\r\n› Ask Codex to do anything",
            &"x".repeat(7_000),
            ObservationScope::ActiveTerminalSnapshot,
        );
        let different_active_marker = observe_payload_application(
            "\r\n› [Pasted Content 4021 chars]",
            &"x".repeat(7_000),
            ObservationScope::ActiveTerminalSnapshot,
        );

        assert!(transaction.confirms_payload());
        assert!(!transaction_evidence_matches_active_composer(
            &transaction,
            &unrelated_active_screen
        ));
        assert!(!transaction_evidence_matches_active_composer(
            &transaction,
            &different_active_marker
        ));
        let no_active_prompt = observe_payload_application(
            "\r\nlog replay [Pasted Content 6323 chars]",
            &"x".repeat(7_000),
            ObservationScope::ActiveTerminalSnapshot,
        );
        assert!(!transaction_evidence_matches_active_composer(
            &transaction,
            &no_active_prompt
        ));
    }

    #[test]
    fn diagnostics_classify_unknown_marker_and_provider_version_without_content() {
        let observation = observe_payload_application(
            "OpenAI Codex (v0.151.0)\r\n\x1b[22;3H[Pasted text 6400 bytes]",
            "private payload",
            ObservationScope::TransactionDelta,
        );

        assert!(!observation.confirms_payload());
        assert_eq!(observation.marker_format, "unrecognized_marker_like");
        assert_eq!(observation.codex_version.as_deref(), Some("0.151.0"));
        assert!(!observation.observed_state().contains("private payload"));
    }

    #[test]
    fn diagnostics_report_partial_literal_match() {
        let observation = observe_payload_application(
            "\x1b[22;3Hfirst line second",
            "first line second line third",
            ObservationScope::TransactionDelta,
        );

        assert_eq!(observation.literal_match_bytes, "first line second".len());
        assert_eq!(
            observation.normalized_payload_bytes,
            "first line second line third".len()
        );
    }

    #[tokio::test]
    async fn payload_application_recovers_from_repaint_cursor_expiry() {
        let watch_state = Arc::new(Mutex::new(AgentWatchState::new(
            "agent-1".to_string(),
            16,
            262_144,
        )));
        let terminal_sessions = Arc::new(TerminalSessionBroker::default());
        let (input_tx, _input_rx) = tokio::sync::mpsc::channel(1);
        let generation = terminal_sessions
            .start_or_replace_runtime(
                "agent-1",
                crate::state::terminal_session::TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry { cols: 80, rows: 24 },
            )
            .await
            .expect("test terminal runtime");
        let cursor = watch_state.lock().unwrap().latest_cursor();
        {
            let mut state = watch_state.lock().unwrap();
            for _ in 0..17 {
                state.push_output(b"\x1b[?2026h");
            }
            state.push_output(b"\r\n\xe2\x80\xba [Pasted Content 5890 chars]\r\n");
        }
        let output_broker = terminal_sessions.clone();
        tokio::task::spawn_blocking(move || {
            output_broker.process_output_blocking(
                "agent-1",
                generation,
                b"\r\n\xe2\x80\xba [Pasted Content 5890 chars]\r\n".to_vec(),
            )
        })
        .await
        .expect("terminal output task")
        .expect("canonical terminal repaint");

        wait_for_watch_payload_applied(
            watch_state,
            terminal_sessions,
            "agent-1",
            &cursor,
            &"x".repeat(7_000),
            None,
        )
        .await
        .expect("active composer evidence should survive cursor expiry");
    }

    /// Sanitized from the observed resume failure: after pause/resume the
    /// partial diff repaint carried only the first 26 bytes of the 91-byte
    /// follow-up into the transaction delta ("Without tools, repeat your",
    /// ending mid-payload before " previous"), while the canonical screen held
    /// the whole payload in the active composer. The old gate never read the
    /// canonical screen because the delta had not confirmed, so it timed out
    /// with literal_match_bytes=26 / normalized_payload_bytes=91 and never sent
    /// Return.
    #[tokio::test]
    async fn partial_repaint_delta_accepts_canonical_active_composer_proof() {
        const PAYLOAD: &str =
            "Without tools, repeat your previous final answer in lowercase. Reply only with that answer.";
        const REPAINTED_PREFIX: &str = "Without tools, repeat your";
        assert_eq!(PAYLOAD.len(), 91);
        assert_eq!(REPAINTED_PREFIX.len(), 26);

        let watch_state = Arc::new(Mutex::new(AgentWatchState::new(
            "agent-1".to_string(),
            16,
            262_144,
        )));
        let terminal_sessions = Arc::new(TerminalSessionBroker::default());
        let (input_tx, _input_rx) = tokio::sync::mpsc::channel(1);
        let generation = terminal_sessions
            .start_or_replace_runtime(
                "agent-1",
                crate::state::terminal_session::TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    cols: 120,
                    rows: 24,
                },
            )
            .await
            .expect("test terminal runtime");
        let cursor = watch_state.lock().unwrap().latest_cursor();

        // The delta only ever repaints a fragment of the composer line.
        watch_state
            .lock()
            .unwrap()
            .push_output(format!("\x1b[22;3H{REPAINTED_PREFIX}\x1b[K").as_bytes());
        let partial = observe_payload_application(
            &format!("\x1b[22;3H{REPAINTED_PREFIX}\x1b[K"),
            PAYLOAD,
            ObservationScope::TransactionDelta,
        );
        assert_eq!(partial.literal_match_bytes, 26);
        assert_eq!(partial.normalized_payload_bytes, 91);
        assert!(!partial.confirms_payload());

        let canonical =
            format!("│ model:     gpt-5.4-mini low   /model to change │\r\n\r\n› {PAYLOAD}\r\n");
        let output_broker = terminal_sessions.clone();
        let canonical_bytes = canonical.clone().into_bytes();
        tokio::task::spawn_blocking(move || {
            output_broker.process_output_blocking("agent-1", generation, canonical_bytes)
        })
        .await
        .expect("terminal output task")
        .expect("canonical terminal repaint");

        wait_for_watch_payload_applied(
            watch_state,
            terminal_sessions,
            "agent-1",
            &cursor,
            PAYLOAD,
            Some(ComposerWriteBaseline {
                runtime_generation: generation,
                payload_already_applied: false,
            }),
        )
        .await
        .expect("canonical active composer holds the exact payload on this runtime");
    }

    #[test]
    fn canonical_proof_holds_only_inside_its_runtime_and_staleness_fences() {
        const PAYLOAD: &str = "recall the previous answer in lowercase";
        let applied = observe_payload_application(
            &format!("│ model: gpt-5.4-mini low /model to change │\r\n› {PAYLOAD}"),
            PAYLOAD,
            ObservationScope::ActiveTerminalSnapshot,
        );
        assert!(applied.confirms_payload());

        let fresh = ComposerWriteBaseline {
            runtime_generation: 2,
            payload_already_applied: false,
        };
        assert!(canonical_proof_confirms_write(&applied, Some(fresh), 2));

        // A runtime replaced between the write and the observation makes the
        // canonical screen a foreign runtime's screen.
        assert!(!canonical_proof_confirms_write(&applied, Some(fresh), 3));

        // The payload was already sitting in the composer before the write, so
        // seeing it afterwards proves nothing about this write.
        assert!(!canonical_proof_confirms_write(
            &applied,
            Some(ComposerWriteBaseline {
                runtime_generation: 2,
                payload_already_applied: true,
            }),
            2,
        ));

        // No baseline means no fence, so canonical-only evidence is refused.
        assert!(!canonical_proof_confirms_write(&applied, None, 2));

        // History, a submitted turn, and a still-starting resume never qualify,
        // even with a clean baseline on the right runtime.
        for screen in [
            format!("› {PAYLOAD}\r\nresponse\r\n› Ask Codex to do anything"),
            format!("│ model:     loading   /model to change │\r\n› {PAYLOAD}"),
            format!("Resuming session…\r\n› {PAYLOAD}"),
        ] {
            let observation = observe_payload_application(
                &screen,
                PAYLOAD,
                ObservationScope::ActiveTerminalSnapshot,
            );
            assert!(
                !canonical_proof_confirms_write(&observation, Some(fresh), 2),
                "screen must not release Return: {screen}"
            );
        }
    }

    #[test]
    fn ready_prompt_rejects_workspace_trust_or_busy_tail() {
        assert!(!output_has_ready_prompt(
            "\r\n› 1. Yes, continue\r\n  2. No, quit\r\nDo you trust the contents of this directory?\r\nPress enter to continue"
        ));
        for busy in [
            "Processing request",
            "Thinking about the request",
            "Final response: complete",
            "Final response: gpt-5 · context window",
        ] {
            assert!(!output_has_ready_prompt(&format!(
                "\r\n› Previous prompt\r\n{busy}\r\n"
            )));
        }
    }
}
