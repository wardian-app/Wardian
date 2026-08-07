//! Protocol layer for Prime Agent's `--mode rpc` channel.
//!
//! RPC mode is a bidirectional JSONL stream over the child's stdin and stdout.
//! Unlike the TUI, it accepts structured commands, which means Wardian can
//! deliver a prompt, steer an in-flight turn, or abort without emulating
//! keystrokes and without depending on terminal markers.
//!
//! This module is deliberately transport-free: it encodes commands, decodes
//! lines, and classifies events. Owning the child process is left to the
//! caller, so the protocol can be tested without spawning anything.

use wardian_core::control::{MessageInputMode, QueuePolicy};

/// Frames outgoing JSONL commands for Prime's reader.
///
/// Prime splits its input on `\n` and strips one trailing `\r`, so a command
/// must occupy exactly one line. `serde_json` escapes control characters inside
/// strings, which keeps multi-line prompts on a single physical line, but this
/// checks the invariant rather than assuming it: a raw newline would silently
/// split one command into two unparseable fragments.
pub fn encode_command(command: &serde_json::Value) -> Result<String, String> {
    let encoded = serde_json::to_string(command)
        .map_err(|error| format!("could not encode Prime RPC command: {error}"))?;
    if encoded.contains('\n') || encoded.contains('\r') {
        return Err("Prime RPC command contained a raw newline".to_string());
    }

    Ok(format!("{encoded}\n"))
}

/// Incremental decoder for Prime's JSONL output.
///
/// Mirrors Prime's own `attachJsonlLineReader`: split on `\n`, tolerate a
/// trailing `\r`. A partial line is held until its terminator arrives, which
/// matters because a pipe read can land mid-event.
#[derive(Debug, Default)]
pub struct JsonlFramer {
    pending: String,
}

impl JsonlFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a chunk and returns every complete line it finished.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);

        let mut lines = Vec::new();
        while let Some(index) = self.pending.find('\n') {
            let mut line = self.pending[..index].to_string();
            self.pending.drain(..=index);
            if line.ends_with('\r') {
                line.pop();
            }
            lines.push(line);
        }

        lines
    }

    /// Whatever is buffered without a terminator, for end-of-stream handling.
    pub fn take_remainder(&mut self) -> Option<String> {
        let remainder = std::mem::take(&mut self.pending);
        let trimmed = remainder.trim_end_matches('\r');
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

/// How a Wardian prompt should reach a Prime session.
///
/// Prime handles the busy case itself, so Wardian chooses a scheduling
/// preference rather than holding the message in a local buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDelivery {
    /// Land the message in the running turn, or start one if idle.
    Steer,
    /// Run the message after the current turn, or immediately if idle.
    FollowUp,
    /// The caller asked for mailbox delivery, which is not an RPC send.
    Withhold,
}

impl PromptDelivery {
    /// The `streamingBehavior` value Prime expects, which is camelCase and
    /// not the same spelling as the standalone `follow_up` command.
    fn streaming_behavior(self) -> Option<&'static str> {
        match self {
            Self::Steer => Some("steer"),
            Self::FollowUp => Some("followUp"),
            Self::Withhold => None,
        }
    }
}

/// Chooses how a prompt should be scheduled.
///
/// Deliberately does not consider whether the session is currently streaming.
/// A bare `prompt` is rejected outright while a turn is running -- observed
/// live as `Agent is already processing. Specify streamingBehavior ('steer' or
/// 'followUp') to queue the message.` -- and any streaming flag Wardian reads
/// can go stale between the check and the write. Prime ignores the preference
/// when the session is idle, so always sending one removes the race instead of
/// narrowing it.
pub fn prompt_delivery(input_mode: MessageInputMode, queue_policy: QueuePolicy) -> PromptDelivery {
    match queue_policy {
        QueuePolicy::MailboxOnly => PromptDelivery::Withhold,
        // The caller wants this message in the live turn, which is steering.
        QueuePolicy::LiveOnly => PromptDelivery::Steer,
        // An approval answers something the agent is already blocked on, so
        // deferring it would deadlock the turn it belongs to.
        QueuePolicy::QueueIfBusy if matches!(input_mode, MessageInputMode::ApprovalAction) => {
            PromptDelivery::Steer
        }
        QueuePolicy::QueueIfBusy => PromptDelivery::FollowUp,
    }
}

/// Builds the `prompt` command, or `None` when nothing is sent.
///
/// One command type covers both the idle and busy cases: `_promptInjectedMessage`
/// only consults `streamingBehavior` when work is already queued, and starts
/// the turn immediately otherwise.
pub fn prompt_command(delivery: PromptDelivery, message: &str) -> Option<serde_json::Value> {
    let streaming_behavior = delivery.streaming_behavior()?;

    Some(serde_json::json!({
        "type": "prompt",
        "message": message,
        "streamingBehavior": streaming_behavior,
    }))
}

/// `abort`, which cancels the running turn without ending the session.
pub fn abort_command() -> serde_json::Value {
    serde_json::json!({ "type": "abort" })
}

/// `set_model`, which takes the provider and model id as separate fields
/// rather than the composite `provider/id` string the CLI flag accepts.
pub fn set_model_command(model_reference: &str) -> Result<serde_json::Value, String> {
    let (provider, model_id) = model_reference
        .trim()
        .split_once('/')
        .ok_or_else(|| format!("Prime model reference must be provider/id: {model_reference}"))?;
    if provider.is_empty() || model_id.is_empty() {
        return Err(format!(
            "Prime model reference must be provider/id: {model_reference}"
        ));
    }

    // The model id keeps any further slashes: prime-inference publishes ids
    // such as `anthropic/claude-opus-5`, so only the first segment is the
    // provider.
    Ok(serde_json::json!({
        "type": "set_model",
        "provider": provider,
        "modelId": model_id,
    }))
}

/// `set_thinking_level`.
pub fn set_thinking_level_command(level: &str) -> serde_json::Value {
    serde_json::json!({ "type": "set_thinking_level", "level": level.trim() })
}

/// `compact`, optionally with custom instructions.
pub fn compact_command(custom_instructions: Option<&str>) -> serde_json::Value {
    match custom_instructions.map(str::trim).filter(|s| !s.is_empty()) {
        Some(instructions) => {
            serde_json::json!({ "type": "compact", "customInstructions": instructions })
        }
        None => serde_json::json!({ "type": "compact" }),
    }
}

/// `observe` / `unobserve`, which stream another session's events into this
/// connection. This is how a subagent tree becomes visible to Wardian.
pub fn observe_command(active_session_id: &str, observe: bool) -> serde_json::Value {
    serde_json::json!({
        "type": if observe { "observe" } else { "unobserve" },
        "activeSessionId": active_session_id.trim(),
    })
}

/// The reply to an `extension_ui_request`.
///
/// The three response shapes are not interchangeable: Prime reads `confirmed`
/// for a confirm dialog and `value` for select and input, and treats a missing
/// field as a negative answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionUiAnswer {
    Cancelled,
    Confirmed(bool),
    Value(String),
}

pub fn extension_ui_response_command(id: &str, answer: &ExtensionUiAnswer) -> serde_json::Value {
    let mut response = serde_json::json!({ "type": "extension_ui_response", "id": id });
    let object = response
        .as_object_mut()
        .expect("literal object is always an object");

    match answer {
        ExtensionUiAnswer::Cancelled => {
            object.insert("cancelled".to_string(), serde_json::Value::Bool(true));
        }
        ExtensionUiAnswer::Confirmed(confirmed) => {
            object.insert("confirmed".to_string(), serde_json::Value::Bool(*confirmed));
        }
        ExtensionUiAnswer::Value(value) => {
            object.insert(
                "value".to_string(),
                serde_json::Value::String(value.clone()),
            );
        }
    }

    response
}

/// A request from a Prime extension for user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionUiRequest {
    pub id: String,
    pub method: String,
    pub title: Option<String>,
    pub message: Option<String>,
    pub options: Vec<String>,
}

impl ExtensionUiRequest {
    /// True when the extension is waiting for an answer before it can continue.
    ///
    /// Only `select`, `confirm`, and `input` block. `notify`, `setStatus`, and
    /// the widget methods are fire-and-forget in
    /// `rpc-extension-ui-context.js`, so treating them as action-required
    /// would strand an agent that is not actually waiting for anything.
    pub fn blocks_the_agent(&self) -> bool {
        matches!(self.method.as_str(), "select" | "confirm" | "input")
    }

    /// The text to show the user, falling back through the fields each dialog
    /// method actually populates.
    pub fn prompt_text(&self) -> String {
        self.title
            .as_deref()
            .or(self.message.as_deref())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Prime Agent requested {}", self.method))
    }
}

/// Parses an `extension_ui_request` line, ignoring every other event.
pub fn parse_extension_ui_request(line: &str) -> Option<ExtensionUiRequest> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    if parsed.get("type")?.as_str()? != "extension_ui_request" {
        return None;
    }

    let string_field = |key: &str| {
        parsed
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
    };

    Some(ExtensionUiRequest {
        id: string_field("id")?,
        method: string_field("method")?,
        title: string_field("title"),
        message: string_field("message"),
        options: parsed
            .get("options")
            .and_then(|value| value.as_array())
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        option.as_str().map(str::to_string).or_else(|| {
                            option
                                .get("label")
                                .and_then(|label| label.as_str())
                                .map(str::to_string)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// An event belonging to a session other than the connection's own.
///
/// `observe` wraps a watched session's stream rather than merging it, so a
/// subagent's events stay attributable. Wardian projects these as read-only
/// nested activity under the root rather than as agents of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedSession {
    /// One event from the observed session, already unwrapped.
    Event {
        active_session_id: String,
        event: String,
    },
    /// The observed session ended; `error` is set when it ended badly.
    Closed {
        active_session_id: String,
        error: Option<String>,
    },
}

impl ObservedSession {
    pub fn active_session_id(&self) -> &str {
        match self {
            Self::Event {
                active_session_id, ..
            }
            | Self::Closed {
                active_session_id, ..
            } => active_session_id,
        }
    }
}

/// Parses an observation envelope, returning `None` for the connection's own
/// events.
///
/// The inner event is re-serialized rather than returned as a `Value` so it
/// can be fed straight back through the same provider parsing that handles a
/// root session's stream. A subagent's events have the same shapes.
pub fn parse_observed_session(line: &str) -> Option<ObservedSession> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    let active_session_id = parsed
        .get("activeSessionId")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();

    match parsed.get("type")?.as_str()? {
        "observed_session_event" => Some(ObservedSession::Event {
            active_session_id,
            event: parsed.get("event")?.to_string(),
        }),
        "observed_session_closed" => Some(ObservedSession::Closed {
            active_session_id,
            error: parsed
                .get("error")
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty()),
        }),
        _ => None,
    }
}

/// The outcome of a command Wardian sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Succeeded { command: String },
    Failed { command: String, error: String },
}

/// Parses the acknowledgement Prime returns for each command.
///
/// A command that fails is answered rather than dropped, so ignoring these
/// would leave Wardian reporting a delivery that never happened.
pub fn parse_command_outcome(line: &str) -> Option<CommandOutcome> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    let command = parsed.get("command")?.as_str()?.to_string();

    match parsed.get("success").and_then(|value| value.as_bool())? {
        true => Some(CommandOutcome::Succeeded { command }),
        false => Some(CommandOutcome::Failed {
            command,
            error: parsed
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("Prime Agent rejected the command")
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_prompt_stays_on_one_wire_line() {
        let command = prompt_command(PromptDelivery::FollowUp, "first\nsecond\r\nthird")
            .expect("prompt command");

        let encoded = encode_command(&command).expect("encode");

        // Exactly one terminator, at the end: Prime splits on \n, so an
        // unescaped newline would arrive as two broken commands.
        assert_eq!(encoded.matches('\n').count(), 1);
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("first\\nsecond"));
    }

    #[test]
    fn framer_reassembles_events_split_across_reads() {
        let mut framer = JsonlFramer::new();

        // A pipe read can land mid-event; nothing may be emitted until the
        // terminator arrives.
        assert!(framer.push("{\"type\":\"tur").is_empty());
        assert_eq!(
            framer.push("n_start\"}\n{\"type\":\"agent_end\"}\n"),
            vec![r#"{"type":"turn_start"}"#, r#"{"type":"agent_end"}"#]
        );
        assert_eq!(framer.take_remainder(), None);
    }

    #[test]
    fn framer_tolerates_carriage_returns_like_primes_own_reader() {
        let mut framer = JsonlFramer::new();

        assert_eq!(
            framer.push("{\"a\":1}\r\n{\"b\":2}\n"),
            vec![r#"{"a":1}"#, r#"{"b":2}"#]
        );
    }

    #[test]
    fn framer_surfaces_a_final_line_with_no_terminator() {
        let mut framer = JsonlFramer::new();

        assert!(framer.push("{\"type\":\"agent_end\"}").is_empty());
        assert_eq!(
            framer.take_remainder().as_deref(),
            Some(r#"{"type":"agent_end"}"#)
        );
        assert_eq!(framer.take_remainder(), None);
    }

    #[test]
    fn queue_policy_chooses_the_scheduling_preference() {
        // Prime holds the message instead of Wardian holding it.
        assert_eq!(
            prompt_delivery(MessageInputMode::Message, QueuePolicy::QueueIfBusy),
            PromptDelivery::FollowUp
        );
        // The caller asked for live delivery, which is what steering is.
        assert_eq!(
            prompt_delivery(MessageInputMode::Message, QueuePolicy::LiveOnly),
            PromptDelivery::Steer
        );
    }

    #[test]
    fn an_approval_answer_always_reaches_the_running_turn() {
        assert_eq!(
            prompt_delivery(MessageInputMode::ApprovalAction, QueuePolicy::QueueIfBusy),
            PromptDelivery::Steer
        );
    }

    #[test]
    fn every_prompt_carries_a_streaming_behavior() {
        // A bare prompt is rejected outright while a turn is running, and the
        // session can start streaming between any check and this write, so the
        // field is never omitted.
        for policy in [QueuePolicy::QueueIfBusy, QueuePolicy::LiveOnly] {
            let delivery = prompt_delivery(MessageInputMode::Message, policy);
            let command = prompt_command(delivery, "hello").expect("prompt command");

            assert_eq!(command["type"], "prompt");
            assert!(command
                .get("streamingBehavior")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value == "steer" || value == "followUp"));
        }
    }

    #[test]
    fn follow_up_uses_primes_camel_case_spelling() {
        // The standalone command is follow_up, but the scheduling field is
        // followUp; sending the snake_case form silently loses the preference.
        let command = prompt_command(PromptDelivery::FollowUp, "hello").expect("prompt command");

        assert_eq!(command["streamingBehavior"], "followUp");
    }

    #[test]
    fn mailbox_delivery_never_sends_over_rpc() {
        assert_eq!(
            prompt_delivery(MessageInputMode::Message, QueuePolicy::MailboxOnly),
            PromptDelivery::Withhold
        );
        assert_eq!(prompt_command(PromptDelivery::Withhold, "hello"), None);
    }

    #[test]
    fn set_model_splits_only_the_provider_off_the_reference() {
        // prime-inference model ids contain slashes of their own, so a naive
        // split would send the wrong model.
        let command = set_model_command("prime-inference/anthropic/claude-opus-5").expect("split");

        assert_eq!(command["provider"], "prime-inference");
        assert_eq!(command["modelId"], "anthropic/claude-opus-5");
    }

    #[test]
    fn set_model_rejects_a_reference_with_no_provider() {
        assert!(set_model_command("gpt-5.3-codex-spark").is_err());
        assert!(set_model_command("/claude-opus-5").is_err());
        assert!(set_model_command("openai-codex/").is_err());
    }

    #[test]
    fn compact_omits_instructions_when_none_are_given() {
        assert_eq!(compact_command(None)["type"], "compact");
        assert!(compact_command(None).get("customInstructions").is_none());
        assert_eq!(
            compact_command(Some("  keep the plan  "))["customInstructions"],
            "keep the plan"
        );
        // Whitespace is not an instruction.
        assert!(compact_command(Some("   "))
            .get("customInstructions")
            .is_none());
    }

    #[test]
    fn observation_targets_a_named_session() {
        assert_eq!(
            observe_command("a1b2c3", true),
            serde_json::json!({ "type": "observe", "activeSessionId": "a1b2c3" })
        );
        assert_eq!(observe_command("a1b2c3", false)["type"], "unobserve");
    }

    #[test]
    fn only_blocking_dialogs_require_user_action() {
        let blocking = [
            r#"{"type":"extension_ui_request","id":"1","method":"confirm","title":"Overwrite?"}"#,
            r#"{"type":"extension_ui_request","id":"2","method":"select","title":"Pick one","options":["a","b"]}"#,
            r#"{"type":"extension_ui_request","id":"3","method":"input","title":"Branch name"}"#,
        ];
        for line in blocking {
            let request = parse_extension_ui_request(line).expect("request");
            assert!(
                request.blocks_the_agent(),
                "{} should block",
                request.method
            );
        }

        // Fire-and-forget in rpc-extension-ui-context.js: nothing is waiting
        // on an answer, so flagging these would strand a working agent.
        for line in [
            r#"{"type":"extension_ui_request","id":"4","method":"notify","message":"done"}"#,
            r#"{"type":"extension_ui_request","id":"5","method":"setStatus","statusKey":"k"}"#,
        ] {
            let request = parse_extension_ui_request(line).expect("request");
            assert!(!request.blocks_the_agent());
        }
    }

    #[test]
    fn select_options_survive_both_shapes() {
        let request = parse_extension_ui_request(
            r#"{"type":"extension_ui_request","id":"1","method":"select","title":"Pick","options":["plain",{"label":"labelled"}]}"#,
        )
        .expect("request");

        assert_eq!(request.options, vec!["plain", "labelled"]);
        assert_eq!(request.prompt_text(), "Pick");
    }

    #[test]
    fn a_request_without_a_title_still_reads_as_something() {
        let request = parse_extension_ui_request(
            r#"{"type":"extension_ui_request","id":"1","method":"confirm","message":"Proceed?"}"#,
        )
        .expect("request");
        assert_eq!(request.prompt_text(), "Proceed?");

        let bare = parse_extension_ui_request(
            r#"{"type":"extension_ui_request","id":"1","method":"input"}"#,
        )
        .expect("request");
        assert_eq!(bare.prompt_text(), "Prime Agent requested input");
    }

    #[test]
    fn other_events_are_not_mistaken_for_ui_requests() {
        assert!(parse_extension_ui_request(r#"{"type":"turn_start"}"#).is_none());
        assert!(parse_extension_ui_request("not json").is_none());
        // A request with no id cannot be answered, so it is not usable.
        assert!(parse_extension_ui_request(
            r#"{"type":"extension_ui_request","method":"confirm"}"#
        )
        .is_none());
    }

    #[test]
    fn each_answer_uses_the_field_prime_reads_for_it() {
        assert_eq!(
            extension_ui_response_command("1", &ExtensionUiAnswer::Confirmed(true)),
            serde_json::json!({"type":"extension_ui_response","id":"1","confirmed":true})
        );
        assert_eq!(
            extension_ui_response_command("2", &ExtensionUiAnswer::Value("main".into())),
            serde_json::json!({"type":"extension_ui_response","id":"2","value":"main"})
        );
        assert_eq!(
            extension_ui_response_command("3", &ExtensionUiAnswer::Cancelled),
            serde_json::json!({"type":"extension_ui_response","id":"3","cancelled":true})
        );
    }

    #[test]
    fn an_observed_event_keeps_its_session_and_unwraps_cleanly() {
        let observed = parse_observed_session(
            r#"{"type":"observed_session_event","activeSessionId":"child-1","event":{"type":"turn_start"}}"#,
        )
        .expect("observed event");

        assert_eq!(observed.active_session_id(), "child-1");
        // The inner event must survive intact so the same provider parsing
        // that handles a root's stream can handle a subagent's.
        let ObservedSession::Event { event, .. } = &observed else {
            panic!("expected an event");
        };
        use wardian_core::models::provider::{AgentEvent, AgentProvider};
        assert_eq!(
            crate::providers::PrimeProvider::new().parse_output(event),
            Some(AgentEvent::UserQuery)
        );
    }

    #[test]
    fn an_observed_close_distinguishes_clean_from_failed() {
        assert_eq!(
            parse_observed_session(
                r#"{"type":"observed_session_closed","activeSessionId":"child-1"}"#
            ),
            Some(ObservedSession::Closed {
                active_session_id: "child-1".to_string(),
                error: None
            })
        );
        assert_eq!(
            parse_observed_session(
                r#"{"type":"observed_session_closed","activeSessionId":"child-1","error":"worker died"}"#
            ),
            Some(ObservedSession::Closed {
                active_session_id: "child-1".to_string(),
                error: Some("worker died".to_string())
            })
        );
    }

    #[test]
    fn the_connections_own_events_are_not_observations() {
        // Without this the root's own stream would be projected as a subagent.
        assert!(parse_observed_session(r#"{"type":"turn_start"}"#).is_none());
        assert!(parse_observed_session(
            r#"{"id":"1","type":"response","command":"observe","success":true}"#
        )
        .is_none());
        // An observation with no session cannot be attributed to anything.
        assert!(parse_observed_session(
            r#"{"type":"observed_session_event","event":{"type":"turn_start"}}"#
        )
        .is_none());
    }

    #[test]
    fn command_failures_are_reported_rather_than_dropped() {
        // Captured verbatim from a live `prime-agent --mode rpc` session.
        assert_eq!(
            parse_command_outcome(
                r#"{"id":"r1","type":"response","command":"prompt","success":true}"#
            ),
            Some(CommandOutcome::Succeeded {
                command: "prompt".to_string()
            })
        );
        assert_eq!(
            parse_command_outcome(
                r#"{"id":"1","command":"steer","success":false,"error":"No active turn"}"#
            ),
            Some(CommandOutcome::Failed {
                command: "steer".to_string(),
                error: "No active turn".to_string()
            })
        );
        // Stream events carry no `command`, so they are not acknowledgements.
        assert!(parse_command_outcome(r#"{"type":"turn_start"}"#).is_none());
    }
}
