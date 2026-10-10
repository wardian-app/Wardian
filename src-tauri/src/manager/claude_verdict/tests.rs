use super::*;

const PROMPT: &str = "7d0a7f0e-4c1e-4a59-8d0a-1f8e2b3c4d5e";
const OTHER_PROMPT: &str = "0b9d2c4e-8a51-4f0e-9c7b-6a5d4e3f2a1b";
const SESSION: &str = "claude-session-1";

fn accepted() -> Vec<String> {
    vec![SESSION.to_string()]
}

fn event(sequence: u64, name: &str, prompt_id: Option<&str>) -> TelemetryEvent {
    TelemetryEvent {
        sequence,
        name: name.to_string(),
        prompt_id: prompt_id.map(str::to_string),
        session_id: Some(SESSION.to_string()),
        timestamp: Some(format!("2026-10-10T07:00:{:02}.000Z", sequence % 60)),
        hook: None,
    }
}

fn other(sequence: u64) -> TelemetryEvent {
    event(sequence, "api_request", Some(PROMPT))
}

fn user_prompt(sequence: u64, prompt_id: &str) -> TelemetryEvent {
    event(sequence, "user_prompt", Some(prompt_id))
}

fn stop_start(sequence: u64, prompt_id: &str, hooks: u64) -> TelemetryEvent {
    TelemetryEvent {
        hook: Some(HookExecution {
            hook_event: "Stop".to_string(),
            num_hooks: Some(hooks),
            num_success: None,
            num_blocking: None,
            num_non_blocking_error: None,
            num_cancelled: None,
            additional_context_chars: None,
        }),
        ..event(sequence, "hook_execution_start", Some(prompt_id))
    }
}

fn stop_complete(
    sequence: u64,
    prompt_id: &str,
    hooks: u64,
    blocking: u64,
    additional_context_chars: u64,
) -> TelemetryEvent {
    TelemetryEvent {
        hook: Some(HookExecution {
            hook_event: "Stop".to_string(),
            num_hooks: Some(hooks),
            num_success: Some(hooks - blocking),
            num_blocking: Some(blocking),
            num_non_blocking_error: Some(0),
            num_cancelled: Some(0),
            additional_context_chars: Some(additional_context_chars),
        }),
        ..event(sequence, "hook_execution_complete", Some(prompt_id))
    }
}

fn ledger(events: impl IntoIterator<Item = TelemetryEvent>) -> TelemetryLedger {
    let mut ledger = TelemetryLedger::default();
    for event in events {
        ledger.insert(event);
    }
    ledger
}

fn round(prompt_id: &str, text: &str) -> String {
    serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": SESSION,
        "prompt_id": prompt_id,
        "last_assistant_message": text,
        "stop_hook_active": false,
    })
    .to_string()
}

fn rounds(lines: &[String]) -> StopRoundLog {
    let mut contents = lines.join("\n");
    contents.push('\n');
    StopRoundLog::parse(&contents)
}

/// One prompt whose first Stop round is blocked by another hook and whose
/// second round ends the turn.
fn continued_turn() -> Vec<TelemetryEvent> {
    vec![
        user_prompt(10, PROMPT),
        other(11),
        stop_start(12, PROMPT, 2),
        stop_complete(13, PROMPT, 2, 1, 0),
        other(14),
        stop_start(15, PROMPT, 2),
        stop_complete(16, PROMPT, 2, 0, 0),
    ]
}

fn verified_text(verdict: Verdict) -> String {
    match verdict {
        Verdict::Verified(completion) => completion.response_text,
        unexpected => panic!("expected a verified completion, got {unexpected:?}"),
    }
}

#[test]
fn single_settled_round_verifies_its_response() {
    let ledger = ledger([
        user_prompt(4, PROMPT),
        other(5),
        stop_start(6, PROMPT, 1),
        stop_complete(7, PROMPT, 1, 0, 0),
    ]);
    let verdict = verify_prompt(
        &ledger,
        &rounds(&[round(PROMPT, "Done.")]),
        PROMPT,
        &accepted(),
    );

    let Verdict::Verified(completion) = verdict else {
        panic!("expected verification, got {verdict:?}");
    };
    assert_eq!(completion.response_text, "Done.");
    assert_eq!(completion.prompt_id, PROMPT);
    assert_eq!(
        completion.completed_at.as_deref(),
        Some("2026-10-10T07:00:07.000Z")
    );
}

#[test]
fn continued_turn_verifies_only_the_final_rounds_response() {
    let log = rounds(&[round(PROMPT, "intermediate A"), round(PROMPT, "final B")]);
    let verdict = verify_prompt(&ledger(continued_turn()), &log, PROMPT, &accepted());
    assert_eq!(verified_text(verdict), "final B");
}

#[test]
fn a_blocked_round_alone_is_pending() {
    let ledger = ledger(continued_turn().into_iter().take(4));
    let verdict = verify_prompt(
        &ledger,
        &rounds(&[round(PROMPT, "intermediate A")]),
        PROMPT,
        &accepted(),
    );
    assert_eq!(verdict, Verdict::Pending);
}

#[test]
fn additional_context_continues_the_turn() {
    let ledger = ledger([
        user_prompt(1, PROMPT),
        stop_start(2, PROMPT, 1),
        stop_complete(3, PROMPT, 1, 0, 42),
    ]);
    let verdict = verify_prompt(
        &ledger,
        &rounds(&[round(PROMPT, "not final")]),
        PROMPT,
        &accepted(),
    );
    assert_eq!(verdict, Verdict::Pending);
}

#[test]
fn dropped_prior_complete_and_final_record_cannot_pair_stale_text() {
    // Round A's completion was lost in transit and round B's Stop record was
    // never written. Counts agree (one record, one final completion), but the
    // sequence gap proves the window is incomplete.
    let events = continued_turn()
        .into_iter()
        .filter(|event| event.sequence != 13);
    let log = rounds(&[round(PROMPT, "stale A")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Pending
    );
}

#[test]
fn an_unemitted_prior_complete_is_an_unpaired_round() {
    // Without a gap: round A's start exists but its completion was never
    // emitted, so the two starts are not separated by a completion.
    let events = continued_turn()
        .into_iter()
        .filter(|event| event.sequence != 13)
        .map(|mut event| {
            if event.sequence > 13 {
                event.sequence -= 1;
            }
            event
        });
    let log = rounds(&[round(PROMPT, "stale A")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::UnpairedStopRound)
    );
}

#[test]
fn a_missing_final_record_is_a_round_count_mismatch() {
    let log = rounds(&[round(PROMPT, "stale A")]);
    assert_eq!(
        verify_prompt(&ledger(continued_turn()), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::RoundCountMismatch)
    );
}

#[test]
fn an_extra_record_is_a_round_count_mismatch() {
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B"), round(PROMPT, "C")]);
    assert_eq!(
        verify_prompt(&ledger(continued_turn()), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::RoundCountMismatch)
    );
}

#[test]
fn a_late_batch_fills_the_gap_and_then_verifies() {
    let mut ledger = ledger(
        continued_turn()
            .into_iter()
            .filter(|event| event.sequence != 14),
    );
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger, &log, PROMPT, &accepted()),
        Verdict::Pending
    );

    assert_eq!(ledger.insert(other(14)), LedgerInsert::New);
    assert_eq!(
        verified_text(verify_prompt(&ledger, &log, PROMPT, &accepted())),
        "B"
    );
}

#[test]
fn retried_duplicate_events_are_idempotent() {
    let mut ledger = ledger(continued_turn());
    for event in continued_turn() {
        assert_eq!(ledger.insert(event), LedgerInsert::Duplicate);
    }
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verified_text(verify_prompt(&ledger, &log, PROMPT, &accepted())),
        "B"
    );
}

#[test]
fn a_conflicting_event_at_a_known_sequence_poisons_the_launch() {
    let mut ledger = ledger(continued_turn());
    assert_eq!(ledger.insert(other(16)), LedgerInsert::Conflict);
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger, &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::LedgerConflict)
    );
}

#[test]
fn a_missing_prompt_start_is_unknown() {
    let events = continued_turn()
        .into_iter()
        .filter(|event| event.sequence != 10);
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::MissingPromptStart)
    );
}

#[test]
fn a_second_prompt_start_is_unknown() {
    let mut events = continued_turn();
    events.push(user_prompt(17, PROMPT));
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::DuplicatePromptStart)
    );
}

#[test]
fn a_restarted_process_with_reused_sequences_is_a_separate_ledger() {
    // A resumed session in a new process restarts `event.sequence` at zero.
    // Its events go to that launch's own ledger; mixing them into this one
    // collides with existing sequences and poisons it rather than pairing.
    let mut ledger = ledger(continued_turn());
    let restarted = [
        user_prompt(10, OTHER_PROMPT),
        stop_start(11, OTHER_PROMPT, 1),
        stop_complete(12, OTHER_PROMPT, 1, 0, 0),
    ];
    for event in restarted {
        ledger.insert(event);
    }
    let log = rounds(&[
        round(PROMPT, "A"),
        round(PROMPT, "B"),
        round(OTHER_PROMPT, "C"),
    ]);
    assert_eq!(
        verify_prompt(&ledger, &log, OTHER_PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::LedgerConflict)
    );
}

#[test]
fn rounds_of_other_prompts_do_not_count() {
    let mut events = vec![
        user_prompt(1, OTHER_PROMPT),
        stop_start(2, OTHER_PROMPT, 1),
        stop_complete(3, OTHER_PROMPT, 1, 0, 0),
    ];
    events.extend(continued_turn());
    let log = rounds(&[
        round(OTHER_PROMPT, "earlier answer"),
        round(PROMPT, "A"),
        round(PROMPT, "B"),
    ]);
    let ledger = ledger(events.into_iter().chain((4..10).map(other)));
    assert_eq!(
        verified_text(verify_prompt(&ledger, &log, PROMPT, &accepted())),
        "B"
    );
    assert_eq!(
        verified_text(verify_prompt(&ledger, &log, OTHER_PROMPT, &accepted())),
        "earlier answer"
    );
}

#[test]
fn a_cancelled_hook_round_is_unsettled() {
    let mut events = continued_turn();
    if let Some(hook) = events[3].hook.as_mut() {
        hook.num_success = Some(0);
        hook.num_cancelled = Some(1);
    }
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::UnsettledStopRound)
    );
}

#[test]
fn a_round_with_a_different_hook_count_is_unsettled() {
    let mut events = continued_turn();
    events[5] = stop_start(15, PROMPT, 3);
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::UnsettledStopRound)
    );
}

#[test]
fn a_stop_round_after_the_final_round_is_unknown() {
    let mut events = continued_turn();
    events.push(stop_start(17, PROMPT, 2));
    events.push(stop_complete(18, PROMPT, 2, 0, 0));
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B"), round(PROMPT, "C")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::EventsAfterFinalRound)
    );
}

#[test]
fn a_completion_without_continuation_attributes_is_unknown() {
    // Claude Code before 2.1.280 does not report additional context; a round
    // that may have continued through it cannot be shown to be final.
    let mut events = continued_turn();
    if let Some(hook) = events[6].hook.as_mut() {
        hook.additional_context_chars = None;
    }
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::MissingContinuationAttribute)
    );
}

#[test]
fn a_foreign_session_is_rejected() {
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(
            &ledger(continued_turn()),
            &log,
            PROMPT,
            &["other".to_string()]
        ),
        Verdict::Unknown(UnknownReason::SessionMismatch)
    );
}

#[test]
fn telemetry_from_a_different_session_is_rejected() {
    let mut events = continued_turn();
    events[6].session_id = Some("other-session".to_string());
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "B")]);
    assert_eq!(
        verify_prompt(&ledger(events), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::SessionMismatch)
    );
}

#[test]
fn a_malformed_round_line_makes_every_prompt_unknown() {
    let log = StopRoundLog::parse(&format!(
        "{}\nnot json\n{}\n",
        round(PROMPT, "A"),
        round(PROMPT, "B")
    ));
    assert!(log.malformed);
    assert_eq!(
        verify_prompt(&ledger(continued_turn()), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::RoundLogMalformed)
    );
}

#[test]
fn a_partially_appended_final_line_is_left_for_the_next_read() {
    let log = StopRoundLog::parse(&format!(
        "{}\n{{\"hook_event_name\":\"St",
        round(PROMPT, "A")
    ));
    assert!(!log.malformed);
    assert_eq!(log.rounds.len(), 1);
}

#[test]
fn an_empty_final_response_is_unknown() {
    let log = rounds(&[round(PROMPT, "A"), round(PROMPT, "   ")]);
    assert_eq!(
        verify_prompt(&ledger(continued_turn()), &log, PROMPT, &accepted()),
        Verdict::Unknown(UnknownReason::EmptyResponse)
    );
}

#[test]
fn api_error_interruption_and_control_commands_produce_no_verdict() {
    // StopFailure, an interrupt, and a local slash command run no Stop hooks,
    // so the prompt never has a completed Stop round.
    let ledger = ledger([
        user_prompt(1, PROMPT),
        other(2),
        TelemetryEvent {
            hook: Some(HookExecution {
                hook_event: "StopFailure".to_string(),
                num_hooks: Some(1),
                num_success: Some(1),
                num_blocking: Some(0),
                num_non_blocking_error: Some(0),
                num_cancelled: Some(0),
                additional_context_chars: Some(0),
            }),
            ..event(3, "hook_execution_complete", Some(PROMPT))
        },
    ]);
    assert!(ledger.stop_prompt_ids().is_empty());
    assert_eq!(
        verify_prompt(&ledger, &StopRoundLog::default(), PROMPT, &accepted()),
        Verdict::Pending
    );
}

#[test]
fn hook_cap_override_never_verifies() {
    // Claude ends the turn after the consecutive block cap even though the
    // last round reported a block, so no round is non-continuing.
    let mut events = vec![user_prompt(1, PROMPT)];
    let mut lines = Vec::new();
    for index in 0..9u64 {
        events.push(stop_start(2 + index * 2, PROMPT, 1));
        events.push(stop_complete(3 + index * 2, PROMPT, 1, 1, 0));
        lines.push(round(PROMPT, &format!("blocked {index}")));
    }
    assert_eq!(
        verify_prompt(&ledger(events), &rounds(&lines), PROMPT, &accepted()),
        Verdict::Pending
    );
}

#[test]
fn otlp_json_request_yields_only_claude_event_metadata() {
    let request = serde_json::json!({
        "resourceLogs": [{
            "resource": { "attributes": [] },
            "scopeLogs": [{
                "scope": { "name": "com.anthropic.claude_code.events" },
                "logRecords": [
                    {
                        "body": { "stringValue": "claude_code.hook_execution_complete" },
                        "attributes": [
                            { "key": "event.name", "value": { "stringValue": "hook_execution_complete" } },
                            { "key": "event.sequence", "value": { "intValue": 16 } },
                            { "key": "event.timestamp", "value": { "stringValue": "2026-10-10T07:00:16.000Z" } },
                            { "key": "prompt.id", "value": { "stringValue": PROMPT } },
                            { "key": "session.id", "value": { "stringValue": SESSION } },
                            { "key": "user.email", "value": { "stringValue": "person@example.com" } },
                            { "key": "hook_event", "value": { "stringValue": "Stop" } },
                            { "key": "num_hooks", "value": { "stringValue": "2" } },
                            { "key": "num_success", "value": { "stringValue": "2" } },
                            { "key": "num_blocking", "value": { "stringValue": "0" } },
                            { "key": "num_non_blocking_error", "value": { "stringValue": "0" } },
                            { "key": "num_cancelled", "value": { "stringValue": "0" } },
                            { "key": "additional_context_chars", "value": { "stringValue": "0" } }
                        ]
                    },
                    {
                        "body": { "stringValue": "claude_code.user_prompt" },
                        "attributes": [
                            { "key": "event.name", "value": { "stringValue": "user_prompt" } },
                            { "key": "event.sequence", "value": { "intValue": "10" } },
                            { "key": "prompt", "value": { "stringValue": "<REDACTED>" } }
                        ]
                    },
                    { "body": { "stringValue": "some other library log" } }
                ]
            }]
        }]
    });

    let events = parse_otlp_logs(&request).expect("valid OTLP logs");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], stop_complete(16, PROMPT, 2, 0, 0));
    assert_eq!(events[1].sequence, 10);
    assert_eq!(events[1].name, "user_prompt");
    let stored = serde_json::to_string(&events).expect("serialize");
    assert!(!stored.contains("person@example.com"));
    assert!(!stored.contains("REDACTED"));
}

#[test]
fn otlp_claude_record_without_sequence_rejects_the_request() {
    let request = serde_json::json!({
        "resourceLogs": [{ "scopeLogs": [{ "logRecords": [{
            "body": { "stringValue": "claude_code.api_request" },
            "attributes": [
                { "key": "event.name", "value": { "stringValue": "api_request" } }
            ]
        }]}]}]
    });
    assert_eq!(
        parse_otlp_logs(&request),
        Err(OtlpRejection::MissingSequence)
    );
    assert_eq!(
        parse_otlp_logs(&serde_json::json!({ "resourceSpans": [] })),
        Err(OtlpRejection::NotOtlpLogs)
    );
}

#[test]
fn ledger_lines_round_trip() {
    for event in continued_turn() {
        let line = serde_json::to_string(&event).expect("serialize");
        let parsed: TelemetryEvent = serde_json::from_str(&line).expect("parse");
        assert_eq!(parsed, event);
    }
}
