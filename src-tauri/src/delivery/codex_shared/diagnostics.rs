use serde_json::Value;

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
