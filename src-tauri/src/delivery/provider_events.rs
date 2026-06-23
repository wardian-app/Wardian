use serde::{Deserialize, Serialize};
use wardian_core::control::DeliveryTransportKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEventKind {
    UserPromptSubmit,
    PermissionRequest,
    PermissionDecision,
    ToolStart,
    ToolEnd,
    SessionStart,
    SessionEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderEventEnvelope {
    pub provider: String,
    pub wardian_session_id: String,
    pub provider_session_id: Option<String>,
    pub interaction_id: Option<String>,
    pub kind: ProviderEventKind,
    pub transport: DeliveryTransportKind,
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_event_envelope_deserializes_plugin_payload() {
        let event: ProviderEventEnvelope = serde_json::from_str(
            r#"{
              "provider":"opencode",
              "wardian_session_id":"agent-1",
              "provider_session_id":"oc-1",
              "interaction_id":"int-1",
              "kind":"user_prompt_submit",
              "transport":"provider_plugin",
              "payload":{"text":"hello"}
            }"#,
        )
        .expect("deserialize provider event");

        assert_eq!(event.provider, "opencode");
        assert_eq!(event.kind, ProviderEventKind::UserPromptSubmit);
        assert_eq!(event.transport, DeliveryTransportKind::ProviderPlugin);
    }
}
