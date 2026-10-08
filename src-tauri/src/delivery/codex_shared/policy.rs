//! One captured policy for the daemon, first thread loader and attachment check.
use super::{CodexSharedClient, CodexSharedError, Value, STARTUP_TIMEOUT};
use serde_json::json;
use std::path::Path;

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;

pub(super) struct ExpectedPolicy(Vec<(String, String)>);

impl ExpectedPolicy {
    pub(super) fn from_server_args(args: &[String]) -> Result<Self, CodexSharedError> {
        let mut fields = Vec::new();
        for pair in args.windows(2).filter(|pair| pair[0] == "-c") {
            let document: toml_edit::DocumentMut = pair[1]
                .parse()
                .map_err(|_| CodexSharedError::unsupported("invalid generated server policy"))?;
            for (key, response_key) in [
                ("model", "model"),
                ("approval_policy", "approvalPolicy"),
                ("approvals_reviewer", "approvalsReviewer"),
                ("sandbox_mode", "sandbox"),
                ("model_reasoning_effort", "reasoningEffort"),
            ] {
                if let Some(value) = document.get(key).and_then(toml_edit::Item::as_str) {
                    fields.push((response_key.to_owned(), value.to_owned()));
                }
            }
        }
        // Supported Wardian policies use stock's `user` review route. Capture
        // it explicitly because an omitted reviewer can inherit saved settings.
        if !fields.iter().any(|(key, _)| key == "approvalsReviewer") {
            fields.push(("approvalsReviewer".into(), "user".into()));
        }
        Ok(Self(fields))
    }

    /// Direct choices survive stock resume permission filtering and retain
    /// ordinary local-daemon adoption. A reviewer preset injects opaque policy
    /// overrides, so reject it before home preparation, overlay or child spawn.
    /// Direct flags cannot replace a saved reviewer; attachment must confirm
    /// the captured user route before publishing a capable owner.
    pub(super) fn tui_permission_args(&self) -> Result<Vec<String>, CodexSharedError> {
        if self
            .0
            .iter()
            .any(|(key, value)| key == "approvalsReviewer" && value != "user")
        {
            return Err(CodexSharedError::unsupported(
                "configured Codex approval reviewer has no supported ordinary local launch encoding",
            ));
        }
        let field = |key: &str| {
            self.0
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let sandbox = field("sandbox")
            .filter(|value| {
                matches!(
                    *value,
                    "read-only" | "workspace-write" | "danger-full-access"
                )
            })
            .ok_or_else(|| {
                CodexSharedError::unsupported(
                    "configured Codex sandbox is unavailable or unsupported",
                )
            })?;
        let approval = field("approvalPolicy")
            .filter(|value| matches!(*value, "on-request" | "never"))
            .ok_or_else(|| {
                CodexSharedError::unsupported(
                    "configured Codex approval policy is unavailable or unsupported",
                )
            })?;
        if sandbox == "danger-full-access" && approval == "never" {
            Ok(vec!["--dangerously-bypass-approvals-and-sandbox".into()])
        } else {
            Ok(vec![
                "--sandbox".into(),
                sandbox.into(),
                "--ask-for-approval".into(),
                approval.into(),
            ])
        }
    }

    /// Validate transient model resolution without changing saved settings.
    pub(super) fn expect_launch_model(
        &mut self,
        model: Option<&str>,
    ) -> Result<(), CodexSharedError> {
        let Some(model) = model else { return Ok(()) };
        if let Some((_, configured)) = self.0.iter().find(|(key, _)| key == "model") {
            if configured != model {
                return Err(CodexSharedError::unsupported(
                    "resolved Codex launch model changed configured model",
                ));
            }
        } else {
            self.0.push(("model".into(), model.into()));
        }
        Ok(())
    }

    /// Cold loads must carry policy as explicit per-thread parameters. Daemon
    /// config defaults alone do not override a resumed thread's saved policy.
    pub(super) fn background_resume_params(&self, thread_id: &str) -> Value {
        self.thread_params(json!({"threadId": thread_id}))
    }

    /// Observe the saved reviewer only during private initial preparation.
    /// Ordinary cold background loads keep the explicit captured reviewer.
    pub(super) fn initial_resume_params(&self, thread_id: &str) -> Value {
        let mut params = self.background_resume_params(thread_id);
        params
            .as_object_mut()
            .expect("thread parameters")
            .remove("approvalsReviewer");
        params
    }

    pub(super) fn initial_reviewer_needs_update(
        &self,
        response: &Value,
    ) -> Result<bool, CodexSharedError> {
        self.validate_fields(response, true)?;
        if !self
            .0
            .iter()
            .any(|(key, value)| key == "approvalsReviewer" && value == "user")
        {
            return Err(CodexSharedError::unsupported(
                "initial checkpoint requires captured user reviewer",
            ));
        }
        match response["approvalsReviewer"].as_str() {
            Some("user") => Ok(false),
            Some("auto_review") => Ok(true),
            _ => Err(CodexSharedError::unsupported(
                "saved Codex reviewer is unsupported for initial checkpoint",
            )),
        }
    }

    fn thread_params(&self, mut params: Value) -> Value {
        for (key, value) in &self.0 {
            match key.as_str() {
                "model" | "approvalPolicy" | "approvalsReviewer" | "sandbox" => {
                    params[key] = json!(value)
                }
                "reasoningEffort" => params["config"] = json!({"model_reasoning_effort": value}),
                _ => {}
            }
        }
        params
    }

    /// Exercise the same parameter producer at both cold background boundaries.
    /// Interactive attachment stays an ID-only rejoin of the TUI-loaded thread.
    pub(super) async fn load_background_thread(
        &self,
        client: &CodexSharedClient,
        resume_id: Option<&str>,
        workspace: &Path,
    ) -> Result<Value, CodexSharedError> {
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            client
                .resume_metadata(self.background_resume_params(id))
                .await
        } else {
            client
                .request_with_timeout(
                    "thread/start",
                    self.thread_params(json!({"cwd": workspace})),
                    STARTUP_TIMEOUT,
                )
                .await
        }
    }

    pub(super) fn validate(&self, response: &Value) -> Result<(), CodexSharedError> {
        self.validate_fields(response, false)
    }

    fn validate_fields(
        &self,
        response: &Value,
        omit_reviewer: bool,
    ) -> Result<(), CodexSharedError> {
        for (key, expected) in &self.0 {
            if omit_reviewer && key == "approvalsReviewer" {
                continue;
            }
            let expected = if key == "sandbox" {
                match expected.as_str() {
                    "danger-full-access" => "dangerFullAccess",
                    "workspace-write" => "workspaceWrite",
                    "read-only" => "readOnly",
                    value => value,
                }
            } else {
                expected.as_str()
            };
            let actual = if key == "sandbox" {
                &response[key]["type"]
            } else {
                &response[key]
            };
            if actual.as_str() != Some(expected) {
                let detail = if matches!(
                    key.as_str(),
                    "sandbox" | "approvalPolicy" | "approvalsReviewer"
                ) {
                    format!(
                        "; expected={}, actual={}",
                        safe_policy_value(Some(expected)),
                        safe_policy_value(actual.as_str())
                    )
                } else {
                    String::new()
                };
                return Err(CodexSharedError::unsupported(format!(
                    "local Codex thread changed configured {key}; attachment rejected{detail}"
                )));
            }
        }
        Ok(())
    }
}

/// Only fixed policy words cross into diagnostics; never echo arbitrary RPC text.
fn safe_policy_value(value: Option<&str>) -> &'static str {
    match value {
        Some("dangerFullAccess") => "dangerFullAccess",
        Some("workspaceWrite") => "workspaceWrite",
        Some("readOnly") => "readOnly",
        Some("externalSandbox") => "externalSandbox",
        Some("on-request") => "on-request",
        Some("never") => "never",
        Some("untrusted") => "untrusted",
        Some("user") => "user",
        Some("guardian_subagent") => "guardian_subagent",
        Some("auto_review") => "auto_review",
        Some(_) => "[unrecognized]",
        None => "[missing-or-non-string]",
    }
}
