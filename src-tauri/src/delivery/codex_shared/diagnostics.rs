use serde_json::Value;

/// Recognize only the 0.159.2-proven rejection forms that establish that an
/// exact-turn steer did not submit model input. Unknown errors remain uncertain.
pub(super) fn stale_steer_rejection(method: &str, params: &Value, error: &Value) -> bool {
    if method != "turn/steer" || error["code"].as_i64() != Some(-32600) {
        return false;
    }
    let Some(expected) = params["expectedTurnId"].as_str().filter(|id| {
        !id.is_empty()
            && !id
                .chars()
                .any(|character| character == '`' || character.is_control())
    }) else {
        return false;
    };
    let Some(message) = error["message"].as_str() else {
        return false;
    };
    if message == "no active turn to steer" {
        return true;
    }
    let prefix = format!("expected active turn id `{expected}` but found `");
    message
        .strip_prefix(prefix.as_str())
        .and_then(|found| found.strip_suffix('`'))
        .is_some_and(|found| {
            !found.is_empty()
                && found != expected
                && !found
                    .chars()
                    .any(|character| character == '`' || character.is_control())
        })
}

/// Translate a known startup rejection without exposing arbitrary provider text,
/// which can contain prompts, credentials, and local paths.
pub(super) fn rejection_message(method: &str, error: &Value) -> &'static str {
    if method == "thread/resume"
        && error["code"] == -32600
        && error["message"]
            .as_str()
            .is_some_and(|message| message.contains("already has an active writer"))
    {
        "Cannot resume this Codex conversation because another Codex process has it open for writing. Release the conversation in the other Codex app or terminal, then restart this Wardian agent. If that app retains the conversation, quit it after saving other work. Do not delete the session or its writer lock."
    } else if method == "thread/resume"
        && error["code"] == -32601
        && error["message"].as_str() == Some("list_turns is not supported yet")
    {
        "This Codex runtime cannot resume the conversation's paginated history (list_turns is not supported yet). Restarting Wardian or closing another Codex app will not resolve this history compatibility error. The original conversation has been retained; use a compatible Codex runtime or a backed-up history recovery."
    } else {
        "provider rejected the request; original diagnostic remains provider-owned"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stale_steer_rejection_requires_exact_observed_form_and_requested_turn() {
        let params = json!({"expectedTurnId":"expected"});
        for message in [
            "no active turn to steer",
            "expected active turn id `expected` but found `other`",
        ] {
            let error = json!({"code":-32600,"message":message});
            assert!(stale_steer_rejection("turn/steer", &params, &error));
            assert!(!stale_steer_rejection("turn/start", &params, &error));
            assert!(!stale_steer_rejection(
                "thread/inject_items",
                &params,
                &error
            ));
            assert!(!stale_steer_rejection("turn/steer", &json!({}), &error));
            assert!(!stale_steer_rejection(
                "turn/steer",
                &json!({"expectedTurnId":""}),
                &error
            ));
        }
        for error in [
            json!({"code":-32600,"message":"other rejection"}),
            json!({"code":-32600,"message":"no active turn to steer "}),
            json!({"code":-32600,"message":"prefix no active turn to steer"}),
            json!({"code":-32600,"message":"expected active turn id `different` but found `other`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `expected`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found ``"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other` suffix"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other`extra`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other\n`"}),
            json!({"code":-32603,"message":"no active turn to steer"}),
            json!({"code":"-32600","message":"no active turn to steer"}),
            json!({"code":-32600.0,"message":"no active turn to steer"}),
            json!({"code":-32600,"message":null}),
            json!({"code":-32600}),
            Value::Null,
        ] {
            assert!(
                !stale_steer_rejection("turn/steer", &params, &error),
                "{error}"
            );
        }
    }

    #[test]
    fn paginated_history_failure_is_distinct_from_writer_conflict() {
        let error = json!({"code":-32601,"message":"list_turns is not supported yet"});
        let message = rejection_message("thread/resume", &error);
        assert!(message.contains("paginated history"));
        assert!(message.contains("will not resolve"));
        assert!(!rejection_message("turn/start", &error).contains("paginated history"));
    }

    #[test]
    fn writer_conflict_explains_recovery_without_copying_provider_payload() {
        let error = json!({"code":-32600,"message":"thread private-id already has an active writer; secret-payload"});
        let message = rejection_message("thread/resume", &error);
        assert!(message.contains("another Codex process"));
        assert!(message.contains("restart this Wardian agent"));
        assert!(!message.contains("private-id"));
        assert!(!message.contains("secret-payload"));
        for (method, error) in [
            ("turn/start", error),
            (
                "thread/resume",
                json!({"code":-32600,"message":"other rejection secret-payload"}),
            ),
            (
                "thread/resume",
                json!({"code":-32603,"message":"already has an active writer"}),
            ),
            ("thread/resume", Value::Null),
        ] {
            assert_eq!(
                rejection_message(method, &error),
                "provider rejected the request; original diagnostic remains provider-owned"
            );
        }
    }
}
