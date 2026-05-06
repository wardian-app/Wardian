use serde::Serialize;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    Generic = 1,
    NotFound = 2,
    NotInSession = 3,
    DbUnavailable = 4,
    Ambiguous = 5,
    AppNotRunning = 6,
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope<'a> {
    pub schema: u8,
    pub error: ErrorBody<'a>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub code: &'a str,
    pub message: String,
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<serde_json::Value>>,
}

#[derive(Debug)]
pub struct CliError {
    pub exit_code: ExitCode,
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub details: Option<Box<serde_json::Value>>,
}

impl CliError {
    pub fn not_in_session() -> Self {
        Self {
            exit_code: ExitCode::NotInSession,
            code: "not_in_session",
            message: "WARDIAN_SESSION_ID environment variable is not set".to_string(),
            hint: Some(
                "Pass a name or uuid to look up a specific agent from outside a Wardian-managed agent process: `wardian agent <name>`."
                    .to_string(),
            ),
            details: Some(Box::new(serde_json::json!({
                "command": "agent",
                "requested": "self",
            }))),
        }
    }

    pub fn not_found(requested: &str) -> Self {
        Self {
            exit_code: ExitCode::NotFound,
            code: "not_found",
            message: format!("Agent was not found: {requested}"),
            hint: Some("Run `wardian agent list --scope all` to inspect known agents.".to_string()),
            details: Some(Box::new(serde_json::json!({ "requested": requested }))),
        }
    }

    pub fn db_unavailable(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::DbUnavailable,
            code: "db_unavailable",
            message: message.into(),
            hint: Some(
                "Open Wardian once or set WARDIAN_HOME to a directory containing state.db."
                    .to_string(),
            ),
            details: None,
        }
    }

    pub fn invalid_field(field: &str) -> Self {
        Self {
            exit_code: ExitCode::Generic,
            code: "invalid_field",
            message: format!("Unknown field: {field}"),
            hint: Some("Use one of: name, uuid, class, provider, workspace, status, status_source, pid, started_at, last_status_at.".to_string()),
            details: Some(Box::new(serde_json::json!({ "field": field }))),
        }
    }

    pub fn app_not_running() -> Self {
        Self {
            exit_code: ExitCode::AppNotRunning,
            code: "app_not_running",
            message: "Wardian is not running. Start the app to use this command.".to_string(),
            hint: Some("Launch Wardian, then retry.".to_string()),
            details: None,
        }
    }

    pub fn generic(message: impl Into<String>) -> Self {
        Self {
            exit_code: ExitCode::Generic,
            code: "generic",
            message: message.into(),
            hint: None,
            details: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&ErrorEnvelope {
            schema: 1,
            error: ErrorBody {
                code: self.code,
                message: self.message.clone(),
                hint: self.hint.clone(),
                details: self.details.clone(),
            },
        })
        .expect("error envelope should serialize")
    }

    pub fn emit(&self) {
        eprintln!("{}", self.to_json());
    }

    pub fn code_i32(&self) -> i32 {
        self.exit_code as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_in_session_error_serializes_envelope() {
        let error = CliError::not_in_session();
        let json = error.to_json();
        assert!(json.contains(r#""schema":1"#));
        assert!(json.contains(r#""code":"not_in_session""#));
        assert_eq!(error.code_i32(), 3);
    }

    #[test]
    fn not_found_error_uses_exit_two() {
        let error = CliError::not_found("ghost");
        assert_eq!(error.code_i32(), 2);
        assert!(error.to_json().contains("ghost"));
    }
}
