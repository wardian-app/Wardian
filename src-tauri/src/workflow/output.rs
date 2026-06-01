//! Pure helpers for turning a headless agent's free-form response into the
//! structured `nodes.<id>.output` value, and a decision's chosen port.

use serde_json::Value;

/// Extract structured output from a worker response. Tries, in order: a trailing
/// fenced ```json block, then the whole response as JSON, else wraps as
/// `{"text": ..}` when no schema was requested.
pub fn extract_structured_output(
    response: &str,
    output_schema: Option<&str>,
) -> Result<Value, String> {
    let output_schema = output_schema.filter(|schema| !schema.trim().is_empty());

    if let Some(block) = last_json_block(response) {
        if let Ok(value) = serde_json::from_str::<Value>(&block) {
            validate_output_schema(&value, output_schema)?;
            return Ok(value);
        }
    }

    let trimmed = response.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        validate_output_schema(&value, output_schema)?;
        return Ok(value);
    }

    if output_schema.is_some() {
        return Err("agent response did not contain valid JSON for declared output_schema".into());
    }

    Ok(serde_json::json!({ "text": response }))
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

fn validate_output_schema(value: &Value, output_schema: Option<&str>) -> Result<(), String> {
    let Some(schema_text) = output_schema.filter(|schema| !schema.trim().is_empty()) else {
        return Ok(());
    };
    let schema: Value = serde_json::from_str(schema_text)
        .map_err(|error| format!("invalid output_schema JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "agent output_schema requires a JSON object response".to_string())?;

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| properties.keys().cloned().collect());
        for field in required {
            let actual = object
                .get(&field)
                .ok_or_else(|| format!("agent output missing required field `{field}`"))?;
            if let Some(expected) = properties
                .get(&field)
                .and_then(|property| property.get("type"))
                .and_then(Value::as_str)
            {
                validate_field_type(&field, actual, expected)?;
            }
        }
        return Ok(());
    }

    let schema_object = schema
        .as_object()
        .ok_or_else(|| "output_schema must be a JSON object".to_string())?;
    for (field, expected) in schema_object {
        let actual = object
            .get(field)
            .ok_or_else(|| format!("agent output missing required field `{field}`"))?;
        if let Some(expected_type) = expected.as_str() {
            validate_field_type(field, actual, expected_type)?;
        }
    }
    Ok(())
}

fn validate_field_type(field: &str, value: &Value, expected: &str) -> Result<(), String> {
    let valid = match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" | "bool" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "agent output field `{field}` did not match output_schema type `{expected}`"
        ))
    }
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
            extract_structured_output(resp, Some("{}")).unwrap(),
            json!({"status":"ok","n":3})
        );
    }

    #[test]
    fn extracts_whole_json_response() {
        assert_eq!(
            extract_structured_output("{\"a\":1}", None).unwrap(),
            json!({"a":1})
        );
    }

    #[test]
    fn falls_back_to_text_wrap() {
        assert_eq!(
            extract_structured_output("just prose", None).unwrap(),
            json!({"text":"just prose"})
        );
    }

    #[test]
    fn blank_schema_falls_back_to_text_wrap() {
        assert_eq!(
            extract_structured_output("just prose", Some("   ")).unwrap(),
            json!({"text":"just prose"})
        );
    }

    #[test]
    fn schema_rejects_non_json_response() {
        let err = extract_structured_output("just prose", Some(r#"{"decision":"string"}"#))
            .expect_err("schema-bound agent output must be JSON");
        assert!(err.contains("valid JSON"));
    }

    #[test]
    fn schema_requires_declared_fields() {
        let err = extract_structured_output(
            r#"{"decision":"ok"}"#,
            Some(r#"{"decision":"string","reason":"string"}"#),
        )
        .expect_err("missing schema field should fail");
        assert!(err.contains("reason"));
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
