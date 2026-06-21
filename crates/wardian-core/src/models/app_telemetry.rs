#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppTelemetry {
    pub cpu_usage: f32,
    pub memory_mb: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_telemetry_serializes() {
        let telemetry = AppTelemetry {
            cpu_usage: 2.5,
            memory_mb: 128.4,
        };

        let json = serde_json::to_string(&telemetry).unwrap();

        assert!(json.contains("\"cpu_usage\":2.5"));
        assert!(json.contains("\"memory_mb\":128.4"));
    }
}
