//! Read-only continuation launch context. No projection is rebound here.
use std::path::{Component, Path};
use wardian_core::models::provider::{ExactResumeSourceBinding, ResumeWorkspaceContext};
use wardian_core::models::AgentConfig;

/// Select the same logical habitat used by ordinary launch, with ownership
/// checked independently of a persisted receipt. Only an existing verified
/// alias may replace that path during preflight.
pub(crate) fn continuation_workspace_context(
    config: &AgentConfig,
    workspace_changed: bool,
    actual_launch_cwd: Option<&Path>,
) -> Result<ResumeWorkspaceContext, String> {
    let id_path = Path::new(&config.session_id);
    let mut components = id_path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("Continuation requires a single-component agent identity".into());
    }
    let workspace = Path::new(&config.folder);
    let habitat = crate::utils::fs::get_wardian_home()
        .map(|home| home.join("agents").join(&config.session_id).join("habitat"));
    let logical =
        super::interactive_provider_cwd(&config.provider, workspace, habitat.as_deref(), None);
    let saved_projection =
        config
            .exact_resume
            .as_ref()
            .and_then(|identity| match &identity.source {
                Some(ExactResumeSourceBinding::StableProjection(binding)) => {
                    Some(binding.projection_path.as_path())
                }
                _ => None,
            });
    let verified_alias = |path: &Path| {
        habitat.as_ref().is_some_and(|root| {
            crate::utils::codex_home::is_owned_habitat_workspace_alias(
                &config.session_id,
                root,
                path,
            )
        })
    };
    let launch = actual_launch_cwd.map(Path::to_path_buf).unwrap_or_else(|| {
        saved_projection
            .filter(|path| verified_alias(path))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| logical.clone())
    });
    let owned_projection = habitat.as_ref().and_then(|root| {
        let expected = super::habitat_workspace_cwd(root);
        (launch == expected || verified_alias(&launch)).then_some(launch.clone())
    });
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if launch.as_os_str().encode_wide().count() > 258 {
            return Err(
                "Continuation preflight requires an existing verified short launch alias".into(),
            );
        }
    }
    Ok(ResumeWorkspaceContext {
        launch_cwd: launch,
        owned_projection,
        workspace_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use wardian_core::models::provider::{ExactResumeIdentity, StableProjectionBinding};

    fn path_units(path: &Path) -> usize {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str().encode_wide().count()
        }
        #[cfg(not(windows))]
        {
            path.as_os_str().len()
        }
    }

    struct IsolatedHome {
        home: PathBuf,
        workspace: PathBuf,
        habitat: PathBuf,
        previous_home: Option<OsString>,
        #[cfg(windows)]
        previous_alias_roots: Option<Vec<PathBuf>>,
        _temporary: tempfile::TempDir,
        // Last field keeps environment serialization through temporary cleanup.
        _environment: tokio::sync::MutexGuard<'static, ()>,
    }

    impl IsolatedHome {
        fn new(long_home: bool) -> Self {
            let environment = crate::utils::wardian_test_env_lock();
            let temporary = tempfile::Builder::new().prefix("ct").tempdir().unwrap();
            let base = temporary.path().canonicalize().unwrap();
            let mut home = base.join("wardian-home");
            if long_home {
                while path_units(&home) < 235 {
                    home.push("long-home-segment-abcdefghijkl");
                }
            }
            let habitat = home.join("agents/agent/habitat");
            let workspace = base.join("destination");
            std::fs::create_dir_all(&habitat).unwrap();
            std::fs::create_dir_all(&workspace).unwrap();
            if long_home {
                crate::utils::fs::create_directory_link(&workspace, &habitat.join("workspace"))
                    .unwrap();
            } else {
                std::fs::create_dir_all(habitat.join("workspace")).unwrap();
            }
            let previous_home = std::env::var_os("WARDIAN_HOME");
            unsafe { std::env::set_var("WARDIAN_HOME", &home) };
            #[cfg(windows)]
            let previous_alias_roots = crate::utils::codex_home::TEST_ROOTS
                .with(|roots| roots.replace(Some(vec![base.join("aliases")])));
            Self {
                home,
                workspace,
                habitat,
                previous_home,
                #[cfg(windows)]
                previous_alias_roots,
                _temporary: temporary,
                _environment: environment,
            }
        }

        fn config(&self, provider: &str) -> AgentConfig {
            AgentConfig {
                session_id: "agent".into(),
                provider: provider.into(),
                folder: self.workspace.to_string_lossy().into_owned(),
                resume_session: Some("native-session".into()),
                ..Default::default()
            }
        }
    }

    impl Drop for IsolatedHome {
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                crate::utils::codex_home::cleanup_habitat_alias(&self.home, "agent").unwrap();
                crate::utils::codex_home::TEST_ROOTS
                    .with(|roots| roots.replace(self.previous_alias_roots.take()));
            }
            match self.previous_home.take() {
                Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
                None => unsafe { std::env::remove_var("WARDIAN_HOME") },
            }
        }
    }

    fn save_projection(config: &mut AgentConfig, path: PathBuf) {
        config.exact_resume = Some(ExactResumeIdentity {
            provider: config.provider.clone(),
            session_id: "native-session".into(),
            source: Some(ExactResumeSourceBinding::StableProjection(
                StableProjectionBinding {
                    source_path: path.with_extension("db"),
                    projection_path: path,
                    session_id: "native-session".into(),
                    native_identity: ("10".into(), "20".into()),
                },
            )),
        });
    }

    #[test]
    fn rejects_agent_identity_paths_before_deriving_owned_workspace() {
        let fixture = IsolatedHome::new(false);
        for id in ["", ".", "..", "nested/agent", "../agent"] {
            let mut config = fixture.config("antigravity");
            config.session_id = id.into();
            assert!(continuation_workspace_context(&config, true, None)
                .unwrap_err()
                .contains("single-component"));
        }
        #[cfg(windows)]
        for id in [r"nested\agent", r"C:\agent", r"\agent"] {
            let mut config = fixture.config("antigravity");
            config.session_id = id.into();
            assert!(continuation_workspace_context(&config, true, None).is_err());
        }
    }

    #[test]
    fn context_matches_each_providers_normal_launch_scope() {
        let fixture = IsolatedHome::new(false);
        for provider in ["claude", "antigravity", "pi", "codex", "opencode"] {
            let config = fixture.config(provider);
            let context = continuation_workspace_context(&config, true, None).unwrap();
            let expected = match provider {
                "pi" | "codex" => fixture.workspace.clone(),
                "opencode" => fixture.home.join("agents/agent/habitat"),
                _ => fixture.home.join("agents/agent/habitat/workspace"),
            };
            assert_eq!(context.launch_cwd, expected, "{provider}");
            assert!(context.workspace_changed);
            assert_eq!(
                context.owned_projection,
                if matches!(provider, "claude" | "antigravity") {
                    Some(expected)
                } else {
                    None
                },
                "{provider}"
            );
        }
    }

    #[test]
    fn forged_projection_receipt_cannot_replace_logical_or_actual_launch_path() {
        let fixture = IsolatedHome::new(false);
        let forged = fixture.workspace.join("forged-projection");
        std::fs::create_dir(&forged).unwrap();
        for provider in ["claude", "antigravity", "pi", "codex", "opencode"] {
            let mut config = fixture.config(provider);
            let normal = continuation_workspace_context(&config, false, None).unwrap();
            save_projection(&mut config, forged.clone());
            let context = continuation_workspace_context(&config, false, None).unwrap();
            assert_eq!(context.launch_cwd, normal.launch_cwd);
            assert_eq!(context.owned_projection, normal.owned_projection);
            let actual =
                continuation_workspace_context(&config, true, Some(&fixture.workspace)).unwrap();
            assert_eq!(actual.launch_cwd, fixture.workspace);
            assert!(actual.owned_projection.is_none());
        }
    }

    #[test]
    fn actual_unowned_antigravity_cwd_has_no_projection_eligibility() {
        let fixture = IsolatedHome::new(false);
        let mut config = fixture.config("antigravity");
        save_projection(&mut config, fixture.habitat.join("workspace"));
        let actual =
            continuation_workspace_context(&config, true, Some(&fixture.workspace)).unwrap();
        assert_eq!(actual.launch_cwd, fixture.workspace);
        assert!(actual.owned_projection.is_none());
        assert!(actual.workspace_changed);
    }

    #[cfg(unix)]
    #[test]
    fn posix_symlink_equivalence_does_not_grant_owned_alias_authority() {
        let fixture = IsolatedHome::new(false);
        let alias = fixture.workspace.join("unregistered-alias");
        std::os::unix::fs::symlink(fixture.habitat.join("workspace"), &alias).unwrap();
        let mut config = fixture.config("antigravity");
        save_projection(&mut config, alias.clone());
        let ordinary = continuation_workspace_context(&config, true, None).unwrap();
        assert_eq!(ordinary.launch_cwd, fixture.habitat.join("workspace"));
        let actual = continuation_workspace_context(&config, true, Some(&alias)).unwrap();
        assert_eq!(actual.launch_cwd, alias);
        assert!(actual.owned_projection.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_verified_short_alias_is_owned_but_cannot_be_replayed_for_another_agent() {
        let fixture = IsolatedHome::new(true);
        let logical = fixture.habitat.join("workspace");
        let alias = crate::utils::codex_home::prepare_habitat_cwd_alias(
            "agent",
            Some(&fixture.habitat),
            &fixture.workspace,
            &logical,
        )
        .unwrap()
        .expect("long habitat requires verified alias");
        let mut config = fixture.config("antigravity");
        save_projection(&mut config, alias.clone());
        let restored = continuation_workspace_context(&config, true, None).unwrap();
        assert_eq!(restored.launch_cwd, alias);
        assert_eq!(restored.owned_projection, Some(alias.clone()));
        save_projection(&mut config, fixture.workspace.clone());
        let actual = continuation_workspace_context(&config, true, Some(&alias)).unwrap();
        assert_eq!(actual.launch_cwd, alias);
        assert_eq!(actual.owned_projection, Some(alias.clone()));
        config.session_id = "other-agent".into();
        let other = continuation_workspace_context(&config, true, Some(&alias)).unwrap();
        assert!(other.owned_projection.is_none());
    }
}
