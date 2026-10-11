//! Publish Pi resume identity only after its deferred first history flush.
use crate::manager::roster_io;
use crate::state::AppState;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wardian_core::models::AgentConfig;

/// Runtime-only selection binds canonical identity to one verified owned file.
/// Fresh launches reject incomplete identity collisions; no path enters persisted config.
pub(super) struct PiLaunchPlan {
    directory: PathBuf,
    native_id: String,
    resume: Option<crate::providers::pi::history::SessionFileBinding>,
    excluded: std::collections::HashSet<PathBuf>,
}

impl PiLaunchPlan {
    pub(super) fn prepare(config: &AgentConfig) -> Result<Self, String> {
        let directory = crate::providers::pi::PiProvider::session_dir(&config.session_id)
            .ok_or("Pi history directory unavailable")?;
        let native_id = crate::manager::session_identity::expected_caller_owned_identity(config)
            .ok_or("Pi launch requires an exact native identity")?
            .to_owned();
        let history = crate::providers::pi::history::inspect_owned_history(&directory, &native_id)?;
        let resume = if config.resume_session.is_some() {
            Some(
                history
                    .complete
                    .ok_or("Pi cannot resume without exact complete owned history")?,
            )
        } else {
            if history.complete.is_some() {
                return Err("Pi pending history must be reconciled before fresh launch".into());
            }
            // Pi's --session-id reopens matching header-only histories too.
            // Watcher exclusion cannot make that provider launch fresh.
            if !history.preexisting.is_empty() {
                return Err(
                    "Pi pending identity has incomplete history; use New Session to start fresh"
                        .into(),
                );
            }
            None
        };
        Ok(Self {
            directory,
            native_id,
            resume,
            excluded: history.preexisting,
        })
    }

    pub(super) fn apply_args(&self, args: &mut [String]) -> Result<(), String> {
        self.validate_selector(args, &self.native_id)?;
        if let Some(binding) = &self.resume {
            let index = args
                .windows(2)
                .position(|pair| pair == ["--session", &self.native_id])
                .ok_or("Pi launch arguments lost canonical resume identity")?;
            args[index + 1] = binding.path.to_string_lossy().into_owned();
        }
        Ok(())
    }

    /// Pi accepts the last duplicate selector. Validate the complete managed
    /// vector, including custom arguments, so it cannot override this binding.
    pub(super) fn validate_args(&self, args: &[String]) -> Result<(), String> {
        let value = match &self.resume {
            Some(binding) => binding
                .path
                .to_str()
                .ok_or("Pi history path is not valid UTF-8")?,
            None => &self.native_id,
        };
        self.validate_selector(args, value)
    }

    fn validate_selector(&self, args: &[String], value: &str) -> Result<(), String> {
        let selector = if self.resume.is_some() {
            "--session"
        } else {
            "--session-id"
        };
        let directory = self
            .directory
            .to_str()
            .ok_or("Pi history directory is not valid UTF-8")?;
        let mut identity_seen = false;
        let mut directory_seen = false;
        for (index, argument) in args.iter().enumerate() {
            match argument.as_str() {
                "--session" | "--session-id" => {
                    if identity_seen
                        || argument != selector
                        || args.get(index + 1).map(String::as_str) != Some(value)
                    {
                        return Err(
                            "Pi custom session selector conflicts with managed identity".into()
                        );
                    }
                    identity_seen = true;
                }
                "--session-dir" => {
                    if directory_seen || args.get(index + 1).map(String::as_str) != Some(directory)
                    {
                        return Err(
                            "Pi custom session directory conflicts with owned history".into()
                        );
                    }
                    directory_seen = true;
                }
                "--no-session" | "--fork" | "--resume" | "-r" | "--continue" | "-c" => {
                    return Err("Pi custom session mode conflicts with managed identity".into());
                }
                _ => {}
            }
        }
        if !identity_seen || !directory_seen {
            return Err("Pi launch arguments lost managed session selection".into());
        }
        Ok(())
    }

    /// Recheck immediately before the child can write. New partial files are
    /// also pre-existing for this launch; changed/ambiguous history is an error.
    pub(super) fn revalidate(&mut self) -> Result<(), String> {
        let history =
            crate::providers::pi::history::inspect_owned_history(&self.directory, &self.native_id)?;
        match (&self.resume, history.complete) {
            (Some(binding), Some(current)) if binding.path == current.path => {
                binding.revalidate()?
            }
            (None, None) if history.preexisting.is_empty() => {}
            _ => return Err("Pi owned history changed during launch preparation".into()),
        }
        self.excluded.extend(history.preexisting);
        Ok(())
    }

    pub(super) fn admits(&self, path: &Path) -> bool {
        if path.parent() != Some(self.directory.as_path()) {
            return false;
        }
        match &self.resume {
            Some(binding) => path == binding.path,
            None => !self.excluded.contains(path),
        }
    }

    pub(super) fn session_file(&self) -> Option<PathBuf> {
        match &self.resume {
            Some(binding) => binding.path.is_file().then(|| binding.path.clone()),
            None => crate::providers::pi::PiProvider::session_file_excluding(
                &self.directory,
                &self.native_id,
                &self.excluded,
            ),
        }
    }

    pub(super) fn resume_path(&self) -> Option<&Path> {
        self.resume.as_ref().map(|binding| binding.path.as_path())
    }
}

/// Leave partial records unconsumed by the caller's cursor so the next poll
/// rereads them after Pi completes its deferred first JSONL flush.
pub(super) fn read_complete_record(reader: &mut impl BufRead, line: &mut String) -> Option<usize> {
    line.clear();
    let read = reader.read_line(line).ok()?;
    (read != 0 && line.ends_with('\n')).then_some(read)
}

#[derive(Default)]
pub(super) struct HistoryConfirmation {
    path: Option<PathBuf>,
    header_matches: bool,
    assistant_persisted: bool,
}

impl HistoryConfirmation {
    pub(super) fn reset(&mut self, path: &Path) {
        self.path = Some(path.to_owned());
        self.header_matches = false;
        self.assistant_persisted = false;
    }

    pub(super) fn observe(&mut self, value: &serde_json::Value, expected: &str) {
        if value["type"] == "session" {
            self.header_matches = value["id"].as_str() == Some(expected);
            self.assistant_persisted = false;
        } else if self.header_matches
            && value["type"] == "message"
            && value["message"]["role"] == "assistant"
        {
            self.assistant_persisted = true;
        }
    }

    pub(super) fn confirmed(&self, path: &Path) -> bool {
        self.path.as_deref() == Some(path) && self.header_matches && self.assistant_persisted
    }
}

fn matches_launch(
    agent: &crate::state::ActiveAgent,
    config: &Arc<Mutex<AgentConfig>>,
    generation: u64,
) -> bool {
    agent.runtime_generation == Some(generation) && Arc::ptr_eq(&agent.config, config)
}

/// The watcher retains this continuation until disk publication finishes. The
/// lifecycle guard fences stop/replacement; roster admission fences concurrent
/// saves. Failed writes leave the live and durable identity pending for retry.
pub(super) async fn publish_resume_identity(
    state: &AppState,
    config: &Arc<Mutex<AgentConfig>>,
    generation: u64,
    native_id: &str,
) -> Result<bool, String> {
    let agent_id = config
        .lock()
        .map_err(|error| error.to_string())?
        .session_id
        .clone();
    // Stop owns this gate while joining the watcher. Waiting here would make
    // the join depend on the lifecycle operation which is waiting for us.
    let Some(_lifecycle) = state.try_lock_agent_lifecycle(&agent_id).await else {
        return Ok(false);
    };
    if config
        .lock()
        .map_err(|error| error.to_string())?
        .pending_pi_session_id()
        != Some(native_id)
    {
        return Ok(false);
    }
    // Stand aside for roster owners too; a later poll retries the same history.
    let Some(barrier) = tokio::task::spawn_blocking(|| {
        wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    let home = crate::utils::fs::get_wardian_home().ok_or("Wardian home unavailable")?;
    let mut configs = {
        let agents = state.agents.lock().await;
        let Some(agent) = agents.get(&agent_id) else {
            return Ok(false);
        };
        if !matches_launch(agent, config, generation) {
            return Ok(false);
        }
        let order = state.agent_order.lock().await;
        crate::manager::state_configs_snapshot(&agents, &order)
    };
    let candidate = configs
        .iter_mut()
        .find(|entry| entry.session_id == agent_id)
        .ok_or("Pi agent missing from roster snapshot")?;
    candidate.resume_session = Some(native_id.to_owned());
    roster_io::write_snapshot_strict(&barrier, home, configs).await?;
    // Nothing becomes resumable in memory before the strict durable write.
    let agents = state.agents.lock().await;
    let Some(agent) = agents.get(&agent_id) else {
        return Ok(false);
    };
    if !matches_launch(agent, config, generation) {
        return Err("Pi runtime changed during history publication".into());
    }
    drop(agents);
    config
        .lock()
        .map_err(|error| error.to_string())?
        .resume_session = Some(native_id.to_owned());
    // Retain the launch capture marker for the first transcript prefix. Serde
    // omits it once resume_session is confirmed.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::provider::AgentProvider;

    #[test]
    fn fresh_launch_admits_the_new_owned_file_when_no_identity_collision_exists() {
        let _home = crate::control::test_support::TestWardianHome::new();
        let config = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            fresh_provider_session_id: Some("reserved".into()),
            ..Default::default()
        };
        let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let header = "{\"type\":\"session\",\"id\":\"reserved\"}\n";
        let mut plan = PiLaunchPlan::prepare(&config).unwrap();
        plan.revalidate().unwrap();
        let mut args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
        plan.apply_args(&mut args).unwrap();
        plan.validate_args(&args).unwrap();
        assert_eq!(plan.session_file(), None);
        let current = dir.join("current.jsonl");
        std::fs::write(&current, header).unwrap();
        assert!(plan.admits(&current));
        assert_eq!(plan.session_file(), Some(current));
    }

    #[test]
    fn fresh_launch_rejects_native_shaped_partial_history_before_pi_can_reopen_it() {
        let home = crate::control::test_support::TestWardianHome::new();
        let config = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            fresh_provider_session_id: Some("8abce8f1-c0f4-4a81-9e0b-3c3e17f9d502".into()),
            folder: home.path().join("cwd").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut plan = PiLaunchPlan::prepare(&config).unwrap();
        let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let partial = dir.join("partial.jsonl");
        // Stock Pi's exact-ID lookup includes this native-shaped header even
        // with no assistant entries. It can also load the retained user turn.
        let header = serde_json::json!({
            "type": "session", "version": 3,
            "id": config.pending_pi_session_id().unwrap(),
            "timestamp": "2026-01-01T00:00:00.000Z", "cwd": config.folder,
        });
        let bytes = format!("{header}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"retained turn\"}}]}}}}\n");
        std::fs::write(&partial, &bytes).unwrap();
        assert!(PiLaunchPlan::prepare(&config).is_err());
        assert!(plan.revalidate().is_err());
        assert_eq!(config.resume_session, None);
        assert!(config.pending_pi_session_id().is_some());
        assert_eq!(std::fs::read_to_string(&partial).unwrap(), bytes);
    }

    #[test]
    fn resumed_launch_uses_exact_complete_file_for_args_and_baseline_then_rejects_change() {
        let _home = crate::control::test_support::TestWardianHome::new();
        let config = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            resume_session: Some("reserved".into()),
            ..Default::default()
        };
        let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let header = "{\"type\":\"session\",\"id\":\"reserved\"}\n";
        std::fs::write(dir.join("old-partial.jsonl"), header).unwrap();
        let current = dir.join("complete.jsonl");
        std::fs::write(
            &current,
            format!("{header}{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\"}}}}\n"),
        )
        .unwrap();
        let mut plan = PiLaunchPlan::prepare(&config).unwrap();
        let mut args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
        plan.apply_args(&mut args).unwrap();
        plan.validate_args(&args).unwrap();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--session", current.to_str().unwrap()]));
        assert_eq!(plan.resume_path(), Some(current.as_path()));
        assert_eq!(plan.session_file(), Some(current.clone()));
        assert_eq!(config.resume_session.as_deref(), Some("reserved"));
        plan.revalidate().unwrap();
        std::fs::write(&current, header).unwrap();
        assert!(plan.revalidate().is_err());
    }

    #[test]
    fn custom_selectors_cannot_override_exact_resume_file_or_owned_directory() {
        let _home = crate::control::test_support::TestWardianHome::new();
        let mut config = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            resume_session: Some("reserved".into()),
            ..Default::default()
        };
        let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let header = "{\"type\":\"session\",\"id\":\"reserved\"}\n";
        let partial = dir.join("partial.jsonl");
        std::fs::write(&partial, header).unwrap();
        std::fs::write(
            dir.join("complete.jsonl"),
            format!("{header}{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\"}}}}\n"),
        )
        .unwrap();
        let plan = PiLaunchPlan::prepare(&config).unwrap();
        for custom in [
            format!("--session '{}'", partial.display()),
            "--session reserved".into(),
            "--session-id reserved".into(),
            "--session-dir other".into(),
            "--no-session".into(),
            "--fork reserved".into(),
            "--resume".into(),
            "-r".into(),
            "--continue".into(),
            "-c".into(),
        ] {
            config.custom_args = Some(custom.clone());
            let mut args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
            assert!(plan.apply_args(&mut args).is_err(), "accepted {custom}");
        }
        config.custom_args = Some("--offline --no-tools".into());
        let mut args = crate::providers::pi::PiProvider::new().get_spawn_args(&config, true);
        plan.apply_args(&mut args).unwrap();
        plan.validate_args(&args).unwrap();
        assert!(args.contains(&"--offline".into()));
        // Validate the final vector too, after managed extensions are appended.
        args.extend(["--session".into(), partial.to_string_lossy().into_owned()]);
        assert!(plan.validate_args(&args).is_err());
    }

    #[test]
    fn missing_confirmed_history_and_late_complete_pending_history_fail_before_launch() {
        let _home = crate::control::test_support::TestWardianHome::new();
        let mut config = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            resume_session: Some("reserved".into()),
            ..Default::default()
        };
        assert!(PiLaunchPlan::prepare(&config).is_err());
        config.resume_session = None;
        config.fresh_provider_session_id = Some("reserved".into());
        let mut plan = PiLaunchPlan::prepare(&config).unwrap();
        let dir = crate::providers::pi::PiProvider::session_dir(&config.session_id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("late.jsonl"), "{\"type\":\"session\",\"id\":\"reserved\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\"}}\n").unwrap();
        assert!(plan.revalidate().is_err());
        assert_eq!(config.resume_session, None);
    }

    #[test]
    fn confirmation_requires_matching_header_and_persisted_assistant_in_same_file() {
        let path = Path::new("owned.jsonl");
        let mut evidence = HistoryConfirmation::default();
        evidence.reset(path);
        let assistant = serde_json::json!({"type":"message","message":{"role":"assistant"}});
        evidence.observe(&assistant, "expected");
        assert!(!evidence.confirmed(path));
        evidence.observe(
            &serde_json::json!({"type":"session","id":"foreign"}),
            "expected",
        );
        evidence.observe(&assistant, "expected");
        assert!(!evidence.confirmed(path));
        evidence.observe(
            &serde_json::json!({"type":"session","id":"expected"}),
            "expected",
        );
        assert!(!evidence.confirmed(path));
        evidence.observe(
            &serde_json::json!({"type":"message","message":{"role":"user"}}),
            "expected",
        );
        assert!(!evidence.confirmed(path));
        evidence.observe(&assistant, "expected");
        assert!(evidence.confirmed(path));
        assert!(!evidence.confirmed(Path::new("other.jsonl")));
        evidence.reset(path);
        assert!(!evidence.confirmed(path));
    }

    #[tokio::test]
    async fn publication_stands_aside_for_lifecycle_and_roster_owners() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let state = AppState::new();
        let config = Arc::new(Mutex::new(AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            fresh_provider_session_id: Some("reserved".into()),
            ..Default::default()
        }));
        let lifecycle = state.lock_agent_lifecycle("agent-1").await;
        assert!(!tokio::time::timeout(
            std::time::Duration::from_secs(1),
            publish_resume_identity(&state, &config, 7, "reserved")
        )
        .await
        .expect("watcher must not wait for its joining owner")
        .unwrap());
        drop(lifecycle);
        let roster = wardian_core::agent_replacement::acquire_agent_roster_barrier(false)
            .unwrap()
            .unwrap();
        assert!(!tokio::time::timeout(
            std::time::Duration::from_secs(1),
            publish_resume_identity(&state, &config, 7, "reserved")
        )
        .await
        .expect("watcher must not wait for a roster owner")
        .unwrap());
        drop(roster);
        assert_eq!(
            config.lock().unwrap().pending_pi_session_id(),
            Some("reserved")
        );
    }

    #[test]
    fn partial_first_flush_records_wait_for_newline_before_confirmation() {
        let header = "{\"type\":\"session\",\"id\":\"expected\"}";
        let assistant = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\"}}";
        let mut line = String::new();
        assert_eq!(
            read_complete_record(&mut std::io::Cursor::new(header), &mut line),
            None
        );
        let mut evidence = HistoryConfirmation::default();
        let path = Path::new("owned.jsonl");
        evidence.reset(path);
        let mut reader = std::io::Cursor::new(format!("{header}\n{assistant}"));
        assert!(read_complete_record(&mut reader, &mut line).is_some());
        evidence.observe(&serde_json::from_str(&line).unwrap(), "expected");
        assert_eq!(read_complete_record(&mut reader, &mut line), None);
        assert!(!evidence.confirmed(path));
        let mut retry = std::io::Cursor::new(format!("{assistant}\n"));
        assert!(read_complete_record(&mut retry, &mut line).is_some());
        evidence.observe(&serde_json::from_str(&line).unwrap(), "expected");
        assert!(evidence.confirmed(path));
    }

    #[tokio::test]
    async fn publication_fences_stale_launch_and_retains_pending_after_failed_write() {
        let home = crate::control::test_support::TestWardianHome::new_async().await;
        let state = AppState::new();
        let mut agent = super::super::tests::agent_without_pty();
        agent.runtime_generation = Some(7);
        let config = agent.config.clone();
        *config.lock().unwrap() = AgentConfig {
            provider: "pi".into(),
            session_id: "agent-1".into(),
            fresh_provider_session_id: Some("reserved".into()),
            ..Default::default()
        };
        state.agents.lock().await.insert("agent-1".into(), agent);
        state.agent_order.lock().await.push("agent-1".into());
        assert!(!publish_resume_identity(&state, &config, 6, "reserved")
            .await
            .unwrap());
        let foreign_config = Arc::new(Mutex::new(config.lock().unwrap().clone()));
        assert!(
            !publish_resume_identity(&state, &foreign_config, 7, "reserved")
                .await
                .unwrap()
        );
        assert!(!publish_resume_identity(&state, &config, 7, "foreign")
            .await
            .unwrap());
        // A directory at the state-file path makes the actual atomic roster
        // write fail. The live identity must stay pending until a retry succeeds.
        let blocked = home.path().join("settings/state.json");
        std::fs::create_dir(&blocked).unwrap();
        assert!(publish_resume_identity(&state, &config, 7, "reserved")
            .await
            .is_err());
        assert_eq!(
            config.lock().unwrap().pending_pi_session_id(),
            Some("reserved")
        );
        std::fs::remove_dir(&blocked).unwrap();
        assert!(publish_resume_identity(&state, &config, 7, "reserved")
            .await
            .unwrap());
        let persisted: Vec<AgentConfig> = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("settings/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted[0].resume_session.as_deref(), Some("reserved"));
        assert_eq!(persisted[0].fresh_provider_session_id, None);
        assert_eq!(
            config.lock().unwrap().fresh_provider_session_id.as_deref(),
            Some("reserved")
        );
        assert!(!publish_resume_identity(&state, &config, 7, "reserved")
            .await
            .unwrap());
    }
}
