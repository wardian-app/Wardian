//! Delivers a Wardian message to a Prime Agent worker through its supervisor.
//!
//! Every other provider is driven by writing keystrokes into its PTY, because
//! that is the only input channel a terminal CLI exposes. Prime Agent is not a
//! terminal CLI with a daemon bolted on: the daemon is the agent, and the TUI is
//! one client of it. `prime-agent send` is another, so Wardian can hand a
//! message to the supervisor and be told whether it was accepted.
//!
//! That acknowledgement is the point. Keystroke delivery can only ever report
//! that bytes were written, so a receipt has to be inferred from what the
//! provider paints afterwards. Reading Prime's TUI for that works today and
//! will keep working only until its layout changes; the supervisor answering
//! `success` is a fact about the message.
//!
//! This deliberately stops short of owning Prime's RPC channel. Doing that
//! means launching with `--mode rpc`, which requests a `client_owned` worker:
//! the supervisor hides those from every other client and reaps them with the
//! client that asked for one. Wardian would gain `abort`, `set_model`, and
//! `observe`, and would lose detached workers, `prime-agent list`, `stop`, and
//! everything built on them.

use crate::utils::delivery_transaction::{TerminalDeliveryError, TerminalDeliveryOutcome};

pub struct PrimeSendRequest<'a> {
    /// Anything `matchWorkers` resolves: the session UUID Wardian persists, the
    /// short daemon id, or the session name.
    pub selector: &'a str,
    pub prompt: &'a str,
}

/// Builds the `send` argument list.
///
/// There is nothing to decide. Wardian's queue policy is spent before delivery
/// reaches this layer -- it chooses whether a message waits in the mailbox, and
/// a drained message arrives here already committed, which is why the keystroke
/// path does not consult it either.
///
/// The steer/follow-up distinction cannot be expressed here in any case.
/// `prime-agent help send` advertises `--steer` and `--follow-up`, but
/// `parseSendArgs` in 0.7.0 accepts only `--from`, `--message`, and `--`, and
/// rejects both flags in every position. The daemon schedules the message
/// itself and reports what it did, so [`parse_send_outcome`] reads the answer
/// out of the receipt rather than Wardian dictating it.
///
/// `--message` rather than a positional: the parser space-joins trailing
/// operands and would read a message beginning with `--` as an option.
pub fn send_args(request: &PrimeSendRequest) -> Vec<String> {
    // `--from` is deliberately unused: it takes a Prime agent selector, and a
    // Wardian sender is usually not one. Wardian already prefixes the sending
    // agent into the message text, so the attribution is not lost.
    vec![
        "send".to_string(),
        "--json".to_string(),
        request.selector.trim().to_string(),
        // Must follow the target: the parser rejects it otherwise.
        "--message".to_string(),
        request.prompt.to_string(),
    ]
}

/// Interprets what the supervisor said about a `send`.
///
/// On success `--json` prints the receipt payload itself, not the command
/// envelope the other daemon commands return: an object carrying
/// `deliveryStatus` and the `target` it resolved to. A failure never reaches
/// that point -- `requireSuccess` throws, so the client exits non-zero with a
/// plain-text `Error:` line and prints no JSON at all. Both are checked,
/// because trusting either alone would read one kind of failure as a delivery.
///
/// `queued` counts as accepted. The agent was busy, so the daemon put the
/// message in its queue rather than the running turn; it is durably held and
/// will run. That is reported in `observed_state` rather than hidden, since it
/// is the one thing Wardian asked for and could not dictate.
pub fn parse_send_outcome(
    exit_ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<TerminalDeliveryOutcome, String> {
    let receipt = exit_ok.then(|| delivery_status(stdout)).flatten();

    match receipt.as_deref() {
        Some(status @ ("delivered" | "queued")) => Ok(TerminalDeliveryOutcome {
            delivery_state: "provider_accepted".to_string(),
            delivery_phase: "supervisor_accepted".to_string(),
            observed_state: Some(status.to_string()),
            reason: Some(format!(
                "Prime Agent's supervisor {} the message for this session",
                if status == "queued" {
                    "queued"
                } else {
                    "delivered"
                }
            )),
        }),
        // An unrecognised status is not a receipt Wardian can stand behind.
        Some(status) => Err(format!(
            "Prime Agent reported an unknown delivery status: {status}"
        )),
        None => Err(first_meaningful_line(stderr)
            .or_else(|| first_meaningful_line(stdout))
            .unwrap_or_else(|| "Prime Agent did not acknowledge the message".to_string())),
    }
}

/// Reads `deliveryStatus` out of a `send` receipt, requiring the target too so
/// an unrelated JSON line cannot be read as one.
fn delivery_status(stdout: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    parsed
        .get("target")
        .and_then(|target| target.get("activeSessionId"))
        .and_then(|value| value.as_str())?;
    parsed
        .get("deliveryStatus")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Finds the selector that addresses an agent's Prime worker.
///
/// The bound provider session is the cheap answer and the usual one. The
/// fallback exists because binding depends on Prime having materialized a
/// transcript, and a message can be sent before that: the worker is registered
/// against the session directory Wardian pinned for this agent from the moment
/// it starts. Teardown resolves the same way, synchronously, in
/// `manager::prime_stop_selector_from_daemon`.
pub async fn worker_selector(
    wardian_session_id: &str,
    bound_provider_session: Option<&str>,
) -> Option<String> {
    if let Some(bound) = bound_provider_session
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(bound.to_string());
    }

    let session_dir = crate::providers::prime::session_dir_for_agent(wardian_session_id)?;
    let provider = crate::providers::ProviderFactory::resolve("prime").ok()?;
    let (program, base_args) = provider.get_executable();
    let output = crate::utils::process::new_silent_command(&program)
        .args(base_args)
        .args(crate::providers::PrimeProvider::list_args())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let sessions = crate::providers::PrimeProvider::parse_list_output(&String::from_utf8_lossy(
        &output.stdout,
    ))
    .ok()?;
    crate::providers::PrimeProvider::stop_selector_for_session_dir(&sessions, &session_dir)
}

/// How long a send waits for the agent's worker to appear.
///
/// Matches the readiness budget the keystroke path gives other providers. A
/// message can be queued the moment an agent is created, well before Prime has
/// registered its worker with the supervisor, and that gap is startup latency
/// rather than a delivery failure.
const WORKER_REGISTRATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Waits for a selector that addresses this agent's worker.
pub async fn wait_for_worker_selector(
    wardian_session_id: &str,
    bound_provider_session: Option<&str>,
) -> Option<String> {
    let deadline = std::time::Instant::now() + WORKER_REGISTRATION_TIMEOUT;
    loop {
        if let Some(selector) = worker_selector(wardian_session_id, bound_provider_session).await {
            return Some(selector);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Hands a message to Prime Agent's supervisor and waits for its answer.
pub async fn deliver(
    request: PrimeSendRequest<'_>,
) -> Result<TerminalDeliveryOutcome, TerminalDeliveryError> {
    if request.selector.trim().is_empty() {
        return Err(TerminalDeliveryError {
            phase: "prime_send_unaddressed",
            // Recoverable: the identity watcher binds one shortly after launch.
            message: "Prime Agent session is not bound to a worker yet".to_string(),
            retry_safe: true,
        });
    }

    let provider = crate::providers::ProviderFactory::resolve("prime").map_err(|error| {
        TerminalDeliveryError {
            phase: "prime_send_unavailable",
            message: error,
            retry_safe: false,
        }
    })?;
    let (program, base_args) = provider.get_executable();

    let output = crate::utils::process::new_silent_command(&program)
        .args(base_args)
        .args(send_args(&request))
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| TerminalDeliveryError {
            phase: "prime_send_spawn_failed",
            message: error.to_string(),
            // The supervisor was never reached, so nothing was delivered twice.
            retry_safe: true,
        })?;

    parse_send_outcome(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
    .map_err(|message| TerminalDeliveryError {
        phase: "prime_send_rejected",
        message,
        retry_safe: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real 0.7.0 receipt, trimmed to the fields Wardian reads.
    fn receipt(status: &str) -> String {
        format!(
            r#"{{"id":"msg_1","message":"hi","deliveryStatus":"{status}","target":{{"activeSessionId":"fa480e17f888","sessionId":"019fd736-07ce-71ef-8eb8-27baa0661ced"}}}}"#
        )
    }

    fn request<'a>() -> PrimeSendRequest<'a> {
        PrimeSendRequest {
            selector: "019fd6b4-7150-77bd-8ea5-3f93134d1169",
            prompt: "Reply with exactly OK.",
        }
    }

    #[test]
    fn no_scheduling_flag_is_sent() {
        // prime-agent 0.7.0 advertises --steer and --follow-up on `send` but
        // its parser rejects both in every position, so scheduling is the
        // daemon's call and the command cannot express Wardian's preference.
        let args = send_args(&request());

        assert!(!args
            .iter()
            .any(|arg| arg == "--steer" || arg == "--follow-up"));
    }

    #[test]
    fn the_message_follows_its_target_as_an_option_value() {
        let args = send_args(&request());

        let target = args
            .iter()
            .position(|arg| arg == "019fd6b4-7150-77bd-8ea5-3f93134d1169")
            .expect("target");
        let flag = args
            .iter()
            .position(|arg| arg == "--message")
            .expect("flag");

        // `--message` is rejected before the target is known, and its value has
        // to be the very next argument.
        assert!(flag > target);
        assert_eq!(
            args.get(flag + 1).map(String::as_str),
            Some("Reply with exactly OK."),
        );
    }

    #[test]
    fn a_message_that_looks_like_a_flag_stays_a_message() {
        let mut awkward = request();
        awkward.prompt = "--follow-up is not a flag here";
        let args = send_args(&awkward);

        // The reason for --message over a positional: the parser space-joins
        // trailing operands and would reject this one as an unknown option.
        assert_eq!(
            args.last().map(String::as_str),
            Some("--follow-up is not a flag here"),
        );
    }

    #[test]
    fn a_multi_line_message_is_one_argument() {
        let mut multiline = request();
        multiline.prompt = "first\nsecond";
        let args = send_args(&multiline);

        // Unlike the RPC channel there is no line framing to protect: the
        // message is an argv entry, so newlines need no escaping.
        assert_eq!(args.last().map(String::as_str), Some("first\nsecond"));
    }

    #[test]
    fn a_delivered_receipt_is_an_authoritative_acceptance() {
        let outcome = parse_send_outcome(true, &receipt("delivered"), "").expect("accepted");

        assert_eq!(outcome.delivery_state, "provider_accepted");
        assert_eq!(outcome.delivery_phase, "supervisor_accepted");
        assert_eq!(outcome.observed_state.as_deref(), Some("delivered"));
    }

    #[test]
    fn a_queued_receipt_is_accepted_and_says_so() {
        // The agent was busy. The message is durably held and will run, so
        // this is acceptance, but a caller that wanted the live turn should be
        // able to see that it did not get one.
        let outcome = parse_send_outcome(true, &receipt("queued"), "").expect("accepted");

        assert_eq!(outcome.delivery_state, "provider_accepted");
        assert_eq!(outcome.observed_state.as_deref(), Some("queued"));
    }

    #[test]
    fn an_unresolved_selector_is_not_read_as_delivered() {
        // Observed on prime-agent 0.7.0: a selector matching no worker exits
        // non-zero with a plain-text error and prints no JSON at all, even
        // under --json.
        let error = parse_send_outcome(false, "", "Error: Unknown active session: no-such-agent")
            .expect_err("unresolved selector");

        assert_eq!(error, "Error: Unknown active session: no-such-agent");
    }

    #[test]
    fn a_receipt_from_a_failed_run_is_not_trusted() {
        let error = parse_send_outcome(
            false,
            &receipt("delivered"),
            "the client died before it finished",
        )
        .expect_err("failed run");

        assert_eq!(error, "the client died before it finished");
    }

    #[test]
    fn json_without_a_resolved_target_is_not_a_receipt() {
        // A status with no target names no session, so it cannot be evidence
        // that this agent got the message. The unrecognised payload is echoed
        // rather than swallowed, since it is the only diagnostic there is.
        let error = parse_send_outcome(true, r#"{"deliveryStatus":"delivered"}"#, "")
            .expect_err("not a receipt");

        assert_eq!(error, r#"{"deliveryStatus":"delivered"}"#);
    }

    #[test]
    fn an_unknown_delivery_status_is_not_claimed_as_success() {
        let error = parse_send_outcome(true, &receipt("dropped"), "").expect_err("unknown status");
        assert!(error.contains("dropped"));
    }

    #[test]
    fn silence_is_a_failure_rather_than_an_accepted_message() {
        let error = parse_send_outcome(true, "", "").expect_err("no acknowledgement");
        assert!(error.contains("did not acknowledge"));
    }
}
