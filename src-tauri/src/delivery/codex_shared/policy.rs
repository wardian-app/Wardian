//! One captured policy for the daemon, first thread loader and attachment check.
use super::{CodexSharedClient, CodexSharedError, Value, STARTUP_TIMEOUT};
use serde_json::json;
use std::path::Path;

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindowsSandboxMode {
    Elevated,
    Mxc,
    Unelevated,
}

impl WindowsSandboxMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Elevated => "elevated",
            Self::Mxc => "mxc",
            Self::Unelevated => "unelevated",
        }
    }

    fn parse(value: &str) -> Result<Self, CodexSharedError> {
        match value {
            "elevated" => Ok(Self::Elevated),
            "mxc" => Ok(Self::Mxc),
            "unelevated" => Ok(Self::Unelevated),
            _ => Err(CodexSharedError::unsupported(
                "effective Codex windows.sandbox choice is unsupported",
            )),
        }
    }
}

fn resolve_windows_sandbox(
    config_read: &Value,
    is_windows: bool,
    workspace_write: bool,
) -> Result<Option<WindowsSandboxMode>, CodexSharedError> {
    if !is_windows || !workspace_write {
        return Ok(None);
    }

    let config = config_read
        .get("config")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CodexSharedError::unsupported("Codex config/read omitted effective config")
        })?;
    let origins = config_read
        .get("origins")
        .and_then(Value::as_object)
        .ok_or_else(|| CodexSharedError::unsupported("Codex config/read omitted config origins"))?;

    let windows = match config.get("windows") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_object().ok_or_else(|| {
            CodexSharedError::unsupported("Codex config/read returned malformed windows config")
        })?),
    };
    let modern_value = windows.and_then(|value| value.get("sandbox"));
    let modern_origin = has_config_origin(origins, "windows.sandbox")?;
    let modern = match (modern_value, modern_origin) {
        (None | Some(Value::Null), false) => None,
        (Some(Value::String(value)), _) => Some(WindowsSandboxMode::parse(value)?),
        (None | Some(Value::Null), true) => {
            return Err(CodexSharedError::unsupported(
                "Codex config/read windows.sandbox origin has no effective value",
            ));
        }
        (Some(_), _) => {
            return Err(CodexSharedError::unsupported(
                "Codex config/read windows.sandbox value is malformed",
            ));
        }
    };

    const LEGACY_FEATURES: &[&str] = &[
        "experimental_windows_sandbox",
        "enable_experimental_windows_sandbox",
        "elevated_windows_sandbox",
        "prefer_mxc",
        "windows_sandbox_service",
    ];
    let features = match config.get("features") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_object().ok_or_else(|| {
            CodexSharedError::unsupported("Codex config/read returned malformed features config")
        })?),
    };
    for path in origins.keys() {
        if let Some(feature) = path.strip_prefix("features.") {
            if is_windows_sandbox_feature(feature) && !LEGACY_FEATURES.contains(&feature) {
                return Err(CodexSharedError::unsupported(
                    "Codex config/read contains an unsupported Windows sandbox feature origin",
                ));
            }
        }
    }
    if let Some(features) = features {
        for (feature, value) in features {
            if is_windows_sandbox_feature(feature)
                && !LEGACY_FEATURES.contains(&feature.as_str())
                && value != &Value::Bool(false)
                && !value.is_null()
            {
                return Err(CodexSharedError::unsupported(
                    "Codex config/read contains an unsupported Windows sandbox feature",
                ));
            }
        }
    }

    let prefer_mxc_origin = has_config_origin(origins, "features.prefer_mxc")?;
    let prefer_mxc = match features.and_then(|entries| entries.get("prefer_mxc")) {
        None | Some(Value::Null) if !prefer_mxc_origin => false,
        Some(Value::Bool(enabled)) => *enabled,
        None | Some(Value::Null) => {
            return Err(CodexSharedError::unsupported(
                "Codex prefer_mxc origin has no effective value",
            ));
        }
        Some(_) => {
            return Err(CodexSharedError::unsupported(
                "Codex prefer_mxc value is malformed",
            ));
        }
    };

    let mut legacy_choice_present = false;
    let mut experimental = false;
    let mut elevated = false;
    for feature in LEGACY_FEATURES {
        if *feature == "prefer_mxc" {
            continue;
        }
        let path = format!("features.{feature}");
        let origin = has_config_origin(origins, &path)?;
        let value = features.and_then(|entries| entries.get(*feature));
        let enabled = match value {
            None | Some(Value::Null) if !origin => false,
            Some(Value::Bool(enabled)) => *enabled,
            None | Some(Value::Null) => {
                return Err(CodexSharedError::unsupported(
                    "Codex Windows sandbox feature origin has no effective value",
                ));
            }
            Some(_) => {
                return Err(CodexSharedError::unsupported(
                    "Codex Windows sandbox feature value is malformed",
                ));
            }
        };
        legacy_choice_present |= origin || value.is_some();
        match *feature {
            "experimental_windows_sandbox" | "enable_experimental_windows_sandbox" => {
                experimental |= enabled;
            }
            "elevated_windows_sandbox" => elevated |= enabled,
            _ => {}
        }
    }

    let legacy = if elevated {
        Some(WindowsSandboxMode::Elevated)
    } else if experimental {
        Some(WindowsSandboxMode::Unelevated)
    } else {
        None
    };
    if modern == Some(WindowsSandboxMode::Mxc) {
        return Ok(modern);
    }
    if prefer_mxc {
        return Err(CodexSharedError::unsupported(
            "effective Windows MXC backend depends on runtime eligibility and cannot be pinned before workspace-write startup",
        ));
    }
    if modern.is_some() {
        return Ok(modern);
    }
    if legacy_choice_present {
        if legacy.is_none() {
            return Err(CodexSharedError::unsupported(
                "legacy Windows sandbox selection disables the backend and cannot be represented for workspace-write startup",
            ));
        }
        return Ok(legacy);
    }
    Ok(Some(WindowsSandboxMode::Unelevated))
}

fn is_windows_sandbox_feature(feature: &str) -> bool {
    feature.contains("windows_sandbox") || feature == "prefer_mxc"
}

fn has_config_origin(
    origins: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<bool, CodexSharedError> {
    match origins.get(path) {
        None => Ok(false),
        Some(metadata) if is_config_layer_metadata(metadata) => Ok(true),
        Some(_) => Err(CodexSharedError::unsupported(
            "Codex config/read returned malformed source origin metadata",
        )),
    }
}

fn is_config_layer_metadata(metadata: &Value) -> bool {
    let Some(metadata) = metadata.as_object() else {
        return false;
    };
    if metadata.get("version").and_then(Value::as_str).is_none() {
        return false;
    }
    let Some(source) = metadata.get("name").and_then(Value::as_object) else {
        return false;
    };
    let Some(source_type) = source.get("type").and_then(Value::as_str) else {
        return false;
    };
    let has_string = |key: &str| source.get(key).and_then(Value::as_str).is_some();
    match source_type {
        "packagedDefaults" | "system" | "legacyManagedConfigTomlFromFile" => has_string("file"),
        "mdm" => has_string("domain") && has_string("key"),
        "enterpriseManaged" => has_string("id") && has_string("name"),
        "user" => {
            has_string("file")
                && matches!(
                    source.get("profile"),
                    None | Some(Value::Null) | Some(Value::String(_))
                )
        }
        "project" => has_string("dotCodexFolder"),
        "sessionFlags" | "legacyManagedConfigTomlFromMdm" => true,
        _ => false,
    }
}

pub(super) struct ExpectedPolicy {
    fields: Vec<(String, String)>,
    windows_sandbox: Option<WindowsSandboxMode>,
}

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
        Ok(Self {
            fields,
            windows_sandbox: None,
        })
    }

    /// Direct choices survive stock resume permission filtering and retain
    /// ordinary local-daemon adoption. A reviewer preset injects opaque policy
    /// overrides, so reject it before home preparation, overlay or child spawn.
    /// Direct flags cannot replace a saved reviewer; attachment must confirm
    /// the captured user route before publishing a capable owner.
    pub(super) fn tui_permission_args(&self) -> Result<Vec<String>, CodexSharedError> {
        if self
            .fields
            .iter()
            .any(|(key, value)| key == "approvalsReviewer" && value != "user")
        {
            return Err(CodexSharedError::unsupported(
                "configured Codex approval reviewer has no supported ordinary local launch encoding",
            ));
        }
        let field = |key: &str| {
            self.fields
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
        if let Some((_, configured)) = self.fields.iter().find(|(key, _)| key == "model") {
            if configured != model {
                return Err(CodexSharedError::unsupported(
                    "resolved Codex launch model changed configured model",
                ));
            }
        } else {
            self.fields.push(("model".into(), model.into()));
        }
        Ok(())
    }

    /// Resolve the backend from the app-server's effective config response.
    /// The server owns layer precedence and legacy feature interpretation; this
    /// only translates reported values into the same per-thread/TUI override.
    pub(super) fn resolve_windows_sandbox(
        &mut self,
        config_read: &Value,
        is_windows: bool,
    ) -> Result<(), CodexSharedError> {
        self.windows_sandbox = resolve_windows_sandbox(
            config_read,
            is_windows,
            self.fields
                .iter()
                .any(|(key, value)| key == "sandbox" && value == "workspace-write"),
        )?;
        Ok(())
    }

    pub(super) fn append_windows_sandbox_arg(&self, args: &mut Vec<String>) {
        if let Some(mode) = self.windows_sandbox {
            args.extend([
                "-c".into(),
                format!(r#"windows.sandbox="{}""#, mode.as_str()),
                "-c".into(),
                "features.prefer_mxc=false".into(),
            ]);
        }
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
            .fields
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
        let mut config = serde_json::Map::new();
        for (key, value) in &self.fields {
            match key.as_str() {
                "model" | "approvalPolicy" | "approvalsReviewer" | "sandbox" => {
                    params[key] = json!(value)
                }
                "reasoningEffort" => {
                    config.insert("model_reasoning_effort".into(), json!(value));
                }
                _ => {}
            }
        }
        if let Some(mode) = self.windows_sandbox {
            config.insert("windows.sandbox".into(), json!(mode.as_str()));
            config.insert("features.prefer_mxc".into(), json!(false));
        }
        if !config.is_empty() {
            params["config"] = Value::Object(config);
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
        for (key, expected) in &self.fields {
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
