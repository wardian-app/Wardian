use crate::providers::ProviderFactory;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ProviderReadiness {
    pub provider: String,
    pub display_name: String,
    pub available: bool,
    pub executable: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
}

const USER_FACING_PROVIDER_DESCRIPTORS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: "claude",
        display_name: "Claude",
    },
    ProviderDescriptor {
        id: "codex",
        display_name: "Codex",
    },
    ProviderDescriptor {
        id: "gemini",
        display_name: "Gemini",
    },
    ProviderDescriptor {
        id: "antigravity",
        display_name: "antigravity",
    },
    ProviderDescriptor {
        id: "opencode",
        display_name: "OpenCode",
    },
];

pub fn user_facing_provider_descriptors() -> &'static [ProviderDescriptor] {
    USER_FACING_PROVIDER_DESCRIPTORS
}

pub fn list_provider_readiness() -> Vec<ProviderReadiness> {
    user_facing_provider_descriptors()
        .iter()
        .map(|descriptor| provider_readiness(descriptor.id))
        .collect()
}

pub fn provider_readiness(provider_id: &str) -> ProviderReadiness {
    let provider_id = provider_id.trim().to_ascii_lowercase();
    let display_name = provider_display_name(&provider_id);

    let Ok(provider) = ProviderFactory::resolve(&provider_id) else {
        return ProviderReadiness {
            provider: provider_id.clone(),
            display_name,
            available: false,
            executable: None,
            reason: Some(format!("Unknown provider '{provider_id}'.")),
        };
    };

    let (executable, base_args) = provider.get_executable();
    readiness_from_launch_parts(&provider_id, provider.name(), &executable, &base_args, None)
}

pub fn ensure_provider_available_for_launch(provider_id: &str) -> Result<(), String> {
    let readiness = provider_readiness(provider_id);
    if readiness.available {
        return Ok(());
    }

    Err(readiness.reason.unwrap_or_else(|| {
        format!(
            "{} is not available. See docs/guide/provider-readiness.md.",
            readiness.display_name
        )
    }))
}

pub fn readiness_from_launch_parts(
    provider_id: &str,
    display_name: &str,
    executable: &str,
    base_args: &[String],
    path_override: Option<&str>,
) -> ProviderReadiness {
    let executable = executable.trim();
    if executable.is_empty() {
        return unavailable(provider_id, display_name, "Provider executable is empty.");
    }

    let Some(resolved_executable) = resolve_executable(executable, path_override) else {
        return unavailable(
            provider_id,
            display_name,
            &format!(
                "{display_name} is not available because the {executable} command was not found in the Wardian app environment. See docs/guide/provider-readiness.md."
            ),
        );
    };

    if is_node_executable(executable) {
        if let Some(script_arg) = base_args.first() {
            if !script_arg.trim().is_empty() && !Path::new(script_arg).exists() {
                return unavailable(
                    provider_id,
                    display_name,
                    &format!(
                        "{display_name} requires {}, but that file was not found. See docs/guide/provider-readiness.md.",
                        script_arg
                    ),
                );
            }
        }
    }

    ProviderReadiness {
        provider: provider_id.to_string(),
        display_name: display_name.to_string(),
        available: true,
        executable: Some(resolved_executable.to_string_lossy().to_string()),
        reason: None,
    }
}

fn provider_display_name(provider_id: &str) -> String {
    user_facing_provider_descriptors()
        .iter()
        .find(|descriptor| descriptor.id == provider_id)
        .map(|descriptor| descriptor.display_name.to_string())
        .unwrap_or_else(|| provider_id.to_string())
}

fn unavailable(provider_id: &str, display_name: &str, reason: &str) -> ProviderReadiness {
    ProviderReadiness {
        provider: provider_id.to_string(),
        display_name: display_name.to_string(),
        available: false,
        executable: None,
        reason: Some(reason.to_string()),
    }
}

fn resolve_executable(executable: &str, path_override: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.is_absolute() || executable.contains('/') || executable.contains('\\') {
        return path.is_file().then(|| path.to_path_buf());
    }

    find_executable_on_path(executable, path_override)
}

fn find_executable_on_path(name: &str, path_override: Option<&str>) -> Option<PathBuf> {
    let candidate_names = executable_candidate_names(name);
    let path_value = path_override
        .map(std::ffi::OsString::from)
        .or_else(readiness_path_os)?;

    for directory in std::env::split_paths(&path_value) {
        for candidate_name in &candidate_names {
            let candidate = directory.join(candidate_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn readiness_path_os() -> Option<std::ffi::OsString> {
    #[cfg(target_os = "macos")]
    {
        return Some(std::ffi::OsString::from(macos_extended_path()));
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("PATH")
    }
}

#[cfg(target_os = "macos")]
fn macos_extended_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let existing = std::env::var("PATH").unwrap_or_default();
    let extra = format!(
        "{home}/.local/bin:/usr/local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:{home}/.npm-global/bin:{home}/.volta/bin",
        home = home
    );
    if existing.is_empty() {
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", extra)
    } else {
        format!("{}:{}", extra, existing)
    }
}

fn executable_candidate_names(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        if Path::new(name).extension().is_some() {
            return vec![name.to_string()];
        }

        let mut names = vec![name.to_string()];
        names.extend(
            executable_extensions()
                .into_iter()
                .map(|extension| format!("{name}{extension}")),
        );
        names
    }

    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

#[cfg(windows)]
fn executable_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .ok()
        .map(|value| {
            value
                .split(';')
                .filter_map(|segment| {
                    let trimmed = segment.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_ascii_lowercase())
                    }
                })
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()])
}

fn is_node_executable(executable: &str) -> bool {
    Path::new(executable)
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("node"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_facing_provider_order_is_claude_first() {
        let ids: Vec<_> = user_facing_provider_descriptors()
            .iter()
            .map(|provider| provider.id)
            .collect();

        assert_eq!(
            ids,
            vec!["claude", "codex", "gemini", "antigravity", "opencode"]
        );
    }

    #[test]
    fn antigravity_descriptor_uses_lowercase_user_label() {
        let descriptor = user_facing_provider_descriptors()
            .iter()
            .find(|provider| provider.id == "antigravity")
            .expect("antigravity descriptor");

        assert_eq!(descriptor.display_name, "antigravity");
    }

    #[test]
    fn missing_bare_executable_reports_unavailable() {
        let readiness = readiness_from_launch_parts(
            "codex",
            "Codex",
            "definitely-not-a-wardian-provider",
            &[],
            None,
        );

        assert!(!readiness.available);
        assert!(readiness.reason.unwrap().contains("not found"));
    }

    #[test]
    fn directory_named_like_executable_is_not_ready() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(temp.path().join("codex")).expect("codex dir");
        let path = temp.path().to_string_lossy();

        let readiness = readiness_from_launch_parts("codex", "Codex", "codex", &[], Some(&path));

        assert!(!readiness.available);
        assert!(readiness.reason.unwrap().contains("not found"));
    }
}
