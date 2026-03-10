#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentTelemetry {
    pub session_id: String,
    pub cpu_usage: f32,
    pub memory_mb: f64,
    pub uptime_seconds: u64,
    pub query_count: usize,
    pub init_timestamp: Option<String>,
    pub current_status: String,
    pub log_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_telemetry_serializes() {
        let telemetry = AgentTelemetry {
            session_id: "abc".into(),
            cpu_usage: 15.5,
            memory_mb: 256.3,
            uptime_seconds: 3600,
            query_count: 42,
            init_timestamp: Some("2026-01-01T00:00:00Z".into()),
            current_status: "Idle".into(),
            log_path: None,
        };
        let json = serde_json::to_string(&telemetry).unwrap();
        assert!(json.contains("\"session_id\":\"abc\""));
        assert!(json.contains("\"query_count\":42"));
    }
}
