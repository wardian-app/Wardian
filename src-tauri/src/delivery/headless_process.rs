use std::path::PathBuf;

use wardian_core::control::{
    DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef, InteractionRecord,
};
use wardian_core::models::AgentConfig;

#[derive(Debug, Clone)]
pub struct HeadlessProcessPromptRequest {
    pub node: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub session_id: String,
    pub resume_session: Option<String>,
    pub config_override: Option<AgentConfig>,
    pub interaction_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HeadlessProcessPromptResult {
    pub interaction: InteractionRecord,
    pub response: String,
}

pub async fn run_headless_process_prompt(
    state: &crate::state::AppState,
    request: HeadlessProcessPromptRequest,
) -> Result<HeadlessProcessPromptResult, String> {
    let interaction = match request.interaction_id.clone() {
        Some(id) => state
            .interactions
            .interaction(&id)
            .await
            .ok_or_else(|| format!("interaction not found: {id}"))?,
        None => state
            .interactions
            .create_message_durable(
                None,
                vec![request.session_id.clone()],
                InteractionBodyRef::Inline {
                    body: request.prompt.clone(),
                },
            )
            .await
            .map_err(|error| error.to_string())?,
    };

    let value = crate::manager::run_headless_with_options(crate::manager::HeadlessRunOptions {
        cwd: &request.cwd,
        prompt: &request.prompt,
        wardian_session_id: &request.session_id,
        resume_session: request.resume_session.as_deref(),
        output_format: "json",
        provider_name: &request.provider,
        config_override: request.config_override.as_ref(),
    })
    .await;

    match value {
        Ok(value) => {
            let response = value
                .get("response")
                .and_then(|value| value.as_str())
                .or_else(|| value.get("text").and_then(|value| value.as_str()))
                .map(ToString::to_string)
                .unwrap_or_else(|| value.to_string());
            state
                .interactions
                .record_delivery_attempt_durable(
                    &interaction.id,
                    &request.session_id,
                    DeliveryTransportKind::HeadlessProcess,
                    0,
                    "headless_process",
                    "provider_applied",
                    Some("process_completed".to_string()),
                    Some("stdout_parsed".to_string()),
                    Some(format!("workflow node {}", request.node)),
                    None,
                )
                .await?;
            Ok(HeadlessProcessPromptResult {
                interaction,
                response,
            })
        }
        Err(error) => {
            state
                .interactions
                .record_delivery_attempt_durable(
                    &interaction.id,
                    &request.session_id,
                    DeliveryTransportKind::HeadlessProcess,
                    0,
                    "headless_process",
                    "failed",
                    Some("process_failed".to_string()),
                    None,
                    Some(format!("workflow node {}", request.node)),
                    Some(DeliveryErrorDetail {
                        code: "headless_process_failed".to_string(),
                        message: sanitize_headless_error(&error, &request.prompt),
                    }),
                )
                .await?;
            Err(error)
        }
    }
}

fn sanitize_headless_error(error: &str, prompt: &str) -> String {
    let mut diagnostic = error.replace(['\r', '\n'], " ");
    let trimmed_prompt = prompt.trim();
    if !trimmed_prompt.is_empty() {
        diagnostic = diagnostic.replace(trimmed_prompt, "[redacted prompt]");
    }
    for marker in ["sk-", "ghp_", "github_pat_", "glpat-", "xoxb-", "xoxp-"] {
        if diagnostic.contains(marker) {
            diagnostic = diagnostic.replace(marker, "[redacted-secret-prefix]");
        }
    }
    let max_chars = 240;
    let mut bounded = diagnostic.chars().take(max_chars).collect::<String>();
    if diagnostic.chars().count() > max_chars {
        bounded.push_str("...");
    }
    if bounded.trim().is_empty() {
        "provider process failed; diagnostic unavailable".to_string()
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::control::{DeliveryTransportKind, InteractionBodyRef};

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous_home: Option<std::ffi::OsString>,
        previous_script: Option<std::ffi::OsString>,
        previous_scenario: Option<std::ffi::OsString>,
        _home: tempfile::TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            let lock = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            let previous_script = std::env::var_os("WARDIAN_MOCK_SCRIPT");
            let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
            std::env::set_var("WARDIAN_HOME", home.path());
            wardian_core::db::init_db_at_path(&home.path().join("state.db"))
                .expect("init test database");
            std::env::remove_var("WARDIAN_MOCK_SCRIPT");
            std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless");
            Self {
                _lock: lock,
                previous_home,
                previous_script,
                previous_scenario,
                _home: home,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
            match self.previous_script.take() {
                Some(value) => std::env::set_var("WARDIAN_MOCK_SCRIPT", value),
                None => std::env::remove_var("WARDIAN_MOCK_SCRIPT"),
            }
            match self.previous_scenario.take() {
                Some(value) => std::env::set_var("WARDIAN_MOCK_SCENARIO", value),
                None => std::env::remove_var("WARDIAN_MOCK_SCENARIO"),
            }
        }
    }

    fn node_available() -> bool {
        std::process::Command::new(if cfg!(windows) { "node.exe" } else { "node" })
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn headless_request_keeps_transport_separate_from_live_surface() {
        let request = HeadlessProcessPromptRequest {
            node: "audit".to_string(),
            provider: "mock".to_string(),
            cwd: std::path::PathBuf::from("<absolute-workspace-path>"),
            prompt: "hello".to_string(),
            session_id: "agent-1".to_string(),
            resume_session: None,
            config_override: None,
            interaction_id: Some("int-1".to_string()),
        };

        assert_eq!(request.provider, "mock");
        assert_eq!(request.interaction_id.as_deref(), Some("int-1"));
    }

    #[tokio::test]
    async fn headless_process_success_persists_transport_attempt() {
        if !node_available() {
            return;
        }
        let _env = TestEnv::new();
        let workspace = tempfile::tempdir().expect("workspace");
        let state = crate::state::AppState::new();

        let result = run_headless_process_prompt(
            &state,
            HeadlessProcessPromptRequest {
                node: "audit".to_string(),
                provider: "mock".to_string(),
                cwd: workspace.path().to_path_buf(),
                prompt: "hello".to_string(),
                session_id: "agent-1".to_string(),
                resume_session: None,
                config_override: None,
                interaction_id: None,
            },
        )
        .await
        .expect("headless success");

        assert_eq!(
            result.response,
            "Mock headless execution completed successfully."
        );
        let attempts = wardian_core::db::list_interaction_delivery_attempts(&result.interaction.id)
            .expect("list attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].transport,
            DeliveryTransportKind::HeadlessProcess
        );
        assert_eq!(attempts[0].delivery_state, "provider_applied");
    }

    #[tokio::test]
    async fn headless_process_failure_persists_sanitized_attempt() {
        if !node_available() {
            return;
        }
        let _env = TestEnv::new();
        let workspace = tempfile::tempdir().expect("workspace");
        let script_dir = tempfile::tempdir().expect("script dir");
        let script = script_dir.path().join("failing-mock-agent.cjs");
        std::fs::write(
            &script,
            "process.stderr.write('raw failure for secret prompt\\n'); process.exit(1);\n",
        )
        .expect("write failing script");
        std::env::set_var("WARDIAN_MOCK_SCRIPT", &script);

        let state = crate::state::AppState::new();
        let interaction = state
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "workflow task".to_string(),
                },
            )
            .await
            .expect("interaction");

        let error = run_headless_process_prompt(
            &state,
            HeadlessProcessPromptRequest {
                node: "audit".to_string(),
                provider: "mock".to_string(),
                cwd: workspace.path().to_path_buf(),
                prompt: "secret prompt".to_string(),
                session_id: "agent-1".to_string(),
                resume_session: None,
                config_override: None,
                interaction_id: Some(interaction.id.clone()),
            },
        )
        .await
        .expect_err("headless failure");

        assert!(error.contains("secret prompt"));
        let attempts = wardian_core::db::list_interaction_delivery_attempts(&interaction.id)
            .expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].transport,
            DeliveryTransportKind::HeadlessProcess
        );
        assert_eq!(attempts[0].delivery_state, "failed");
        let persisted_error = attempts[0].error.as_ref().expect("persisted error");
        assert_eq!(persisted_error.code, "headless_process_failed");
        assert!(!persisted_error.message.contains("secret prompt"));
        assert!(persisted_error.message.contains("[redacted prompt]"));
    }

    #[test]
    fn headless_error_diagnostic_redacts_prompt_and_bounds_output() {
        let prompt = "secret customer prompt";
        let raw = format!(
            "Headless provider mock exited with status 1: failed while handling {prompt}\n{}",
            "x".repeat(400)
        );

        let sanitized = sanitize_headless_error(&raw, prompt);

        assert!(!sanitized.contains(prompt));
        assert!(sanitized.contains("[redacted prompt]"));
        assert!(sanitized.len() <= 243);
    }
}
