//! Guards the boundary between Wardian/provider session identity and ambient credentials.

use std::ffi::{OsStr, OsString};
use wardian_core::models::AgentConfig;

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

fn clear_credential_resume_session_with_environment(
    config: &mut AgentConfig,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> bool {
    let matches = config
        .resume_session
        .as_deref()
        .is_some_and(|value| value_matches_credentials(value, environment));
    if matches {
        config.resume_session = None;
    }
    matches
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

pub(crate) fn clear_credential_resume_session(config: &mut AgentConfig) -> bool {
    clear_credential_resume_session_with_environment(config, std::env::vars_os())
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
    fn poisoned_resume_is_cleared_without_changing_wardian_uuid() {
        let secret = "00000000-0000-4000-8000-0000000000aa";
        let mut config = AgentConfig {
            session_id: "wardian-session".into(),
            resume_session: Some(secret.into()),
            ..Default::default()
        };
        assert!(clear_credential_resume_session_with_environment(
            &mut config,
            env(&[("GEMINI_API_KEY", secret)])
        ));
        assert_eq!(config.session_id, "wardian-session");
        assert_eq!(config.resume_session, None);
    }
}
