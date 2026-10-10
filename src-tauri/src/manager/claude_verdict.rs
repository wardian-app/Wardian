//! Verified Claude turn completions.
//!
//! Claude Code's interactive `Stop` hook runs before Claude decides whether
//! the turn continues: any other Stop hook may still block it. A Stop record
//! alone therefore cannot prove that its `last_assistant_message` is the
//! final answer. The only supported interactive surface that reports the
//! decision is the OpenTelemetry log event `claude_code.hook_execution_complete`
//! for `hook_event = "Stop"`, which Claude emits after every Stop hook of a
//! round has settled. It carries the round's blocking counts and the prompt
//! ID, but not the response text.
//!
//! This module joins the two without inferring identity from equal counts
//! alone. Each launch has its own append-only Stop round log (written by
//! Wardian's Stop hook) and its own telemetry ledger (written by the loopback
//! receiver). A prompt is verified only when:
//!
//! - the ledger holds every event sequence number from the prompt's
//!   `user_prompt` event through the final Stop `hook_execution_complete`;
//!   Claude numbers events per process, so a dropped event is a gap;
//! - every Stop round in that window is a `hook_execution_start` followed by
//!   its `hook_execution_complete`, with every hook settled and none cancelled,
//!   so no hook of an earlier round can append after a later round;
//! - exactly the last round is non-continuing (no blocking decision and no
//!   additional context), and nothing for the prompt follows it;
//! - the round log holds exactly one record per round for the prompt, in
//!   round order, from the same Claude session.
//!
//! Anything else is `Pending` (a gap may still be filled by a retried batch)
//! or `Unknown`. Neither creates an Inbox card.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub(crate) mod launch;
pub(crate) mod receiver;
pub(crate) mod telemetry_config;

#[cfg(test)]
mod tests;

/// Per-launch Stop round log appended by Wardian's Claude Stop hook.
pub(crate) const STOP_ROUNDS_FILE: &str = "stop-rounds.jsonl";
/// Per-launch telemetry ledger appended by the loopback receiver.
pub(crate) const TELEMETRY_FILE: &str = "telemetry.jsonl";
/// Prompt IDs whose verified completion is durably in the Inbox queue.
pub(crate) const COMMITTED_FILE: &str = "committed.jsonl";
/// Launch description, including whether verification is available.
pub(crate) const LAUNCH_FILE: &str = "launch.json";

const EVENT_PREFIX: &str = "claude_code.";
const USER_PROMPT_EVENT: &str = "user_prompt";
const HOOK_START_EVENT: &str = "hook_execution_start";
const HOOK_COMPLETE_EVENT: &str = "hook_execution_complete";
const STOP_HOOK_EVENT: &str = "Stop";

/// Hook execution counts copied from a `hook_execution_*` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HookExecution {
    pub(crate) hook_event: String,
    pub(crate) num_hooks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) num_success: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) num_blocking: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) num_non_blocking_error: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) num_cancelled: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) additional_context_chars: Option<u64>,
}

/// The metadata Wardian keeps from one Claude telemetry log record. Prompt
/// text, account attributes, and every other attribute are discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TelemetryEvent {
    pub(crate) sequence: u64,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hook: Option<HookExecution>,
}

impl TelemetryEvent {
    fn is_stop_hook(&self, name: &str) -> bool {
        self.name == name
            && self
                .hook
                .as_ref()
                .is_some_and(|hook| hook.hook_event == STOP_HOOK_EVENT)
    }

    fn belongs_to(&self, prompt_id: &str) -> bool {
        self.prompt_id.as_deref() == Some(prompt_id)
    }
}

/// Why an OTLP request body was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OtlpRejection {
    NotOtlpLogs,
    MissingSequence,
    NameMismatch,
}

/// Extracts Claude Code events from an OTLP/HTTP JSON `ExportLogsServiceRequest`.
///
/// Records whose body is not a `claude_code.*` event are ignored. A Claude
/// record without a usable `event.sequence` rejects the whole request, because
/// its position in the process stream would otherwise be unknowable.
pub(crate) fn parse_otlp_logs(
    request: &serde_json::Value,
) -> Result<Vec<TelemetryEvent>, OtlpRejection> {
    let resource_logs = request
        .get("resourceLogs")
        .and_then(serde_json::Value::as_array)
        .ok_or(OtlpRejection::NotOtlpLogs)?;
    let mut events = Vec::new();
    for resource_log in resource_logs {
        let scope_logs = resource_log
            .get("scopeLogs")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for scope_log in scope_logs {
            let records = scope_log
                .get("logRecords")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for record in records {
                if let Some(event) = parse_log_record(record)? {
                    events.push(event);
                }
            }
        }
    }
    Ok(events)
}

fn parse_log_record(
    record: &serde_json::Value,
) -> Result<Option<TelemetryEvent>, OtlpRejection> {
    let Some(body_name) = record
        .get("body")
        .and_then(|body| body.get("stringValue"))
        .and_then(serde_json::Value::as_str)
        .and_then(|body| body.strip_prefix(EVENT_PREFIX))
    else {
        return Ok(None);
    };
    let attributes = OtlpAttributes::new(record);
    let sequence = attributes
        .unsigned("event.sequence")
        .ok_or(OtlpRejection::MissingSequence)?;
    if attributes.string("event.name").as_deref() != Some(body_name) {
        return Err(OtlpRejection::NameMismatch);
    }
    let hook = matches!(body_name, HOOK_START_EVENT | HOOK_COMPLETE_EVENT)
        .then(|| {
            attributes.string("hook_event").map(|hook_event| HookExecution {
                hook_event,
                num_hooks: attributes.unsigned("num_hooks"),
                num_success: attributes.unsigned("num_success"),
                num_blocking: attributes.unsigned("num_blocking"),
                num_non_blocking_error: attributes.unsigned("num_non_blocking_error"),
                num_cancelled: attributes.unsigned("num_cancelled"),
                additional_context_chars: attributes.unsigned("additional_context_chars"),
            })
        })
        .flatten();
    Ok(Some(TelemetryEvent {
        sequence,
        name: body_name.to_string(),
        prompt_id: attributes.string("prompt.id"),
        session_id: attributes.string("session.id"),
        timestamp: attributes.string("event.timestamp"),
        hook,
    }))
}

struct OtlpAttributes<'a>(&'a [serde_json::Value]);

impl<'a> OtlpAttributes<'a> {
    fn new(record: &'a serde_json::Value) -> Self {
        Self(
            record
                .get("attributes")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        )
    }

    fn value(&self, key: &str) -> Option<&'a serde_json::Value> {
        self.0
            .iter()
            .find(|attribute| attribute.get("key").and_then(serde_json::Value::as_str) == Some(key))
            .and_then(|attribute| attribute.get("value"))
    }

    fn string(&self, key: &str) -> Option<String> {
        self.value(key)?
            .get("stringValue")?
            .as_str()
            .map(str::to_string)
    }

    /// OTLP JSON encodes 64-bit integers either as numbers or as decimal
    /// strings; Claude also reports its hook counts as string attributes.
    fn unsigned(&self, key: &str) -> Option<u64> {
        let value = self.value(key)?;
        if let Some(int_value) = value.get("intValue") {
            return int_value
                .as_u64()
                .or_else(|| int_value.as_str().and_then(|text| text.parse().ok()));
        }
        value
            .get("stringValue")?
            .as_str()?
            .parse()
            .ok()
    }
}

/// Outcome of adding one event to a [`TelemetryLedger`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LedgerInsert {
    New,
    Duplicate,
    Conflict,
}

/// One launch's Claude telemetry, keyed by the per-process event sequence.
#[derive(Debug, Default)]
pub(crate) struct TelemetryLedger {
    events: BTreeMap<u64, TelemetryEvent>,
    conflicted: bool,
}

impl TelemetryLedger {
    /// Adds an event. An exporter retry may resend a batch, so an identical
    /// event is a duplicate; a different event at the same sequence means the
    /// stream is not one process's stream, and nothing in it can be trusted.
    pub(crate) fn insert(&mut self, event: TelemetryEvent) -> LedgerInsert {
        match self.events.get(&event.sequence) {
            Some(existing) if *existing == event => LedgerInsert::Duplicate,
            Some(_) => {
                self.conflicted = true;
                LedgerInsert::Conflict
            }
            None => {
                self.events.insert(event.sequence, event);
                LedgerInsert::New
            }
        }
    }

    pub(crate) fn mark_conflicted(&mut self) {
        self.conflicted = true;
    }

    fn is_contiguous(&self, first: u64, last: u64) -> bool {
        first <= last && self.events.range(first..=last).count() as u64 == last - first + 1
    }

    fn events(&self) -> impl Iterator<Item = &TelemetryEvent> {
        self.events.values()
    }

    /// Prompts with at least one completed Stop round in this launch.
    pub(crate) fn stop_prompt_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.events()
            .filter(|event| event.is_stop_hook(HOOK_COMPLETE_EVENT))
            .filter_map(|event| event.prompt_id.clone())
            .filter(|prompt_id| seen.insert(prompt_id.clone()))
            .collect()
    }
}

/// One Stop hook invocation as Claude passed it to Wardian's hook.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct StopRound {
    pub(crate) hook_event_name: String,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) prompt_id: Option<String>,
    #[serde(default)]
    pub(crate) last_assistant_message: Option<String>,
}

/// The parsed per-launch Stop round log, in append order.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StopRoundLog {
    pub(crate) rounds: Vec<StopRound>,
    /// A complete line that is not a Stop record. Its prompt is unknowable,
    /// so no prompt in this launch can be shown to have a complete round set.
    pub(crate) malformed: bool,
}

impl StopRoundLog {
    /// Parses complete lines only. A final line without its newline is still
    /// being appended and is left for the next read.
    pub(crate) fn parse(contents: &str) -> Self {
        let mut log = Self::default();
        let complete = match contents.rfind('\n') {
            Some(end) => &contents[..end],
            None => return log,
        };
        for line in complete.split('\n') {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<StopRound>(line) {
                Ok(round) if round.hook_event_name == STOP_HOOK_EVENT => log.rounds.push(round),
                _ => log.malformed = true,
            }
        }
        log
    }
}

/// A provider-verified final answer for one Claude prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedCompletion {
    pub(crate) prompt_id: String,
    pub(crate) response_text: String,
    /// `event.timestamp` of the Stop round's `hook_execution_complete`.
    pub(crate) completed_at: Option<String>,
}

/// Why a prompt with a non-continuing Stop round cannot be verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    LedgerConflict,
    RoundLogMalformed,
    MissingPromptStart,
    DuplicatePromptStart,
    StopRoundBeforePromptStart,
    UnpairedStopRound,
    UnsettledStopRound,
    MissingContinuationAttribute,
    EventsAfterFinalRound,
    RoundCountMismatch,
    SessionMismatch,
    EmptyResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Verified(VerifiedCompletion),
    /// No non-continuing round yet, or a gap a retried batch may still fill.
    Pending,
    Unknown(UnknownReason),
}

/// Whether a completed Stop round let Claude continue the turn. `None`
/// means the event lacks an attribute required to tell.
fn round_continues(hook: &HookExecution) -> Option<bool> {
    if hook.num_blocking? > 0 {
        return Some(true);
    }
    Some(hook.additional_context_chars? > 0)
}

/// Every hook in the round reached an outcome and none was cancelled, so no
/// hook process of this round is still able to write.
fn round_settled(start: &HookExecution, complete: &HookExecution) -> bool {
    let (Some(started), Some(hooks)) = (start.num_hooks, complete.num_hooks) else {
        return false;
    };
    let settled = [
        complete.num_success,
        complete.num_blocking,
        complete.num_non_blocking_error,
    ]
    .into_iter()
    .try_fold(0u64, |total, count| count.map(|count| total + count));
    started == hooks && complete.num_cancelled == Some(0) && settled == Some(hooks)
}

/// Decides whether `prompt_id` has a provider-verified final answer.
pub(crate) fn verify_prompt(
    ledger: &TelemetryLedger,
    round_log: &StopRoundLog,
    prompt_id: &str,
    accepted_sessions: &[String],
) -> Verdict {
    if ledger.conflicted {
        return Verdict::Unknown(UnknownReason::LedgerConflict);
    }

    let stop_events = ledger
        .events()
        .filter(|event| {
            event.belongs_to(prompt_id)
                && (event.is_stop_hook(HOOK_START_EVENT) || event.is_stop_hook(HOOK_COMPLETE_EVENT))
        })
        .collect::<Vec<_>>();
    let mut final_round = None;
    for event in stop_events
        .iter()
        .filter(|event| event.name == HOOK_COMPLETE_EVENT)
    {
        match event.hook.as_ref().and_then(round_continues) {
            Some(true) => {}
            Some(false) => {
                final_round = Some(*event);
                break;
            }
            None => return Verdict::Unknown(UnknownReason::MissingContinuationAttribute),
        }
    }
    let Some(final_round) = final_round else {
        return Verdict::Pending;
    };

    let mut starts = ledger
        .events()
        .filter(|event| event.name == USER_PROMPT_EVENT && event.belongs_to(prompt_id));
    let Some(prompt_start) = starts.next() else {
        return Verdict::Unknown(UnknownReason::MissingPromptStart);
    };
    if starts.next().is_some() {
        return Verdict::Unknown(UnknownReason::DuplicatePromptStart);
    }
    if stop_events
        .iter()
        .any(|event| event.sequence < prompt_start.sequence)
    {
        return Verdict::Unknown(UnknownReason::StopRoundBeforePromptStart);
    }
    if !ledger.is_contiguous(prompt_start.sequence, final_round.sequence) {
        return Verdict::Pending;
    }
    if stop_events
        .iter()
        .any(|event| event.sequence > final_round.sequence)
    {
        return Verdict::Unknown(UnknownReason::EventsAfterFinalRound);
    }

    let mut rounds = 0usize;
    let mut open_start: Option<&HookExecution> = None;
    for event in &stop_events {
        let Some(hook) = event.hook.as_ref() else {
            return Verdict::Unknown(UnknownReason::UnpairedStopRound);
        };
        match (event.name.as_str(), open_start.take()) {
            (HOOK_START_EVENT, None) => open_start = Some(hook),
            (HOOK_COMPLETE_EVENT, Some(start)) => {
                if !round_settled(start, hook) {
                    return Verdict::Unknown(UnknownReason::UnsettledStopRound);
                }
                rounds += 1;
            }
            _ => return Verdict::Unknown(UnknownReason::UnpairedStopRound),
        }
    }

    if round_log.malformed {
        return Verdict::Unknown(UnknownReason::RoundLogMalformed);
    }
    let prompt_rounds = round_log
        .rounds
        .iter()
        .filter(|round| round.prompt_id.as_deref().map(str::trim) == Some(prompt_id))
        .collect::<Vec<_>>();
    if prompt_rounds.len() != rounds {
        return Verdict::Unknown(UnknownReason::RoundCountMismatch);
    }
    let Some(last_round) = prompt_rounds.last() else {
        return Verdict::Unknown(UnknownReason::RoundCountMismatch);
    };

    let Some(session_id) = last_round.session_id.as_deref() else {
        return Verdict::Unknown(UnknownReason::SessionMismatch);
    };
    let same_session = |value: Option<&str>| value == Some(session_id);
    if !accepted_sessions.iter().any(|accepted| accepted == session_id)
        || !prompt_rounds
            .iter()
            .all(|round| same_session(round.session_id.as_deref()))
        || !same_session(final_round.session_id.as_deref())
        || !same_session(prompt_start.session_id.as_deref())
    {
        return Verdict::Unknown(UnknownReason::SessionMismatch);
    }

    let Some(response_text) = last_round
        .last_assistant_message
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    else {
        return Verdict::Unknown(UnknownReason::EmptyResponse);
    };

    Verdict::Verified(VerifiedCompletion {
        prompt_id: prompt_id.to_string(),
        response_text: response_text.to_string(),
        completed_at: final_round.timestamp.clone(),
    })
}
