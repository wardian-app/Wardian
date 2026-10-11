//! Decides whether a Claude launch may export telemetry to Wardian.
//!
//! Wardian points Claude's OpenTelemetry log exporter at its loopback
//! receiver only when no telemetry configuration of the user's, the
//! repository's, or the organization's is in effect. A configured collector,
//! endpoint, header, certificate, or opt-out is preserved untouched, and the
//! launch reports verified completions as unavailable instead.

use std::path::{Path, PathBuf};

/// Telemetry variables Claude Code reads (mirrors its exporter key list),
/// matched case-insensitively. Any one of them means telemetry is already
/// someone else's decision.
const TELEMETRY_KEYS: &[&str] = &[
    "CLAUDE_CODE_ENABLE_TELEMETRY",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA",
    "ENABLE_ENHANCED_TELEMETRY_BETA",
    "ENABLE_BETA_TRACING_DETAILED",
    "BETA_TRACING_ENDPOINT",
];
const TELEMETRY_PREFIXES: &[&str] = &["OTEL_", "ANT_OTEL_", "CLAUDE_CODE_OTEL_"];
/// Settings keys that configure telemetry outside the `env` block.
const TELEMETRY_SETTINGS: &[&str] = &["otelHeadersHelper"];

pub(crate) fn is_telemetry_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    TELEMETRY_KEYS.contains(&key.as_str())
        || TELEMETRY_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// Where existing telemetry configuration was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExistingTelemetry {
    ProcessEnvironment,
    Settings(PathBuf),
    UnreadableSettings(PathBuf),
    ManagedPolicy,
}

impl ExistingTelemetry {
    /// A description safe to log: it names the source, never values.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::ProcessEnvironment => "Wardian's environment sets telemetry variables".into(),
            Self::Settings(path) => format!("{} configures telemetry", path.display()),
            Self::UnreadableSettings(path) => {
                format!("{} could not be read to rule out telemetry", path.display())
            }
            Self::ManagedPolicy => "a managed Claude Code policy is installed".into(),
        }
    }
}

/// Returns the first telemetry configuration that would apply to a Claude
/// process started in `project_dir`, or `None` when there is none.
pub(crate) fn existing_telemetry(project_dir: &Path) -> Option<ExistingTelemetry> {
    if std::env::vars_os().any(|(key, _)| key.to_str().is_some_and(is_telemetry_env_key)) {
        return Some(ExistingTelemetry::ProcessEnvironment);
    }
    if managed_policy_installed() {
        return Some(ExistingTelemetry::ManagedPolicy);
    }
    settings_files(project_dir)
        .into_iter()
        .find_map(|path| settings_file_telemetry(&path))
}

fn settings_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut files = managed_settings_files();
    if let Some(config_dir) = claude_config_dir() {
        files.push(config_dir.join("settings.json"));
    }
    let project = project_dir.join(".claude");
    files.push(project.join("settings.json"));
    files.push(project.join("settings.local.json"));
    files
}

/// Claude's user configuration directory, honouring `CLAUDE_CONFIG_DIR`.
fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

fn managed_settings_root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

fn managed_settings_files() -> Vec<PathBuf> {
    let root = managed_settings_root();
    let mut files = vec![root.join("managed-settings.json")];
    if let Ok(entries) = std::fs::read_dir(root.join("managed-settings.d")) {
        let mut fragments = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        fragments.sort();
        files.extend(fragments);
    }
    files
}

/// Registry and MDM policy carry settings Wardian does not parse; their mere
/// presence keeps telemetry under the organization's control.
fn managed_policy_installed() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
            .into_iter()
            .any(|hive| {
                winreg::RegKey::predef(hive)
                    .open_subkey(r"SOFTWARE\Policies\ClaudeCode")
                    .is_ok()
            })
    }
    #[cfg(target_os = "macos")]
    {
        Path::new("/Library/Managed Preferences/com.anthropic.claudecode.plist").exists()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

fn settings_file_telemetry(path: &Path) -> Option<ExistingTelemetry> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(ExistingTelemetry::UnreadableSettings(path.to_path_buf())),
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Some(ExistingTelemetry::UnreadableSettings(path.to_path_buf()));
    };
    settings_configure_telemetry(&settings).then(|| ExistingTelemetry::Settings(path.to_path_buf()))
}

/// Whether one Claude settings document configures telemetry.
pub(crate) fn settings_configure_telemetry(settings: &serde_json::Value) -> bool {
    let env_keys = settings
        .get("env")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|env| env.keys().any(|key| is_telemetry_env_key(key)));
    env_keys
        || TELEMETRY_SETTINGS
            .iter()
            .any(|key| settings.get(*key).is_some())
}

/// The `env` block Wardian adds to a launch's `--settings` when it owns
/// telemetry: metadata-only events to `endpoint`, metrics and traces off,
/// and no prompt, response, or tool content.
pub(crate) fn loopback_telemetry_env(endpoint: &str) -> serde_json::Map<String, serde_json::Value> {
    [
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
        ("OTEL_LOGS_EXPORTER", "otlp"),
        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json"),
        ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", endpoint),
        ("OTEL_LOGS_EXPORT_INTERVAL", "1000"),
        ("OTEL_METRICS_EXPORTER", "none"),
        ("OTEL_TRACES_EXPORTER", "none"),
    ]
    .into_iter()
    .map(|(key, value)| {
        (
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        )
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_keys_cover_exporters_endpoints_headers_and_opt_outs() {
        for key in [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "otel_exporter_otlp_headers",
            "OTEL_EXPORTER_OTLP_LOGS_CLIENT_CERTIFICATE",
            "OTEL_LOGS_EXPORTER",
            "OTEL_LOG_USER_PROMPTS",
            "CLAUDE_CODE_ENABLE_TELEMETRY",
            "CLAUDE_CODE_OTEL_HEADERS_HELPER_DEBOUNCE_MS",
            "ANT_OTEL_EXPORTER_OTLP_ENDPOINT",
        ] {
            assert!(is_telemetry_env_key(key), "{key}");
        }
        for key in ["ANTHROPIC_MODEL", "CLAUDE_CODE_USE_BEDROCK", "HOME"] {
            assert!(!is_telemetry_env_key(key), "{key}");
        }
    }

    #[test]
    fn settings_with_telemetry_env_or_headers_helper_are_existing_configuration() {
        assert!(settings_configure_telemetry(&serde_json::json!({
            "env": { "OTEL_LOGS_EXPORTER": "none" }
        })));
        assert!(settings_configure_telemetry(&serde_json::json!({
            "otelHeadersHelper": "/bin/headers"
        })));
        assert!(!settings_configure_telemetry(&serde_json::json!({
            "env": { "ANTHROPIC_MODEL": "opus" },
            "hooks": {}
        })));
    }

    #[test]
    fn unreadable_project_settings_cannot_rule_out_telemetry() {
        let root = tempfile::tempdir().expect("temp dir");
        let settings = root.path().join(".claude");
        std::fs::create_dir_all(&settings).expect("settings dir");
        std::fs::write(settings.join("settings.local.json"), "{ not json").expect("write");
        assert_eq!(
            settings_file_telemetry(&settings.join("settings.local.json")),
            Some(ExistingTelemetry::UnreadableSettings(
                settings.join("settings.local.json")
            ))
        );
        assert_eq!(
            settings_file_telemetry(&settings.join("settings.json")),
            None
        );
    }

    #[test]
    fn loopback_env_exports_only_logs_without_content() {
        let env = loopback_telemetry_env("http://127.0.0.1:4318/claude-telemetry/token/v1/logs");
        assert_eq!(env["OTEL_EXPORTER_OTLP_LOGS_PROTOCOL"], "http/json");
        assert_eq!(env["OTEL_METRICS_EXPORTER"], "none");
        assert_eq!(env["OTEL_TRACES_EXPORTER"], "none");
        for content_flag in [
            "OTEL_LOG_USER_PROMPTS",
            "OTEL_LOG_ASSISTANT_RESPONSES",
            "OTEL_LOG_TOOL_DETAILS",
            "OTEL_LOG_TOOL_CONTENT",
            "OTEL_LOG_RAW_API_BODIES",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
        ] {
            assert!(!env.contains_key(content_flag), "{content_flag}");
        }
    }
}
