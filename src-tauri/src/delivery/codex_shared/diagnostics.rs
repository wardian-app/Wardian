use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

use regex::RegexSet;

use serde_json::Value;

/// Recognize only the 0.159.2-proven rejection forms that establish that an
/// exact-turn steer did not submit model input. Unknown errors remain uncertain.
pub(super) fn stale_steer_rejection(method: &str, params: &Value, error: &Value) -> bool {
    if method != "turn/steer" || error["code"].as_i64() != Some(-32600) {
        return false;
    }
    let Some(expected) = params["expectedTurnId"].as_str().filter(|id| {
        !id.is_empty()
            && !id
                .chars()
                .any(|character| character == '`' || character.is_control())
    }) else {
        return false;
    };
    let Some(message) = error["message"].as_str() else {
        return false;
    };
    if message == "no active turn to steer" {
        return true;
    }
    let prefix = format!("expected active turn id `{expected}` but found `");
    message
        .strip_prefix(prefix.as_str())
        .and_then(|found| found.strip_suffix('`'))
        .is_some_and(|found| {
            !found.is_empty()
                && found != expected
                && !found
                    .chars()
                    .any(|character| character == '`' || character.is_control())
        })
}

const STDERR_SENSITIVE_REDACTION: &str = "[provider stderr omitted: potentially sensitive content]";
const STDERR_TRUNCATED_REDACTION: &str = "[provider stderr omitted: capture limit reached]";
const STDERR_INVALID_REDACTION: &str = "[provider stderr omitted: non-text or control data]";
const STDERR_READ_FAILURE_REDACTION: &str = "[provider stderr omitted: capture failed]";
const MAX_STDERR_DIAGNOSTIC_CHARS: usize = 1024;
const MAX_REDACTION_CREDENTIAL_COUNT: usize = 128;
const MAX_REDACTION_CREDENTIAL_BYTES: usize = 64 * 1024;

/// Path and environment values that must not cross into startup logs.
pub(super) struct StderrRedactionContext {
    private_paths: Vec<String>,
    credential_values: Vec<Vec<u8>>,
    redact_all: bool,
}

impl StderrRedactionContext {
    #[cfg(test)]
    pub(super) fn new(
        private_paths: impl IntoIterator<Item = impl AsRef<Path>>,
        values: impl IntoIterator<Item = Vec<u8>>,
    ) -> Self {
        let private_paths = normalize_private_paths(private_paths);
        let mut credential_values = Vec::new();
        let mut credential_bytes = 0usize;
        let mut redact_all = false;
        for value in values {
            add_credential_value(
                &value,
                &mut credential_values,
                &mut credential_bytes,
                &mut redact_all,
            );
            if redact_all {
                break;
            }
        }
        Self {
            private_paths,
            credential_values,
            redact_all,
        }
    }

    pub(super) fn from_command(
        private_paths: impl IntoIterator<Item = std::path::PathBuf>,
        command: &std::process::Command,
    ) -> Self {
        Self::from_environment(private_paths, std::env::vars_os(), command)
    }

    fn from_environment(
        private_paths: impl IntoIterator<Item = std::path::PathBuf>,
        inherited: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
        command: &std::process::Command,
    ) -> Self {
        let private_paths = normalize_private_paths(private_paths);
        let mut credential_values = Vec::new();
        let mut credential_bytes = 0usize;
        let mut redact_all = false;
        for (name, value) in inherited {
            if is_credential_env_name(&name) {
                if let Some(value) = value.to_str() {
                    add_credential_value(
                        value.as_bytes(),
                        &mut credential_values,
                        &mut credential_bytes,
                        &mut redact_all,
                    );
                } else {
                    redact_all = true;
                }
                if redact_all {
                    break;
                }
            }
        }
        if !redact_all {
            for (name, value) in command.get_envs() {
                if is_credential_env_name(name) {
                    if let Some(value) = value {
                        if let Some(value) = value.to_str() {
                            add_credential_value(
                                value.as_bytes(),
                                &mut credential_values,
                                &mut credential_bytes,
                                &mut redact_all,
                            );
                        } else {
                            redact_all = true;
                        }
                        if redact_all {
                            break;
                        }
                    }
                }
            }
        }
        Self {
            private_paths,
            credential_values,
            redact_all,
        }
    }

    fn contains_private_path(&self, text: &str) -> bool {
        let text = normalized_path(text);
        self.private_paths.iter().any(|path| text.contains(path))
    }

    fn contains_credential(&self, text: &str) -> bool {
        self.credential_values.iter().any(|value| {
            text.as_bytes()
                .windows(value.len())
                .any(|part| part == value)
        })
    }

    fn contains_sensitive_content(&self, text: &str) -> bool {
        self.contains_private_path(text)
            || self.contains_credential(text)
            || unsafe_stderr_patterns().is_match(text)
    }
}

impl Drop for StderrRedactionContext {
    fn drop(&mut self) {
        for value in &mut self.credential_values {
            value.fill(0);
        }
    }
}

fn normalize_private_paths(paths: impl IntoIterator<Item = impl AsRef<Path>>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| normalized_path(&path.as_ref().to_string_lossy()))
        .filter(|path| path.len() >= 4)
        .collect()
}

fn add_credential_value(
    value: &[u8],
    values: &mut Vec<Vec<u8>>,
    total_bytes: &mut usize,
    redact_all: &mut bool,
) {
    if value.is_empty() || values.iter().any(|existing| existing.as_slice() == value) {
        return;
    }
    if value.len() < 4
        || values.len() == MAX_REDACTION_CREDENTIAL_COUNT
        || total_bytes.saturating_add(value.len()) > MAX_REDACTION_CREDENTIAL_BYTES
    {
        *redact_all = true;
        return;
    }
    *total_bytes += value.len();
    values.push(value.to_vec());
}

/// Only this sanitized value may be added to the general startup log.
pub(super) struct SafeStderrDiagnostic(Option<String>);

impl SafeStderrDiagnostic {
    pub(super) fn as_log_value(&self) -> Value {
        self.0
            .as_ref()
            .map(|diagnostic| Value::String(diagnostic.clone()))
            .unwrap_or(Value::Null)
    }

    #[cfg(test)]
    pub(super) fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }

    fn fixed(message: &str) -> Self {
        Self(Some(message.to_string()))
    }

    pub(super) fn from_bytes(
        bytes: &[u8],
        truncated: bool,
        read_failed: bool,
        redaction: &StderrRedactionContext,
    ) -> Self {
        if bytes.is_empty() {
            return Self(None);
        }
        if read_failed {
            return Self::fixed(STDERR_READ_FAILURE_REDACTION);
        }
        if truncated {
            return Self::fixed(STDERR_TRUNCATED_REDACTION);
        }

        let Ok(text) = std::str::from_utf8(bytes) else {
            return Self::fixed(STDERR_INVALID_REDACTION);
        };
        if text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\r' | '\n' | '\t'))
        {
            return Self::fixed(STDERR_INVALID_REDACTION);
        }
        if redaction.redact_all || redaction.contains_sensitive_content(text) {
            return Self::fixed(STDERR_SENSITIVE_REDACTION);
        }

        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            return Self(None);
        }
        if redaction.contains_sensitive_content(&normalized) {
            return Self::fixed(STDERR_SENSITIVE_REDACTION);
        }
        let mut diagnostic = normalized
            .chars()
            .take(MAX_STDERR_DIAGNOSTIC_CHARS)
            .collect::<String>();
        if normalized.chars().count() > MAX_STDERR_DIAGNOSTIC_CHARS {
            diagnostic.push_str("...");
        }
        Self(Some(diagnostic))
    }
}

/// Credential-like names are deliberately broad because their values may be
/// echoed by a child process even when the value itself has no known prefix.
pub(super) fn is_credential_env_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    [
        "APIKEY",
        "API_KEY",
        "ACCESS_KEY",
        "ACCESSKEY",
        "CLIENT_KEY",
        "SIGNING_KEY",
        "SSH_KEY",
        "_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CAPABILITY",
        "CREDENTIAL",
        "AUTHORIZATION",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn unsafe_stderr_patterns() -> &'static RegexSet {
    static PATTERNS: OnceLock<RegexSet> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        RegexSet::new([
            r"(?i)\b(?:api[ _-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|password|passwd|authorization|credential|secret)\b",
            r"(?i)\bbearer\s+\S+|\bbasic\s+[A-Za-z0-9+/=]{8,}",
            r"(?i)\b(?:sk-[A-Za-z0-9_-]{8,}|sk_(?:live|test)_[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|glpat-[A-Za-z0-9_-]{8,}|xox[baprs]-[A-Za-z0-9-]{8,}|AKIA[A-Z0-9]{16})\b",
            r"\b[A-Za-z0-9_-]{32,}\b",
            r"(?i)\b[A-Z]:[\\/]|\\\\[^\\/\s]+[\\/][^\\/\s]+",
            r#"(?i)(?:^|[\s\"'=:(])/(?:[^\s\"'<>]+)"#,
            r#"(?i)(?:^|[\s\"'=:(])(?:\.\.?|~)[\\/](?:[^\s\"'<>]+)"#,
            r"(?i)\b(?:file|unix):///|%USERPROFILE%|%HOME%|\$\{?HOME\}?",
            r"(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:[^@\s/]+@",
        ])
        .expect("stderr privacy patterns are valid")
    })
}

/// Translate a known startup rejection without exposing arbitrary provider text,
/// which can contain prompts, credentials, and local paths.
pub(super) fn rejection_message(method: &str, error: &Value) -> &'static str {
    if method == "thread/resume"
        && error["code"] == -32600
        && error["message"]
            .as_str()
            .is_some_and(|message| message.contains("already has an active writer"))
    {
        "Cannot resume this Codex conversation because another Codex process has it open for writing. Release the conversation in the other Codex app or terminal, then restart this Wardian agent. If that app retains the conversation, quit it after saving other work. Do not delete the session or its writer lock."
    } else if method == "thread/resume"
        && error["code"] == -32601
        && error["message"].as_str() == Some("list_turns is not supported yet")
    {
        "This Codex runtime cannot resume the conversation's paginated history (list_turns is not supported yet). Restarting Wardian or closing another Codex app will not resolve this history compatibility error. The original conversation has been retained; use a compatible Codex runtime or a backed-up history recovery."
    } else {
        "provider rejected the request; original diagnostic remains provider-owned"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stale_steer_rejection_requires_exact_observed_form_and_requested_turn() {
        let params = json!({"expectedTurnId":"expected"});
        for message in [
            "no active turn to steer",
            "expected active turn id `expected` but found `other`",
        ] {
            let error = json!({"code":-32600,"message":message});
            assert!(stale_steer_rejection("turn/steer", &params, &error));
            assert!(!stale_steer_rejection("turn/start", &params, &error));
            assert!(!stale_steer_rejection(
                "thread/inject_items",
                &params,
                &error
            ));
            assert!(!stale_steer_rejection("turn/steer", &json!({}), &error));
            assert!(!stale_steer_rejection(
                "turn/steer",
                &json!({"expectedTurnId":""}),
                &error
            ));
        }
        for error in [
            json!({"code":-32600,"message":"other rejection"}),
            json!({"code":-32600,"message":"no active turn to steer "}),
            json!({"code":-32600,"message":"prefix no active turn to steer"}),
            json!({"code":-32600,"message":"expected active turn id `different` but found `other`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `expected`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found ``"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other` suffix"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other`extra`"}),
            json!({"code":-32600,"message":"expected active turn id `expected` but found `other\n`"}),
            json!({"code":-32603,"message":"no active turn to steer"}),
            json!({"code":"-32600","message":"no active turn to steer"}),
            json!({"code":-32600.0,"message":"no active turn to steer"}),
            json!({"code":-32600,"message":null}),
            json!({"code":-32600}),
            Value::Null,
        ] {
            assert!(
                !stale_steer_rejection("turn/steer", &params, &error),
                "{error}"
            );
        }
    }

    #[test]
    fn paginated_history_failure_is_distinct_from_writer_conflict() {
        let error = json!({"code":-32601,"message":"list_turns is not supported yet"});
        let message = rejection_message("thread/resume", &error);
        assert!(message.contains("paginated history"));
        assert!(message.contains("will not resolve"));
        assert!(!rejection_message("turn/start", &error).contains("paginated history"));
    }

    #[test]
    fn writer_conflict_explains_recovery_without_copying_provider_payload() {
        let error = json!({"code":-32600,"message":"thread private-id already has an active writer; secret-payload"});
        let message = rejection_message("thread/resume", &error);
        assert!(message.contains("another Codex process"));
        assert!(message.contains("restart this Wardian agent"));
        assert!(!message.contains("private-id"));
        assert!(!message.contains("secret-payload"));
        for (method, error) in [
            ("turn/start", error),
            (
                "thread/resume",
                json!({"code":-32600,"message":"other rejection secret-payload"}),
            ),
            (
                "thread/resume",
                json!({"code":-32603,"message":"already has an active writer"}),
            ),
            ("thread/resume", Value::Null),
        ] {
            assert_eq!(
                rejection_message(method, &error),
                "provider rejected the request; original diagnostic remains provider-owned"
            );
        }
    }

    #[test]
    fn stderr_diagnostic_redacts_private_paths_credentials_and_token_shapes() {
        let context = StderrRedactionContext::new(
            [std::path::PathBuf::from(
                r"C:\Users\operator\.wardian\workspace",
            )],
            [b"credential-value-123".to_vec()],
        );
        let sensitive = "[provider stderr omitted: potentially sensitive content]";

        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                br"failed in C:\Users\operator\.wardian\workspace\agent",
                false,
                false,
                &context,
            )
            .as_str(),
            Some(sensitive)
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                b"provider rejected credential-value-123",
                false,
                false,
                &context,
            )
            .as_str(),
            Some(sensitive)
        );
        let whitespace_split_credential = StderrRedactionContext::new(
            std::iter::empty::<std::path::PathBuf>(),
            [b"violet amber".to_vec()],
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                b"startup violet\namber",
                false,
                false,
                &whitespace_split_credential,
            )
            .as_str(),
            Some(sensitive)
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                b"provider returned sk-protectedcredential123456",
                false,
                false,
                &context,
            )
            .as_str(),
            Some(sensitive)
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                b"socket initialization failed",
                false,
                false,
                &context
            )
            .as_str(),
            Some("socket initialization failed")
        );
    }

    #[test]
    fn stderr_context_uses_command_credentials_and_fails_closed_when_incomplete() {
        let secret = "wardian-test-credential-9f1b";
        let mut command = std::process::Command::new("codex");
        command.env("CODEX_API_KEY", secret);
        let context = StderrRedactionContext::from_environment(
            std::iter::empty::<std::path::PathBuf>(),
            std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
            &command,
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                format!("startup echoed {secret}").as_bytes(),
                false,
                false,
                &context,
            )
            .as_str(),
            Some("[provider stderr omitted: potentially sensitive content]")
        );
        assert!(is_credential_env_name(OsStr::new("AWS_ACCESS_KEY_ID")));

        let short_secret = StderrRedactionContext::new(
            std::iter::empty::<std::path::PathBuf>(),
            [b"abc".to_vec()],
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(
                b"ordinary provider text",
                false,
                false,
                &short_secret
            )
            .as_str(),
            Some("[provider stderr omitted: potentially sensitive content]")
        );
    }

    #[test]
    fn stderr_diagnostic_omits_truncated_invalid_and_control_data() {
        let context = StderrRedactionContext::new(
            std::iter::empty::<std::path::PathBuf>(),
            std::iter::empty::<Vec<u8>>(),
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(b"useful prefix", true, false, &context).as_str(),
            Some("[provider stderr omitted: capture limit reached]")
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(&[0xff], false, false, &context).as_str(),
            Some("[provider stderr omitted: non-text or control data]")
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(b"first\x1bsecond", false, false, &context).as_str(),
            Some("[provider stderr omitted: non-text or control data]")
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(b"partial output", false, true, &context).as_str(),
            Some("[provider stderr omitted: capture failed]")
        );
    }

    #[test]
    fn stderr_diagnostic_collapses_whitespace_and_caps_safe_text() {
        let context = StderrRedactionContext::new(
            std::iter::empty::<std::path::PathBuf>(),
            std::iter::empty::<Vec<u8>>(),
        );
        assert_eq!(
            SafeStderrDiagnostic::from_bytes(b"  startup\n\terror  ", false, false, &context)
                .as_str(),
            Some("startup error")
        );
        let long_text = "a ".repeat(600);
        let diagnostic =
            SafeStderrDiagnostic::from_bytes(long_text.as_bytes(), false, false, &context);
        assert_eq!(
            diagnostic.as_str().unwrap().chars().count(),
            MAX_STDERR_DIAGNOSTIC_CHARS + 3
        );
        assert!(diagnostic.as_str().unwrap().ends_with("..."));
    }
}
