//! On-demand own-versus-subagent telemetry details.

use chrono::DateTime;
use serde::Serialize;
use wardian_core::telemetry::horizon::HorizonWindow;
use wardian_core::telemetry::matrix::Measure;

const BREAKDOWN_MEASURES: [Measure; 13] = [
    Measure::ActiveMs,
    Measure::Turns,
    Measure::FreshTokens,
    Measure::CachedTokens,
    Measure::CacheWriteTokens,
    Measure::OutputTokens,
    Measure::ReasoningTokens,
    Measure::TotalTokens,
    Measure::CacheHitRate,
    Measure::Files,
    Measure::LinesAdded,
    Measure::LinesRemoved,
    Measure::LinesChanged,
];

#[derive(Debug, Clone, Serialize)]
pub struct TelemetryAgentBreakdownMeasureDto {
    pub measure: Measure,
    pub total: Option<i64>,
    pub own: Option<i64>,
    pub subagents: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelemetryAgentBreakdownDto {
    pub key: String,
    pub label: String,
    pub can_open_agent: bool,
    pub window: HorizonWindow,
    pub measures: Vec<TelemetryAgentBreakdownMeasureDto>,
}

/// Read the exact caller-supplied window for one roster root.
#[tauri::command(async, rename_all = "snake_case")]
pub fn telemetry_agent_breakdown(
    session_id: String,
    from: String,
    to: String,
) -> Result<TelemetryAgentBreakdownDto, String> {
    validate_window(&from, &to)?;
    if session_id.trim().is_empty() {
        return Err("session_id is required".to_string());
    }

    // Read labels before taking the global database connection. The connection
    // guard is non-reentrant, and the detail query itself owns the lock.
    let agent = wardian_core::db::get_all_agents()
        .unwrap_or_default()
        .into_iter()
        .find(|agent| agent.session_id == session_id);
    let (label, can_open_agent) = match agent {
        Some(agent) if !agent.session_name.trim().is_empty() => (agent.session_name, true),
        _ => (historical_label(&session_id), false),
    };
    let window = HorizonWindow {
        from: from.clone(),
        to: to.clone(),
        from_floored: false,
    };

    wardian_core::db::get_db_conn(|conn| {
        let breakdown =
            wardian_core::telemetry::attribution::agent_breakdown(conn, &session_id, &from, &to)?;
        let measures = BREAKDOWN_MEASURES
            .into_iter()
            .map(|measure| TelemetryAgentBreakdownMeasureDto {
                measure,
                total: breakdown.total.value(measure),
                own: breakdown.own.value(measure),
                subagents: breakdown.subagents.value(measure),
            })
            .collect();
        Ok(TelemetryAgentBreakdownDto {
            key: session_id,
            label,
            can_open_agent,
            window,
            measures,
        })
    })
    .map_err(|error| format!("could not read telemetry agent breakdown: {error}"))
}

fn validate_window(from: &str, to: &str) -> Result<(), String> {
    let from_instant = DateTime::parse_from_rfc3339(from)
        .map_err(|error| format!("invalid telemetry from timestamp: {error}"))?;
    let to_instant = DateTime::parse_from_rfc3339(to)
        .map_err(|error| format!("invalid telemetry to timestamp: {error}"))?;
    if from_instant >= to_instant {
        return Err("telemetry window must have an increasing from/to range".to_string());
    }
    Ok(())
}

fn historical_label(session_id: &str) -> String {
    let short_id: String = session_id.chars().take(8).collect();
    format!("Historical agent {short_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_windows_require_increasing_rfc3339_timestamps() {
        assert!(validate_window("2026-08-20T00:00:00Z", "2026-08-20T01:00:00Z").is_ok());
        assert!(validate_window("2026-08-20T01:00:00Z", "2026-08-20T00:00:00Z").is_err());
        assert!(validate_window("not-a-time", "2026-08-20T01:00:00Z").is_err());
    }

    #[test]
    fn historical_label_keeps_a_short_traceable_identity() {
        assert_eq!(
            historical_label("1234567890abcdef"),
            "Historical agent 12345678"
        );
    }

    #[test]
    fn detail_measure_order_matches_the_existing_telemetry_enum() {
        assert_eq!(BREAKDOWN_MEASURES.len(), 13);
        assert_eq!(
            BREAKDOWN_MEASURES
                .iter()
                .map(|measure| measure.as_str())
                .collect::<Vec<_>>(),
            vec![
                "active_ms",
                "turns",
                "fresh_tokens",
                "cached_tokens",
                "cache_write_tokens",
                "output_tokens",
                "reasoning_tokens",
                "total_tokens",
                "cache_hit_rate",
                "files",
                "lines_added",
                "lines_removed",
                "lines_changed",
            ]
        );
    }

    #[test]
    fn ipc_dispatch_reads_contract_snake_case_arguments() {
        let app = tauri::test::mock_builder()
            .invoke_handler(tauri::generate_handler![telemetry_agent_breakdown])
            .build(tauri::generate_context!())
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "telemetry_agent_breakdown".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "https://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: tauri::ipc::InvokeBody::Json(serde_json::json!({
                    "session_id": "",
                    "from": "2026-09-18T00:00:00Z",
                    "to": "2026-09-18T01:00:00Z"
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(response, serde_json::json!("session_id is required"));
    }
}
