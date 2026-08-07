use std::{path::PathBuf, time::Duration};

use wardian_core::control::{
    DeliveryErrorDetail, DeliveryTransportKind, InteractionBodyRef, InteractionRecord,
};
use wardian_core::conversation_lease::ConversationLeaseOwner;
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
    pub timeout: Duration,
    pub lease_owner: Option<ConversationLeaseOwner>,
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

    let value = run_with_session_lease_retries(&request).await;

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
                    Some(format!("headless run {}", request.node)),
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
                    Some(format!("headless run {}", request.node)),
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

/// Runs the headless prompt, retrying only a launch that Prime's session lease
/// refused.
///
/// Prime is the only provider that guards its session file with a lease keyed
/// to the owning process, so a resume issued while the previous worker is still
/// exiting fails for a reason that clears on its own. Every other failure, and
/// every other provider, is returned on the first attempt. Retries are
/// deliberately not recorded as separate delivery attempts: one prompt still
/// produces one attempt row, whose outcome is the one the caller sees.
async fn run_with_session_lease_retries(
    request: &HeadlessProcessPromptRequest,
) -> Result<serde_json::Value, String> {
    let mut backoff = session_lease_retry_backoff(&request.provider).iter();

    loop {
        let outcome =
            crate::manager::run_headless_with_options(crate::manager::HeadlessRunOptions {
                cwd: &request.cwd,
                prompt: &request.prompt,
                wardian_session_id: &request.session_id,
                resume_session: request.resume_session.as_deref(),
                output_format: "json",
                provider_name: &request.provider,
                config_override: request.config_override.as_ref(),
                timeout: request.timeout,
                lease_owner: request.lease_owner.clone(),
            })
            .await;

        let Err(error) = outcome else {
            return outcome;
        };
        if !crate::providers::PrimeProvider::is_session_lease_conflict(&error) {
            return Err(error);
        }
        let Some(delay) = backoff.next() else {
            return Err(error);
        };

        crate::utils::logging::log_debug(&format!(
            "[Wardian] Prime session lease busy for {} (owner {}); retrying in {}ms",
            request.session_id,
            crate::providers::PrimeProvider::session_lease_conflict_owner(&error)
                .unwrap_or_else(|| "unnamed".to_string()),
            delay.as_millis()
        ));
        tokio::time::sleep(*delay).await;
    }
}

/// The retry schedule for a provider, empty for everything but Prime.
fn session_lease_retry_backoff(provider: &str) -> &'static [Duration] {
    if provider.eq_ignore_ascii_case("prime") {
        &crate::providers::PrimeProvider::SESSION_LEASE_RETRY_BACKOFF
    } else {
        &[]
    }
}

pub(crate) fn sanitize_headless_error(error: &str, prompt: &str) -> String {
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
        previous_delay: Option<std::ffi::OsString>,
        _home: tempfile::TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            let lock = crate::utils::wardian_test_env_lock();
            let home = tempfile::tempdir().expect("temp wardian home");
            let previous_home = std::env::var_os("WARDIAN_HOME");
            let previous_script = std::env::var_os("WARDIAN_MOCK_SCRIPT");
            let previous_scenario = std::env::var_os("WARDIAN_MOCK_SCENARIO");
            let previous_delay = std::env::var_os("WARDIAN_MOCK_DELAY_MS");
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
                previous_delay,
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
            match self.previous_delay.take() {
                Some(value) => std::env::set_var("WARDIAN_MOCK_DELAY_MS", value),
                None => std::env::remove_var("WARDIAN_MOCK_DELAY_MS"),
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
            timeout: crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
            lease_owner: None,
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
                timeout: crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
                lease_owner: None,
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
                timeout: crate::manager::DEFAULT_HEADLESS_RUN_TIMEOUT,
                lease_owner: None,
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

    #[tokio::test]
    async fn headless_process_times_out_and_persists_a_failed_attempt() {
        if !node_available() {
            return;
        }
        let _env = TestEnv::new();
        let workspace = tempfile::tempdir().expect("workspace");
        std::env::set_var("WARDIAN_MOCK_SCENARIO", "headless_delayed");
        std::env::set_var("WARDIAN_MOCK_DELAY_MS", "1000");
        let state = crate::state::AppState::new();
        let interaction = state
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "take too long".to_string(),
                },
            )
            .await
            .expect("interaction");

        let started = std::time::Instant::now();
        let error = run_headless_process_prompt(
            &state,
            HeadlessProcessPromptRequest {
                node: "timeout".to_string(),
                provider: "mock".to_string(),
                cwd: workspace.path().to_path_buf(),
                prompt: "take too long".to_string(),
                session_id: "agent-1".to_string(),
                resume_session: None,
                config_override: None,
                interaction_id: Some(interaction.id.clone()),
                timeout: Duration::from_millis(25),
                lease_owner: None,
            },
        )
        .await
        .expect_err("headless process should time out");

        assert!(error.contains("exceeded its"));
        assert!(started.elapsed() < Duration::from_secs(2));
        let attempts = wardian_core::db::list_interaction_delivery_attempts(&interaction.id)
            .expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].delivery_state, "failed");
    }

    #[test]
    fn only_prime_retries_a_busy_session_lease() {
        assert!(!session_lease_retry_backoff("prime").is_empty());
        assert!(!session_lease_retry_backoff("Prime").is_empty());
        // No other provider leases its session file, so a failure there is
        // final and must surface immediately.
        for provider in [
            "claude",
            "codex",
            "gemini",
            "opencode",
            "antigravity",
            "mock",
        ] {
            assert!(session_lease_retry_backoff(provider).is_empty());
        }
    }

    #[tokio::test]
    async fn a_failing_prime_launch_still_records_one_attempt() {
        if !node_available() {
            return;
        }
        let _env = TestEnv::new();
        let workspace = tempfile::tempdir().expect("workspace");
        let state = crate::state::AppState::new();
        let interaction = state
            .interactions
            .create_message_durable(
                None,
                vec!["agent-1".to_string()],
                InteractionBodyRef::Inline {
                    body: "hello".to_string(),
                },
            )
            .await
            .expect("interaction");

        // The isolated WARDIAN_HOME has no Prime kernel environment, so this
        // fails the readiness gate rather than the session lease. That is the
        // point: a non-lease failure must return on the first attempt instead
        // of sitting through the backoff or recording extra attempt rows.
        let started = std::time::Instant::now();
        run_headless_process_prompt(
            &state,
            HeadlessProcessPromptRequest {
                node: "audit".to_string(),
                provider: "prime".to_string(),
                cwd: workspace.path().to_path_buf(),
                prompt: "hello".to_string(),
                session_id: "agent-1".to_string(),
                resume_session: None,
                config_override: None,
                interaction_id: Some(interaction.id.clone()),
                timeout: Duration::from_secs(20),
                lease_owner: None,
            },
        )
        .await
        .expect_err("prime headless run should fail without a lease conflict");

        assert!(started.elapsed() < Duration::from_secs(10));
        let attempts = wardian_core::db::list_interaction_delivery_attempts(&interaction.id)
            .expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].delivery_state, "failed");
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
