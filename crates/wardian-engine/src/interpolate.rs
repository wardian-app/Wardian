/// Resolve `{{ dotted.path }}` placeholders in `template` against `registry`
/// (a JSON object). Returns the resolved string, or `Err(path)` for the first
/// reference that doesn't resolve to a scalar. Whitespace inside `{{ }}` is
/// trimmed. Non-string scalars (numbers/bools) render via their JSON form.
pub fn resolve(template: &str, registry: &serde_json::Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| "unterminated {{".to_string())?;
        let path = after[..end].trim();
        out.push_str(&lookup(path, registry).ok_or_else(|| path.to_string())?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn lookup(path: &str, registry: &serde_json::Value) -> Option<String> {
    let mut cur = registry;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    match cur {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null | serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            None
        }
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> serde_json::Value {
        serde_json::json!({
            "nodes": { "plan": { "output": { "path": "src/x.rs", "n": 3 } } },
            "trigger": { "output": { "id": "wf-1" } },
            "storage": { "branch": "main" }
        })
    }

    #[test]
    fn resolves_known_paths() {
        let out = resolve(
            "file {{nodes.plan.output.path}} n={{nodes.plan.output.n}} on {{storage.branch}}",
            &reg(),
        )
        .unwrap();
        assert_eq!(out, "file src/x.rs n=3 on main");
    }

    #[test]
    fn passes_through_text_without_placeholders() {
        assert_eq!(resolve("plain text", &reg()).unwrap(), "plain text");
    }

    #[test]
    fn unresolved_reference_is_an_error() {
        let err = resolve("{{nodes.ghost.output.x}}", &reg()).unwrap_err();
        assert!(err.contains("nodes.ghost.output.x"));
    }
}
