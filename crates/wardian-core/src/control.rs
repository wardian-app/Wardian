use crate::identity::AgentIdentity;
use serde::{Deserialize, Serialize};
pub const CONTROL_SCHEMA: u8 = 1;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ControlRequest {
    AgentList,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentListResponse {
    pub schema: u8,
    pub agents: Vec<AgentIdentity>,
}

impl AgentListResponse {
    pub fn new(agents: Vec<AgentIdentity>) -> Self {
        Self {
            schema: CONTROL_SCHEMA,
            agents,
        }
    }
}

pub fn endpoint_key() -> Option<String> {
    let home = crate::paths::wardian_home()?;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in home.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    Some(format!("{hash:016x}"))
}

#[cfg(windows)]
pub fn pipe_name() -> Option<String> {
    endpoint_key().map(|key| format!(r"\\.\pipe\wardian-control-{key}"))
}

#[cfg(unix)]
pub fn socket_path() -> Option<std::path::PathBuf> {
    crate::paths::wardian_home().map(|home| home.join("run").join("control.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_response_uses_current_schema() {
        let response = AgentListResponse::new(Vec::new());
        assert_eq!(response.schema, CONTROL_SCHEMA);
    }

    #[test]
    fn endpoint_key_is_stable_for_home_path() {
        let _guard = crate::tests::env_lock();
        std::env::set_var("WARDIAN_HOME", "D:/Development/Wardian/.tmp/live-status");
        assert_eq!(endpoint_key().as_deref(), Some("50403db71b810eba"));
        std::env::remove_var("WARDIAN_HOME");
    }
}
