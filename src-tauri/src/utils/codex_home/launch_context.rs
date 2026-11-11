use crate::utils::shell::CodexRuntimePolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use wardian_core::models::AgentConfig;

use super::upstream::{resolve_upstream_home, UpstreamHome};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceFingerprint {
    path: PathBuf,
    exists: bool,
    sha256: Option<String>,
}

/// Private launch-time choices and source identities selected during durable
/// spawn preparation. Fingerprints are persisted, while source contents remain
/// only in the existing ephemeral preparation and projection paths.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CodexLaunchContext {
    wardian_home: PathBuf,
    upstream: UpstreamHome,
    policy: CodexRuntimePolicy,
    effective_policy: CodexRuntimePolicy,
    sources: Vec<SourceFingerprint>,
    auth_environment: Vec<(String, String)>,
    #[serde(skip_serializing)]
    contents: Vec<(String, Option<Vec<u8>>)>,
}

impl std::fmt::Debug for CodexLaunchContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexLaunchContext")
            .field("sources", &self.sources)
            .field("effective_policy", &self.effective_policy)
            .finish_non_exhaustive()
    }
}

impl CodexLaunchContext {
    pub(crate) fn capture(config: &AgentConfig) -> Result<Self, String> {
        let (policy, settings_path, settings_digest) =
            crate::utils::shell::load_codex_runtime_policy_source()?;
        let effective_policy = crate::providers::codex::effective_codex_runtime_policy(
            &config.codex_config(),
            &policy,
        );
        Self::capture_selected(policy, effective_policy, settings_path, settings_digest)
    }

    fn capture_selected(
        policy: CodexRuntimePolicy,
        effective_policy: CodexRuntimePolicy,
        settings_path: PathBuf,
        settings_digest: Option<String>,
    ) -> Result<Self, String> {
        let wardian_home = crate::utils::get_wardian_home()
            .ok_or_else(|| "Wardian home is unavailable".to_string())?;
        let wardian_home = canonical_or_original(&wardian_home)?;
        let upstream = resolve_upstream_home()?;
        let mut sources = vec![SourceFingerprint {
            path: settings_path,
            exists: settings_digest.is_some(),
            sha256: settings_digest,
        }];
        let mut contents = Vec::new();
        for name in ["config.toml", "auth.json", "cap_sid"] {
            let (fingerprint, bytes) = read_source(&upstream.path.join(name))?;
            if name == "config.toml" {
                let text = std::str::from_utf8(bytes.as_deref().unwrap_or_default())
                    .map_err(|_| "Codex launch config is not readable")?;
                text.parse::<toml_edit::DocumentMut>()
                    .map_err(|_| "Codex launch config is malformed")?;
            } else if name == "auth.json" {
                if let Some(bytes) = bytes.as_deref() {
                    let auth: serde_json::Value = serde_json::from_slice(bytes)
                        .map_err(|_| "Codex launch auth source is malformed")?;
                    if !auth.is_object() {
                        return Err("Codex launch auth source is malformed".into());
                    }
                }
            }
            sources.push(fingerprint);
            contents.push((name.to_string(), bytes));
        }
        let auth_environment = [
            "CODEX_API_KEY",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_ORG_ID",
            "OPENAI_PROJECT_ID",
        ]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_string(), value)))
        .map(|(name, value)| (name, fingerprint_os_string(&value)))
        .collect();
        Ok(Self {
            wardian_home,
            upstream,
            policy,
            effective_policy,
            sources,
            auth_environment,
            contents,
        })
    }

    /// Re-read the selected authorities immediately before an external start.
    /// Failure is deliberately generic so private paths and fingerprints never
    /// enter operator-facing diagnostics.
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        let (policy, settings_path, settings_digest) =
            crate::utils::shell::load_codex_runtime_policy_source()
                .map_err(|_| "Codex launch policy or source is no longer readable".to_string())?;
        let current = Self::capture_selected(
            policy,
            self.effective_policy.clone(),
            settings_path,
            settings_digest,
        )
        .map_err(|_| "Codex launch policy or source is no longer readable".to_string())?;
        if current == *self {
            Ok(())
        } else {
            Err("Codex launch policy or source changed during preparation".to_string())
        }
    }

    pub(crate) fn effective_policy(&self) -> &CodexRuntimePolicy {
        &self.effective_policy
    }

    pub(crate) fn upstream(&self) -> &UpstreamHome {
        &self.upstream
    }

    /// Bytes and fingerprints come from the same bounded read during preparing.
    /// Projection must use these bytes while launch revalidates their authority.
    pub(crate) fn source_contents(&self, name: &str) -> Option<&[u8]> {
        self.contents
            .iter()
            .find(|(source, _)| source == name)
            .and_then(|(_, bytes)| bytes.as_deref())
    }

    pub(crate) fn descriptor_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|_| "could not encode private launch descriptor".into())
    }
}

fn canonical_or_original(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(path.to_path_buf())
            } else {
                Err(error)
            }
        })
        .map_err(|_| "Wardian home is not readable".to_string())
}

fn read_source(path: &Path) -> Result<(SourceFingerprint, Option<Vec<u8>>), String> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path.symlink_metadata().is_ok() {
                return Err("Codex launch source link is unresolved".to_string());
            }
            return Ok((
                SourceFingerprint {
                    path: path.to_path_buf(),
                    exists: false,
                    sha256: None,
                },
                None,
            ));
        }
        Err(_) => return Err("Codex launch source is not readable".to_string()),
        Ok(metadata) if !metadata.is_file() => {
            return Err("Codex launch source is not a regular file".to_string());
        }
        Ok(_) => {}
    }
    let source_path =
        std::fs::canonicalize(path).map_err(|_| "Codex launch source is not readable")?;
    let metadata =
        std::fs::metadata(&source_path).map_err(|_| "Codex launch source is not readable")?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("Codex launch source is not a bounded regular file".to_string());
    }
    let mut file =
        std::fs::File::open(&source_path).map_err(|_| "Codex launch source is not readable")?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&mut file, 16 * 1024 * 1024 + 1),
        &mut bytes,
    )
    .map_err(|_| "Codex launch source is not readable")?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Codex launch source exceeds its bound".into());
    }
    let fingerprint = SourceFingerprint {
        path: source_path,
        exists: true,
        sha256: Some(format!("{:x}", Sha256::digest(&bytes))),
    };
    Ok((fingerprint, Some(bytes)))
}

fn fingerprint_os_string(value: &std::ffi::OsStr) -> String {
    let mut hasher = Sha256::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(value.as_bytes());
    };
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for unit in value.encode_wide() {
            hasher.update(unit.to_le_bytes());
        }
    };
    #[cfg(not(any(unix, windows)))]
    hasher.update(value.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::shell::{CodexRuntimePolicy, ShellSettings};
    use std::ffi::OsString;

    struct EnvironmentFixture {
        _temp: tempfile::TempDir,
        previous: Vec<(&'static str, Option<OsString>)>,
        _guard: tokio::sync::MutexGuard<'static, ()>,
    }

    impl EnvironmentFixture {
        fn new() -> Self {
            let guard = crate::utils::wardian_test_env_lock();
            let temp = tempfile::tempdir().unwrap();
            let wardian = temp.path().join("wardian");
            let upstream = temp.path().join("codex");
            std::fs::create_dir_all(wardian.join("settings")).unwrap();
            std::fs::create_dir_all(&upstream).unwrap();
            let previous = [
                "WARDIAN_HOME",
                "CODEX_HOME",
                "CODEX_API_KEY",
                "OPENAI_API_KEY",
            ]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
            unsafe {
                std::env::set_var("WARDIAN_HOME", &wardian);
                std::env::set_var("CODEX_HOME", &upstream);
                std::env::set_var("OPENAI_API_KEY", "fixture-secret-value");
                std::env::remove_var("CODEX_API_KEY");
            }
            let settings = ShellSettings {
                codex_runtime_policy: CodexRuntimePolicy {
                    sandbox_mode: "read-only".into(),
                    approval_policy: "on-request".into(),
                    full_auto: false,
                    trust_workspaces: true,
                },
                ..Default::default()
            };
            std::fs::write(
                wardian.join("settings").join("shell.json"),
                serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
            std::fs::write(upstream.join("config.toml"), "model = 'gpt-test'\n").unwrap();
            std::fs::write(
                upstream.join("auth.json"),
                br#"{"tokens":{"access_token":"fixture-file-secret"}}"#,
            )
            .unwrap();
            Self {
                _temp: temp,
                previous,
                _guard: guard,
            }
        }
    }

    impl Drop for EnvironmentFixture {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                unsafe {
                    if let Some(value) = value {
                        std::env::set_var(name, value);
                    } else {
                        std::env::remove_var(name);
                    }
                }
            }
        }
    }

    #[test]
    fn capture_pins_policy_and_selected_auth_sources_without_persisting_secrets() {
        let fixture = EnvironmentFixture::new();
        let config = AgentConfig {
            provider: "codex".into(),
            ..Default::default()
        };
        let context = CodexLaunchContext::capture(&config).unwrap();
        context.revalidate().unwrap();
        assert_eq!(context.effective_policy().sandbox_mode, "read-only");
        let descriptor = context.descriptor_json().unwrap();
        assert!(!descriptor.contains("fixture-secret-value"));
        assert!(!descriptor.contains("fixture-file-secret"));

        let upstream = fixture._temp.path().join("codex");
        std::fs::write(
            upstream.join("auth.json"),
            br#"{"tokens":{"access_token":"rotated-file-secret"}}"#,
        )
        .unwrap();
        assert!(context.revalidate().is_err());

        let context = CodexLaunchContext::capture(&config).unwrap();
        let settings = fixture._temp.path().join("wardian/settings/shell.json");
        std::fs::write(
            settings,
            serde_json::to_vec(&ShellSettings {
                codex_runtime_policy: CodexRuntimePolicy {
                    sandbox_mode: "workspace-write".into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert!(context.revalidate().is_err());
    }

    #[test]
    fn malformed_policy_source_fails_closed() {
        let fixture = EnvironmentFixture::new();
        let config = AgentConfig {
            provider: "codex".into(),
            ..Default::default()
        };
        let settings = fixture._temp.path().join("wardian/settings/shell.json");
        std::fs::write(settings, "{").unwrap();
        assert!(CodexLaunchContext::capture(&config).is_err());
    }

    #[test]
    fn selected_contents_stay_bound_and_rotated_authority_stops_projection() {
        let fixture = EnvironmentFixture::new();
        let config = AgentConfig {
            provider: "codex".into(),
            ..Default::default()
        };
        let context = CodexLaunchContext::capture(&config).unwrap();
        let auth = context.source_contents("auth.json").unwrap().to_vec();
        let config_bytes = context.source_contents("config.toml").unwrap().to_vec();
        let upstream = fixture._temp.path().join("codex");
        std::fs::write(
            upstream.join("auth.json"),
            br#"{"tokens":{"access_token":"rotated"}}"#,
        )
        .unwrap();
        std::fs::write(upstream.join("config.toml"), "model = 'changed'\n").unwrap();
        assert_eq!(context.source_contents("auth.json").unwrap(), auth);
        assert_eq!(
            context.source_contents("config.toml").unwrap(),
            config_bytes
        );
        assert!(!format!("{context:?}").contains("fixture-file-secret"));
        assert!(!context
            .descriptor_json()
            .unwrap()
            .contains("fixture-file-secret"));
        let projected = fixture._temp.path().join("projected");
        std::fs::create_dir_all(&projected).unwrap();
        std::fs::write(projected.join("auth.json"), "keep until authorized").unwrap();
        let result = crate::utils::fs::sync_codex_agent_home_with_context(
            &upstream,
            &projected,
            &fixture._temp.path().join("skills"),
            &context,
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(projected.join("auth.json")).unwrap(),
            "keep until authorized"
        );
    }

    #[test]
    fn malformed_config_and_auth_are_rejected_during_preparation() {
        let fixture = EnvironmentFixture::new();
        let config = AgentConfig {
            provider: "codex".into(),
            ..Default::default()
        };
        let upstream = fixture._temp.path().join("codex");
        std::fs::write(upstream.join("config.toml"), "model = [").unwrap();
        assert!(CodexLaunchContext::capture(&config).is_err());
        std::fs::write(upstream.join("config.toml"), "").unwrap();
        std::fs::write(upstream.join("auth.json"), "{").unwrap();
        assert!(CodexLaunchContext::capture(&config).is_err());
    }

    #[test]
    fn projection_uses_selected_contents_and_clears_absent_credential_sources() {
        let fixture = EnvironmentFixture::new();
        let config = AgentConfig {
            provider: "codex".into(),
            ..Default::default()
        };
        let context = CodexLaunchContext::capture(&config).unwrap();
        let projected = fixture._temp.path().join("projected");
        std::fs::create_dir_all(&projected).unwrap();
        std::fs::write(projected.join("cap_sid"), "old account").unwrap();
        crate::utils::fs::sync_codex_agent_home_with_context(
            &context.upstream().path,
            &projected,
            &fixture._temp.path().join("skills"),
            &context,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(projected.join("auth.json")).unwrap(),
            context.source_contents("auth.json").unwrap()
        );
        assert!(!projected.join("cap_sid").exists());
        assert!(std::fs::read_to_string(projected.join("config.toml"))
            .unwrap()
            .contains("gpt-test"));
        std::fs::remove_file(context.upstream().path.join("auth.json")).unwrap();
        let environment_auth = CodexLaunchContext::capture(&config).unwrap();
        crate::utils::fs::sync_codex_agent_home_with_context(
            &environment_auth.upstream().path,
            &projected,
            &fixture._temp.path().join("skills"),
            &environment_auth,
        )
        .unwrap();
        assert!(!projected.join("auth.json").exists());
    }
}
