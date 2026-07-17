//! Guards the boundary between Wardian/provider session identity and ambient credentials.

use std::ffi::{OsStr, OsString};
use wardian_core::models::AgentConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderIdentityOutcome {
    Confirmed,
    Captured,
}

fn credential_env_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    name == "API_KEY"
        || name.ends_with("_API_KEY")
        || name == "TOKEN"
        || name.ends_with("_TOKEN")
        || name == "SECRET"
        || name.ends_with("_SECRET")
        || name.contains("_SECRET_")
        || name == "PASSWORD"
        || name.ends_with("_PASSWORD")
}

fn value_matches_credentials(
    candidate: &str,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> bool {
    let candidate = candidate.trim();
    !candidate.is_empty()
        && environment
            .into_iter()
            .any(|(name, value)| credential_env_name(&name) && value.to_string_lossy() == candidate)
}

fn validate_session_values_with_environment(
    wardian_session_id: &str,
    resume_session: Option<&str>,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<(), String> {
    let environment = environment.into_iter().collect::<Vec<_>>();
    if value_matches_credentials(wardian_session_id, environment.clone())
        || resume_session.is_some_and(|value| value_matches_credentials(value, environment.clone()))
    {
        return Err(
            "Refusing provider launch because a session identifier matches a credential environment value."
                .to_string(),
        );
    }
    Ok(())
}

fn validate_config_for_launch_with_environment(
    config: &AgentConfig,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<(), String> {
    let environment = environment.into_iter().collect::<Vec<_>>();
    validate_session_values_with_environment(
        &config.session_id,
        config.resume_session.as_deref(),
        environment.clone(),
    )?;
    if config
        .fresh_provider_session_id
        .as_deref()
        .is_some_and(|value| value_matches_credentials(value, environment.clone()))
    {
        return Err("provider session identity matches a credential environment value".to_string());
    }
    Ok(())
}

fn expected_caller_owned_identity(config: &AgentConfig) -> Option<&str> {
    config
        .fresh_provider_session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .resume_session
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
}

fn apply_provider_identity_with_environment(
    provider: &str,
    config: &mut AgentConfig,
    candidate: &str,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<ProviderIdentityOutcome, String> {
    let candidate = candidate.trim();
    let environment = environment.into_iter().collect::<Vec<_>>();
    if candidate.is_empty() {
        return Err(format!("{provider} did not provide a session identity"));
    }
    if value_matches_credentials(candidate, environment) {
        return Err(format!(
            "{provider} session identity matches a credential environment value"
        ));
    }
    if candidate == config.session_id.trim() {
        return Err(format!(
            "{provider} provider identity conflicts with the Wardian agent identity"
        ));
    }

    match provider {
        "claude" | "gemini" => {
            let expected = expected_caller_owned_identity(config).ok_or_else(|| {
                format!("{provider} session identity has no caller-owned expectation")
            })?;
            if candidate != expected {
                return Err(format!(
                    "{provider} returned a conflicting session identity"
                ));
            }
            Ok(ProviderIdentityOutcome::Confirmed)
        }
        "codex" => {
            if uuid::Uuid::parse_str(candidate).is_err() {
                return Err("codex returned a malformed thread identity".to_string());
            }
            capture_or_confirm_provider_identity(provider, config, candidate)
        }
        "opencode" => {
            if candidate.len() <= "ses_".len() || !candidate.starts_with("ses_") {
                return Err("opencode returned a malformed session identity".to_string());
            }
            capture_or_confirm_provider_identity(provider, config, candidate)
        }
        "antigravity" => capture_or_confirm_provider_identity(provider, config, candidate),
        "mock" => Ok(ProviderIdentityOutcome::Confirmed),
        _ => Err(format!(
            "{provider} does not define an initialization identity contract"
        )),
    }
}

fn capture_or_confirm_provider_identity(
    provider: &str,
    config: &mut AgentConfig,
    candidate: &str,
) -> Result<ProviderIdentityOutcome, String> {
    if let Some(expected) = config
        .resume_session
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if expected != candidate {
            return Err(format!(
                "{provider} returned a conflicting session identity"
            ));
        }
        return Ok(ProviderIdentityOutcome::Confirmed);
    }

    config.resume_session = Some(candidate.to_string());
    Ok(ProviderIdentityOutcome::Captured)
}

pub(crate) fn validate_session_values_for_launch(
    wardian_session_id: &str,
    resume_session: Option<&str>,
) -> Result<(), String> {
    validate_session_values_with_environment(
        wardian_session_id,
        resume_session,
        std::env::vars_os(),
    )
}

pub(crate) fn validate_config_for_launch(config: &AgentConfig) -> Result<(), String> {
    validate_config_for_launch_with_environment(config, std::env::vars_os())
}

pub(crate) fn apply_provider_identity(
    provider: &str,
    config: &mut AgentConfig,
    candidate: &str,
) -> Result<ProviderIdentityOutcome, String> {
    apply_provider_identity_with_environment(provider, config, candidate, std::env::vars_os())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use wardian_core::models::AgentConfig;

    fn env(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    fn test_config(
        provider: &str,
        resume_session: Option<&str>,
        fresh_provider_session_id: Option<&str>,
    ) -> AgentConfig {
        AgentConfig {
            provider: provider.to_string(),
            session_id: "wardian-session".to_string(),
            resume_session: resume_session.map(str::to_string),
            fresh_provider_session_id: fresh_provider_session_id.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn uuid_shaped_api_key_is_a_credential_value() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        assert!(value_matches_credentials(
            secret,
            env(&[("OPENAI_API_KEY", secret)])
        ));
    }

    #[test]
    fn wardian_session_environment_is_not_a_credential() {
        let session = "00000000-0000-4000-8000-0000000000aa";
        assert!(!value_matches_credentials(
            session,
            env(&[("WARDIAN_SESSION_ID", session), ("TERM", session)])
        ));
    }

    #[test]
    fn unsafe_launch_error_does_not_echo_the_credential() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let error = validate_session_values_with_environment(
            "wardian-session",
            Some(secret),
            env(&[("ANTHROPIC_API_KEY", secret)]),
        )
        .expect_err("credential resume must fail closed");
        assert!(!error.contains(secret));
        assert!(error.contains("credential environment value"));
    }

    #[test]
    fn credential_resume_is_rejected_without_changing_configuration() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let config = test_config("gemini", Some(secret), None);
        let before = config.clone();
        let error = validate_config_for_launch_with_environment(
            &config,
            env(&[("GEMINI_API_KEY", secret)]),
        )
        .expect_err("credential resume must fail closed");
        assert!(!error.contains(secret));
        assert_eq!(config.session_id, "wardian-session");
        assert_eq!(config.resume_session, before.resume_session);
    }

    #[test]
    fn claude_init_confirms_but_cannot_replace_expected_id() {
        let mut config = test_config("claude", Some("expected"), None);
        assert_eq!(
            apply_provider_identity_with_environment("claude", &mut config, "expected", Vec::new(),),
            Ok(ProviderIdentityOutcome::Confirmed),
        );

        let before = config.clone();
        let error = apply_provider_identity_with_environment(
            "claude",
            &mut config,
            "different",
            Vec::new(),
        )
        .expect_err("conflicting init must fail");
        assert!(!error.contains("different"));
        assert_eq!(config.resume_session, before.resume_session);
    }

    #[test]
    fn gemini_fresh_init_confirms_the_caller_owned_id() {
        let mut config = test_config("gemini", None, Some("fresh-provider-id"));
        assert_eq!(
            apply_provider_identity_with_environment(
                "gemini",
                &mut config,
                "fresh-provider-id",
                Vec::new(),
            ),
            Ok(ProviderIdentityOutcome::Confirmed),
        );
        assert_eq!(config.resume_session, None);
    }

    #[test]
    fn codex_fresh_captures_only_a_uuid() {
        let id = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let mut config = test_config("codex", None, None);
        assert_eq!(
            apply_provider_identity_with_environment("codex", &mut config, id, Vec::new()),
            Ok(ProviderIdentityOutcome::Captured),
        );
        assert_eq!(config.resume_session.as_deref(), Some(id));

        let mut malformed = test_config("codex", None, None);
        let error = apply_provider_identity_with_environment(
            "codex",
            &mut malformed,
            "not-a-uuid",
            Vec::new(),
        )
        .expect_err("malformed Codex ID must fail");
        assert!(!error.contains("not-a-uuid"));
        assert_eq!(malformed.resume_session, None);
    }

    #[test]
    fn codex_resume_rejects_a_different_valid_thread_id() {
        let expected = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let candidate = "019db2f3-22de-7861-8bc6-1b86db1686dc";
        let mut config = test_config("codex", Some(expected), None);
        let error =
            apply_provider_identity_with_environment("codex", &mut config, candidate, Vec::new())
                .expect_err("conflicting Codex ID must fail");
        assert!(!error.contains(candidate));
        assert_eq!(config.resume_session.as_deref(), Some(expected));
    }

    #[test]
    fn provider_identity_cannot_equal_the_wardian_agent_uuid() {
        let wardian_id = "019db2f3-22de-7861-8bc6-1b86db1686db";
        let mut config = test_config("codex", Some(wardian_id), None);
        config.session_id = wardian_id.to_string();
        let before = config.clone();

        let error =
            apply_provider_identity_with_environment("codex", &mut config, wardian_id, Vec::new())
                .expect_err("Wardian identity substitution must fail");

        assert!(!error.contains(wardian_id));
        assert_eq!(config.resume_session, before.resume_session);
    }

    #[test]
    fn opencode_requires_its_provider_owned_id_shape() {
        let mut config = test_config("opencode", None, None);
        assert_eq!(
            apply_provider_identity_with_environment(
                "opencode",
                &mut config,
                "ses_exact",
                Vec::new(),
            ),
            Ok(ProviderIdentityOutcome::Captured),
        );

        let mut malformed = test_config("opencode", None, None);
        assert!(apply_provider_identity_with_environment(
            "opencode",
            &mut malformed,
            "wardian-uuid",
            Vec::new(),
        )
        .is_err());
        assert_eq!(malformed.resume_session, None);
    }

    #[test]
    fn secret_candidate_is_rejected_without_mutation_or_echo() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let mut config = test_config("codex", None, None);
        let error = apply_provider_identity_with_environment(
            "codex",
            &mut config,
            secret,
            env(&[("OPENAI_API_KEY", secret)]),
        )
        .expect_err("secret identity must fail");
        assert!(!error.contains(secret));
        assert_eq!(config.resume_session, None);
    }
}
