use crate::registry::node_types;

/// The registry as a self-describing JSON value the TS builder consumes. The
/// `NodeTypeDef`/`FieldDef` serde derives carry the shape; this just wraps them
/// with a schema version.
pub fn ts_schema_value() -> serde_json::Value {
    serde_json::json!({
        "schema": 2,
        "node_types": node_types(),
    })
}

/// Pretty-printed JSON for writing to a committed artifact the builder imports.
pub fn ts_schema_json() -> String {
    serde_json::to_string_pretty(&ts_schema_value()).expect("registry serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_every_node_type_with_fields_and_ports() {
        let value = ts_schema_value();
        assert_eq!(value["schema"], 2);
        let types = value["node_types"].as_array().unwrap();
        assert_eq!(types.len(), crate::registry::node_types().len());
        let task = types.iter().find(|t| t["id"] == "task").unwrap();
        assert_eq!(task["kind"], "agent");
        assert!(task["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "prompt"));
    }

    #[test]
    fn schema_is_pretty_json_string() {
        let s = ts_schema_json();
        assert!(s.starts_with('{'));
        assert!(s.contains("\"node_types\""));
    }
}
