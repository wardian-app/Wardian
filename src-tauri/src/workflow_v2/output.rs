//! Pure helpers for turning a headless agent's free-form response into the
//! structured `nodes.<id>.output` value, and a decision's chosen port.

use serde_json::Value;

/// Extract structured output from a worker response. Tries, in order: a trailing
/// fenced ```json block, then the whole response as JSON, else wrap as `{"text": ..}`.
/// `_output_schema` is accepted for future validation; unused parsing-wise in 5a.
pub fn extract_structured_output(response: &str, _output_schema: Option<&str>) -> Value {
    if let Some(block) = last_json_block(response) {
        if let Ok(value) = serde_json::from_str::<Value>(&block) {
            return value;
        }
    }

    let trimmed = response.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return value;
    }

    serde_json::json!({ "text": response })
}

/// Find the contents of the last ```json ... ``` fenced block, if any.
fn last_json_block(s: &str) -> Option<String> {
    let mut out = None;
    let mut rest = s;

    while let Some(start) = rest.find("```json") {
        let after = &rest[start + "```json".len()..];
        if let Some(end) = after.find("```") {
            out = Some(after[..end].trim().to_string());
            rest = &after[end + 3..];
        } else {
            break;
        }
    }

    out
}

/// Return the declared choice that appears in the response, case-insensitively,
/// or None. Prefers the first declared choice that matches.
pub fn parse_decision_port(response: &str, choices: &[String]) -> Option<String> {
    let lower = response.to_lowercase();
    choices
        .iter()
        .find(|choice| lower.contains(&choice.to_lowercase()))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_trailing_json_block() {
        let resp = "Here is the result:\n```json\n{\"status\":\"ok\",\"n\":3}\n```\n";
        assert_eq!(
            extract_structured_output(resp, Some("{}")),
            json!({"status":"ok","n":3})
        );
    }

    #[test]
    fn extracts_whole_json_response() {
        assert_eq!(extract_structured_output("{\"a\":1}", None), json!({"a":1}));
    }

    #[test]
    fn falls_back_to_text_wrap() {
        assert_eq!(
            extract_structured_output("just prose", None),
            json!({"text":"just prose"})
        );
    }

    #[test]
    fn decision_matches_declared_choice_case_insensitively() {
        let choices = vec!["approve".to_string(), "deny".to_string()];
        assert_eq!(
            parse_decision_port("I choose APPROVE.", &choices),
            Some("approve".to_string())
        );
    }

    #[test]
    fn decision_returns_none_when_no_choice_present() {
        let choices = vec!["approve".to_string(), "deny".to_string()];
        assert_eq!(parse_decision_port("unsure", &choices), None);
    }
}
