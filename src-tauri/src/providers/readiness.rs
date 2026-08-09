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
        display_name: "Antigravity",
    },
    ProviderDescriptor {
        id: "opencode",
        display_name: "OpenCode",
    },
    ProviderDescriptor {
        id: "prime",
        display_name: "Prime Agent",
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
    let readiness =
        readiness_from_launch_parts(&provider_id, &display_name, &executable, &base_args, None);

    if provider_id == "prime" && readiness.available {
        if let Some(reason) = prime_kernel_blocker() {
            return blocked_by_runtime(readiness, reason);
        }
    }

    readiness
}

/// Marks a provider whose CLI resolved but whose runtime dependency is missing.
///
/// The executable is kept deliberately. It is the signal that the CLI was found
/// and something else blocks the launch, which is what stops the UI from
/// telling a user to reinstall software they already have.
fn blocked_by_runtime(readiness: ProviderReadiness, reason: String) -> ProviderReadiness {
    ProviderReadiness {
        available: false,
        reason: Some(reason),
        ..readiness
    }
}

/// Prime Agent's only tool is an IPython kernel, so a resolvable binary with no
/// usable kernel yields an agent that fails every tool call. Returns a reason
/// when the kernel cannot be satisfied.
///
/// This is deliberately a filesystem check rather than a live `ipython` call:
/// readiness populates the provider list on every render, and executing a real
/// agent turn per refresh would be far too slow.
///
/// `prime-agent doctor` is *not* the probe here. It inspects background
/// services and reports success on an install whose kernel is entirely broken.
fn prime_kernel_blocker() -> Option<String> {
    if crate::providers::prime::kernel_python().is_some() {
        return None;
    }

    // Reading provisioning progress is a lock and a clone. Starting or waiting
    // on the work would not be: this runs on every provider-list paint.
    if let Some(progress) = prime_provisioning_progress(
        &crate::providers::prime_kernel::provisioning_state(),
    ) {
        return Some(progress);
    }

    // Prime Agent 0.7.0's own bootstrap is broken on Windows (it assumes the
    // POSIX `bin/python` virtualenv layout), so a missing Wardian-managed
    // environment is fatal there. On other platforms its bootstrap is expected
    // to work, so absence is not treated as a blocker.
    #[cfg(target_os = "windows")]
    {
        Some(prime_kernel_blocker_reason(
            crate::providers::prime::wardian_kernel_venv_dir().as_deref(),
        ))
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Turns provisioning progress into the blocker text, when it has something to
/// say that the generic "set it up yourself" message does not.
///
/// `Done` returns `None` on purpose: the environment was published but this
/// call already failed to find an interpreter, so the generic instructions are
/// the honest answer rather than a claim that setup succeeded.
fn prime_provisioning_progress(
    state: &crate::providers::prime_kernel::ProvisioningState,
) -> Option<String> {
    use crate::providers::prime_kernel::ProvisioningState;

    match state {
        ProvisioningState::Running => Some(
            "Wardian is setting up Prime Agent's Python kernel. This runs once and takes a minute; Prime Agent becomes available when it finishes."
                .to_string(),
        ),
        ProvisioningState::Failed(reason) => Some(reason.clone()),
        ProvisioningState::Idle | ProvisioningState::Done => None,
    }
}

/// Explains a missing kernel in terms of the path the running build actually
/// uses.
///
/// A debug build resolves its Wardian home under `target/`, not `~/.wardian`,
/// so a `<WARDIAN_HOME>` placeholder sends the user to build a virtualenv the
/// app will never look at.
#[cfg(target_os = "windows")]
fn prime_kernel_blocker_reason(venv_dir: Option<&Path>) -> String {
    let location = match venv_dir {
        Some(dir) => format!("at {}", dir.display()),
        None => "under the Wardian home".to_string(),
    };

    format!(
        "Prime Agent is installed but its Python kernel is not set up. Prime Agent 0.7.0 cannot bootstrap the kernel on Windows. Create a virtualenv {location} with ipykernel and Prime's bundled prime-agent-runtime, or set PRIME_AGENT_KERNEL_PYTHON to an interpreter that has them. See docs/guide/provider-readiness.md."
    )
}

/// Readiness check for an actual launch, which may block where the render-path
/// check may not.
///
/// A Prime launch started while the kernel is still being provisioned waits for
/// it. Failing here instead would tell the user to set up something Wardian is
/// already most of the way through building.
pub fn ensure_provider_available_for_launch(provider_id: &str) -> Result<(), String> {
    let readiness = provider_readiness(provider_id);
    if readiness.available {
        return Ok(());
    }

    if provider_id.trim().eq_ignore_ascii_case("prime") {
        crate::providers::prime_kernel::wait_for_in_flight_provisioning();
        let readiness = provider_readiness(provider_id);
        if readiness.available {
            return Ok(());
        }
        return Err(unavailable_launch_error(readiness));
    }

    Err(unavailable_launch_error(readiness))
}

fn unavailable_launch_error(readiness: ProviderReadiness) -> String {
    readiness.reason.unwrap_or_else(|| {
        format!(
            "{} is not available. See docs/guide/provider-readiness.md.",
            readiness.display_name
        )
    })
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

/// Resolves a helper tool the same way provider executables are resolved, so a
/// tool Wardian shells out to is found under the app's environment rather than
/// the user's shell.
pub fn find_executable_for_provisioning(name: &str) -> Option<PathBuf> {
    find_executable_on_path(name, None)
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
            vec![
                "claude",
                "codex",
                "gemini",
                "antigravity",
                "opencode",
                "prime"
            ]
        );
    }

    #[test]
    fn prime_descriptor_uses_the_full_product_name() {
        let descriptor = user_facing_provider_descriptors()
            .iter()
            .find(|provider| provider.id == "prime")
            .expect("prime descriptor");

        assert_eq!(descriptor.display_name, "Prime Agent");
    }

    #[test]
    fn antigravity_descriptor_uses_capitalized_user_label() {
        let descriptor = user_facing_provider_descriptors()
            .iter()
            .find(|provider| provider.id == "antigravity")
            .expect("antigravity descriptor");

        assert_eq!(descriptor.display_name, "Antigravity");
    }

    #[test]
    fn antigravity_readiness_uses_capitalized_descriptor_label() {
        let _guard = crate::utils::wardian_test_env_lock();
        let previous_path = std::env::var_os("PATH");
        let previous_pathext = std::env::var_os("PATHEXT");
        let temp = tempfile::tempdir().expect("temp dir");
        let executable = if cfg!(windows) {
            temp.path().join("agy.exe")
        } else {
            temp.path().join("agy")
        };
        std::fs::write(&executable, "").expect("fake agy");
        std::env::set_var("PATH", temp.path());
        #[cfg(windows)]
        std::env::set_var("PATHEXT", ".EXE");

        let readiness = provider_readiness("antigravity");

        assert!(readiness.available);
        assert_eq!(readiness.display_name, "Antigravity");

        match previous_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        match previous_pathext {
            Some(value) => std::env::set_var("PATHEXT", value),
            None => std::env::remove_var("PATHEXT"),
        }
    }

    /// The point of the message is that the reader can act on it, and a debug
    /// build's home is nowhere near `~/.wardian`.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_kernel_blocker_names_the_path_this_build_actually_reads() {
        let reason = prime_kernel_blocker_reason(Some(Path::new(
            r"D:\work\target\debug\.wardian\prime-kernel-venv",
        )));

        assert!(reason.contains(r"D:\work\target\debug\.wardian\prime-kernel-venv"));
        assert!(!reason.contains("<WARDIAN_HOME>"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_kernel_blocker_still_reads_without_a_resolvable_home() {
        let reason = prime_kernel_blocker_reason(None);

        assert!(reason.contains("under the Wardian home"));
        assert!(reason.contains("PRIME_AGENT_KERNEL_PYTHON"));
    }

    /// The user should learn that setup is under way from the same place they
    /// would otherwise be told to do it themselves.
    #[test]
    fn an_in_flight_provision_is_reported_instead_of_setup_instructions() {
        use crate::providers::prime_kernel::ProvisioningState;

        let progress =
            prime_provisioning_progress(&ProvisioningState::Running).expect("progress text");

        assert!(progress.contains("setting up"));
        assert!(!progress.contains("Create a virtualenv"));
    }

    #[test]
    fn a_failed_provision_reports_its_own_reason() {
        use crate::providers::prime_kernel::ProvisioningState;

        let reason = prime_provisioning_progress(&ProvisioningState::Failed(
            "uv exploded".to_string(),
        ));

        assert_eq!(reason.as_deref(), Some("uv exploded"));
    }

    /// Reaching `Done` while the interpreter is still missing means something
    /// removed it, so the honest answer is the ordinary setup instructions
    /// rather than a claim that provisioning worked.
    #[test]
    fn a_finished_provision_with_no_kernel_falls_back_to_instructions() {
        use crate::providers::prime_kernel::ProvisioningState;

        assert_eq!(prime_provisioning_progress(&ProvisioningState::Done), None);
        assert_eq!(prime_provisioning_progress(&ProvisioningState::Idle), None);
    }

    #[test]
    fn a_runtime_blocker_keeps_the_resolved_executable() {
        let resolved = ProviderReadiness {
            provider: "prime".to_string(),
            display_name: "Prime Agent".to_string(),
            available: true,
            executable: Some("/npm/prime-agent".to_string()),
            reason: None,
        };

        let blocked = blocked_by_runtime(resolved, "kernel is not set up".to_string());

        assert!(!blocked.available);
        assert_eq!(blocked.executable.as_deref(), Some("/npm/prime-agent"));
        assert_eq!(blocked.reason.as_deref(), Some("kernel is not set up"));
    }

    #[test]
    fn a_missing_executable_reports_no_executable() {
        let readiness = readiness_from_launch_parts(
            "prime",
            "Prime Agent",
            "definitely-not-a-wardian-provider",
            &[],
            None,
        );

        assert!(!readiness.available);
        assert!(readiness.executable.is_none());
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
