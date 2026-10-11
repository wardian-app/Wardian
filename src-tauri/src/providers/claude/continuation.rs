//! Claude interprets the shared receipt; lifecycle code only carries it.
use std::path::{Path, PathBuf};

use wardian_core::models::provider::{
    AppendOnlyTranscriptBinding, ExactResumeSourceBinding, ResumeWorkspaceContext,
};
use wardian_core::models::AgentConfig;

use super::history::{self, ClaudeTranscriptBinding};

fn root_selector() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn identity(config: &AgentConfig) -> Result<(), String> {
    config.validate_exact_resume_identity()?;
    if config.provider != "claude" || config.exact_resume.is_none() {
        return Err("Claude exact continuation requires its durable provider identity".into());
    }
    Ok(())
}

fn receipt(config: &AgentConfig) -> Result<&AppendOnlyTranscriptBinding, String> {
    identity(config)?;
    match config
        .exact_resume
        .as_ref()
        .and_then(|exact| exact.source.as_ref())
    {
        Some(ExactResumeSourceBinding::AppendOnlyTranscript(binding)) => {
            if config.resume_session.as_deref() != Some(binding.session_id.as_str()) {
                return Err("Claude continuation receipt has another session identity".into());
            }
            Ok(binding)
        }
        Some(_) => Err("Claude continuation receipt has another source kind".into()),
        None => Err("Claude exact continuation requires its prepared transcript receipt".into()),
    }
}

fn provider_binding(
    binding: &AppendOnlyTranscriptBinding,
) -> Result<ClaudeTranscriptBinding, String> {
    let number = |value: &str| {
        value
            .parse::<u64>()
            .ok()
            .filter(|number| number.to_string() == value)
            .ok_or_else(|| {
                "Claude transcript receipt has malformed native file identity".to_string()
            })
    };
    Ok(ClaudeTranscriptBinding {
        config_root: binding.config_root.clone(),
        canonical_path: binding.canonical_path.clone(),
        session_id: binding.session_id.clone(),
        native_identity: (
            number(&binding.native_identity.0)?,
            number(&binding.native_identity.1)?,
        ),
        confirmed_bytes: binding.confirmed_bytes,
        prefix_sha256: binding.prefix_sha256.clone(),
    })
}

fn verified_existing(config: &AgentConfig) -> Result<ClaudeTranscriptBinding, String> {
    let stored = receipt(config)?;
    if root_selector() != stored.root_selector {
        return Err(
            "Claude configuration root selector changed after continuation preparation".into(),
        );
    }
    let binding = provider_binding(stored)?;
    history::revalidate_exact_continuation_at_root(config, &binding.config_root, &binding)
}

fn publish(config: &mut AgentConfig, binding: ClaudeTranscriptBinding, selector: Option<PathBuf>) {
    let stored = AppendOnlyTranscriptBinding {
        config_root: binding.config_root,
        canonical_path: binding.canonical_path,
        session_id: binding.session_id,
        native_identity: (
            binding.native_identity.0.to_string(),
            binding.native_identity.1.to_string(),
        ),
        confirmed_bytes: binding.confirmed_bytes,
        prefix_sha256: binding.prefix_sha256,
        root_selector: selector,
    };
    // All fallible identity, option, selector and source checks precede mutation.
    config
        .exact_resume
        .as_mut()
        .expect("validated exact identity")
        .source = Some(ExactResumeSourceBinding::AppendOnlyTranscript(stored));
}

pub(super) fn validate(config: &AgentConfig) -> Result<(), String> {
    identity(config)?;
    if config
        .exact_resume
        .as_ref()
        .is_some_and(|exact| exact.source.is_some())
    {
        verified_existing(config).map(|_| ())
    } else {
        // Legacy exact intent may be prepared by the following context-aware
        // preflight. This read-only check never creates a native fresh grant.
        let context = crate::manager::continuation_workspace_context(config, false, None)?;
        history::prepare_exact_continuation(config, &context.launch_cwd).map(|_| ())
    }
}

pub(super) fn prepare(
    config: &mut AgentConfig,
    context: &ResumeWorkspaceContext,
) -> Result<(), String> {
    identity(config)?;
    if context.workspace_changed {
        history::validate_workspace_handover_root()?;
    }
    let selector = root_selector();
    let binding = if config
        .exact_resume
        .as_ref()
        .is_some_and(|exact| exact.source.is_some())
    {
        verified_existing(config)?
    } else {
        history::prepare_exact_continuation(config, &context.launch_cwd)?
    };
    if root_selector() != selector {
        return Err(
            "Claude configuration root selector changed during continuation preparation".into(),
        );
    }
    publish(config, binding, selector);
    Ok(())
}

pub(super) fn revalidate(
    config: &mut AgentConfig,
    context: &ResumeWorkspaceContext,
) -> Result<(), String> {
    identity(config)?;
    if context.workspace_changed {
        history::validate_workspace_handover_root()?;
    }
    let selector = receipt(config)?.root_selector.clone();
    let binding = verified_existing(config)?;
    if root_selector() != selector {
        return Err(
            "Claude configuration root selector changed during continuation revalidation".into(),
        );
    }
    publish(config, binding, selector);
    Ok(())
}

/// Child root for the verified generation. Missing or foreign exact receipts
/// cannot fall back to whichever root the current environment happens to select.
pub(crate) fn continuation_root(config: &AgentConfig, child_cwd: &Path) -> Result<PathBuf, String> {
    if config.provider == "claude" && config.exact_resume.is_some() {
        let binding = receipt(config)?;
        if !binding.config_root.is_absolute() {
            return Err("Claude exact continuation root is not absolute".into());
        }
        Ok(binding.config_root.clone())
    } else {
        history::config_root(child_cwd)
    }
}

/// Pin an explicitly selected configuration root without changing Claude's
/// native default credential/config lookup. A default transcript root is not
/// equivalent to setting CLAUDE_CONFIG_DIR to that directory.
pub(crate) fn optional_continuation_root_override(
    config: &AgentConfig,
    child_cwd: &Path,
) -> Result<Option<PathBuf>, String> {
    if config.provider != "claude" {
        return Err("Claude child root override requires the Claude provider".into());
    }
    let selector = root_selector();
    if config.exact_resume.is_some() {
        let binding = receipt(config)?;
        if selector != binding.root_selector {
            return Err(
                "Claude configuration root selector changed after continuation preparation".into(),
            );
        }
        let root = continuation_root(config, child_cwd)?;
        Ok(binding.root_selector.as_ref().map(|_| root))
    } else {
        selector
            .map(|_| history::config_root(child_cwd))
            .transpose()
    }
}

/// Canonical log source for a validated Claude generation, never a destination
/// project-folder guess or an arbitrary different provider's receipt.
pub(crate) fn continuation_path(config: &AgentConfig) -> Option<PathBuf> {
    let binding = receipt(config).ok()?;
    binding
        .canonical_path
        .is_absolute()
        .then(|| binding.canonical_path.clone())
}

#[cfg(test)]
mod tests {
    use super::super::ClaudeProvider;
    use super::*;
    use std::ffi::OsString;
    use std::io::Write;
    use wardian_core::models::provider::AgentProvider;

    const ID: &str = "019d331a-0500-7592-969f-8f437886f42b";

    struct Fixture {
        _temporary: tempfile::TempDir,
        previous_root: Option<OsString>,
        root: PathBuf,
        path: PathBuf,
        config: AgentConfig,
        context: ResumeWorkspaceContext,
    }
    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().join("provider-root");
            let source = root.join("projects/source");
            std::fs::create_dir_all(&source).unwrap();
            let path = source.join(format!("{ID}.jsonl"));
            std::fs::write(&path, record("retained")).unwrap();
            let previous_root = std::env::var_os("CLAUDE_CONFIG_DIR");
            unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &root) };
            let mut config = AgentConfig {
                provider: "claude".into(),
                session_id: uuid::Uuid::new_v4().to_string(),
                folder: temporary.path().to_string_lossy().into_owned(),
                resume_session: Some(ID.into()),
                ..Default::default()
            };
            config.require_exact_resume();
            let context = ResumeWorkspaceContext {
                launch_cwd: temporary.path().to_path_buf(),
                workspace_changed: false,
                owned_projection: None,
            };
            Self {
                _temporary: temporary,
                previous_root,
                root,
                path,
                config,
                context,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match self.previous_root.take() {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }
    fn record(text: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type":"user","sessionId":ID,
            "message":{"role":"user","content":text}})
        )
    }

    #[test]
    fn callbacks_preserve_restored_authority_and_extend_closing_append() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        let provider = ClaudeProvider::new();
        provider
            .prepare_exact_resume(&mut fixture.config, &fixture.context)
            .unwrap();
        let prepared = receipt(&fixture.config).unwrap().clone();
        let mut restored: AgentConfig =
            serde_json::from_value(serde_json::to_value(&fixture.config).unwrap()).unwrap();
        std::fs::File::options()
            .append(true)
            .open(&fixture.path)
            .unwrap()
            .write_all(record("closing append").as_bytes())
            .unwrap();
        let context = ResumeWorkspaceContext {
            launch_cwd: fixture.context.launch_cwd.join("different-destination"),
            workspace_changed: true,
            owned_projection: None,
        };
        provider
            .revalidate_exact_resume(&mut restored, &context)
            .unwrap();
        let extended = receipt(&restored).unwrap();
        assert_eq!(extended.config_root, prepared.config_root);
        assert_eq!(extended.canonical_path, prepared.canonical_path);
        assert_eq!(extended.native_identity, prepared.native_identity);
        assert_eq!(extended.root_selector, Some(fixture.root.clone()));
        assert!(extended.confirmed_bytes > prepared.confirmed_bytes);
        assert_eq!(
            continuation_root(&restored, &context.launch_cwd).unwrap(),
            prepared.config_root
        );
        assert_eq!(continuation_path(&restored), Some(prepared.canonical_path));
    }

    #[test]
    fn default_child_root_does_not_override_native_config_lookup() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        prepare(&mut fixture.config, &fixture.context).unwrap();
        let Some(ExactResumeSourceBinding::AppendOnlyTranscript(binding)) = fixture
            .config
            .exact_resume
            .as_mut()
            .unwrap()
            .source
            .as_mut()
        else {
            unreachable!()
        };
        // Simulate a restored receipt captured with the native default selector.
        // Its independently captured canonical transcript authority stays pinned.
        binding.root_selector = None;
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        assert_eq!(
            optional_continuation_root_override(&fixture.config, &fixture.context.launch_cwd)
                .unwrap(),
            None
        );
        assert_eq!(
            continuation_root(&fixture.config, &fixture.context.launch_cwd).unwrap(),
            std::fs::canonicalize(&fixture.root).unwrap()
        );
        assert_eq!(
            continuation_path(&fixture.config),
            Some(std::fs::canonicalize(&fixture.path).unwrap())
        );
        let fresh = AgentConfig {
            provider: "claude".into(),
            ..Default::default()
        };
        assert_eq!(
            optional_continuation_root_override(&fresh, &fixture.context.launch_cwd).unwrap(),
            None
        );
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "") };
        assert_eq!(
            optional_continuation_root_override(&fresh, &fixture.context.launch_cwd).unwrap(),
            None
        );
    }

    #[test]
    fn explicit_child_root_pins_receipt_and_rejects_selector_changes() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        prepare(&mut fixture.config, &fixture.context).unwrap();
        let expected = std::fs::canonicalize(&fixture.root).unwrap();
        let changed_cwd = fixture.context.launch_cwd.join("rotated-workspace");
        assert_eq!(
            optional_continuation_root_override(&fixture.config, &changed_cwd).unwrap(),
            Some(expected)
        );
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        assert!(
            optional_continuation_root_override(&fixture.config, &changed_cwd)
                .unwrap_err()
                .contains("selector changed")
        );
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "relative-root") };
        let fresh = AgentConfig {
            provider: "claude".into(),
            ..Default::default()
        };
        assert_eq!(
            optional_continuation_root_override(&fresh, &changed_cwd).unwrap(),
            Some(changed_cwd.join("relative-root"))
        );
        assert!(optional_continuation_root_override(
            &AgentConfig {
                provider: "codex".into(),
                ..Default::default()
            },
            &changed_cwd
        )
        .is_err());
    }

    #[test]
    fn replacement_and_root_selector_change_never_recapture_receipt() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        prepare(&mut fixture.config, &fixture.context).unwrap();
        let before = serde_json::to_value(&fixture.config).unwrap();
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", fixture.root.join("new-selector")) };
        assert!(prepare(&mut fixture.config, &fixture.context)
            .unwrap_err()
            .contains("selector changed"));
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &fixture.root) };
        std::fs::rename(&fixture.path, fixture.path.with_extension("saved")).unwrap();
        std::fs::write(&fixture.path, record("retained")).unwrap();
        assert!(prepare(&mut fixture.config, &fixture.context)
            .unwrap_err()
            .contains("replaced"));
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
    }

    #[test]
    fn missing_receipt_cannot_cross_revalidation_boundary() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        let before = serde_json::to_value(&fixture.config).unwrap();
        validate(&fixture.config).unwrap();
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
        assert!(revalidate(&mut fixture.config, &fixture.context)
            .unwrap_err()
            .contains("prepared transcript receipt"));
        assert!(continuation_root(&fixture.config, &fixture.context.launch_cwd).is_err());
        assert_eq!(continuation_path(&fixture.config), None);
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
    }

    #[test]
    fn native_identity_strings_cannot_overflow_or_alias_the_original() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        prepare(&mut fixture.config, &fixture.context).unwrap();
        let original = fixture.config.clone();
        for invalid in ["", "18446744073709551616", "+1", "01", "not-an-id"] {
            let mut changed = original.clone();
            let Some(ExactResumeSourceBinding::AppendOnlyTranscript(binding)) =
                changed.exact_resume.as_mut().unwrap().source.as_mut()
            else {
                unreachable!()
            };
            binding.native_identity.0 = invalid.into();
            let before = serde_json::to_value(&changed).unwrap();
            assert!(revalidate(&mut changed, &fixture.context)
                .unwrap_err()
                .contains("native file identity"));
            assert_eq!(serde_json::to_value(&changed).unwrap(), before);
        }
    }

    #[test]
    fn relative_override_rejects_workspace_rotation_before_source_capture() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "relative-provider-root") };
        fixture.context.workspace_changed = true;
        let before = serde_json::to_value(&fixture.config).unwrap();
        assert!(prepare(&mut fixture.config, &fixture.context)
            .unwrap_err()
            .contains("could rebind"));
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
    }

    #[test]
    fn source_kind_and_provider_identity_cannot_be_substituted() {
        let _lock = crate::utils::wardian_test_env_lock();
        let mut fixture = Fixture::new();
        prepare(&mut fixture.config, &fixture.context).unwrap();
        let mut wrong = fixture.config.clone();
        wrong.provider = "codex".into();
        assert!(revalidate(&mut wrong, &fixture.context).is_err());
        assert_eq!(continuation_path(&wrong), None);
        fixture.config.exact_resume.as_mut().unwrap().source =
            Some(ExactResumeSourceBinding::MaterializedStore(
                wardian_core::models::provider::MaterializedStoreBinding {
                    source_path: fixture.path.clone(),
                    session_id: ID.into(),
                    native_identity: ("0".into(), "0".into()),
                    confirmed_records: Vec::new(),
                },
            ));
        let before = serde_json::to_value(&fixture.config).unwrap();
        assert!(prepare(&mut fixture.config, &fixture.context)
            .unwrap_err()
            .contains("source kind"));
        assert_eq!(serde_json::to_value(&fixture.config).unwrap(), before);
    }
}
