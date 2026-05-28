use serde::{Deserialize, Serialize};

/// The closed set of field-type primitives every node field is composed from.
/// `kind` is the serde tag so the generated TS schema and `--json` output are
/// self-describing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldType {
    Text,
    LongText,
    Prompt,
    Code { language: String },
    Enum { options: Vec<String> },
    Bool,
    Number,
    Duration,
    Path,
    AgentRef,
    RoleRef,
    ClassRef,
    McpRef,
    WorkflowRef,
    JsonSchema,
    KvMap,
    /// Declares a named outgoing branch on the node (e.g. Decision choices).
    BranchPort,
    Cron,
    SecretRef,
}

/// A single field on a node type: its primitive type plus authoring metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    pub id: String,
    #[serde(flatten)]
    pub field_type: FieldType,
    pub label: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
}

impl FieldDef {
    pub fn new(id: impl Into<String>, field_type: FieldType, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            field_type,
            label: label.into(),
            required: false,
            multiple: false,
            default: None,
            help: String::new(),
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn multiple(mut self) -> Self {
        self.multiple = true;
        self
    }

    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.help = text.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_type_serializes_with_snake_case_kind_tag() {
        let enum_ty = FieldType::Enum {
            options: vec!["a".into(), "b".into()],
        };
        let json = serde_json::to_value(&enum_ty).unwrap();
        assert_eq!(json["kind"], "enum");
        assert_eq!(json["options"][0], "a");
    }

    #[test]
    fn code_field_carries_language() {
        let ty = FieldType::Code {
            language: "python".into(),
        };
        let json = serde_json::to_value(&ty).unwrap();
        assert_eq!(json["kind"], "code");
        assert_eq!(json["language"], "python");
    }

    #[test]
    fn field_def_defaults_are_not_required_and_single() {
        let def = FieldDef::new("prompt", FieldType::Prompt, "Prompt");
        assert!(!def.required);
        assert!(!def.multiple);
        assert!(def.default.is_none());
    }
}
