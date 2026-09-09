use super::*;
use crate::delivery::native_broker::NativeSessionSpec;
use crate::providers::ProviderFactory;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, Command};

#[path = "attachment.rs"]
mod attachment;
#[cfg(test)]
#[path = "owner_preparation_tests.rs"]
mod preparation_tests;

/// Called only by a new owner under the broker's exclusive owner gate, after
/// all previous readers have joined. Generic habitat refresh never calls this.
fn prepare_owner_habitat(
    workspace: &std::path::Path,
    class_name: &str,
    agent_id: &str,
) -> Result<(PathBuf, PathBuf), CodexSharedError> {
    let habitat = crate::utils::fs::prepare_habitat_workspace(workspace, class_name, agent_id)
        .map_err(CodexSharedError::unsupported)?;
    let wardian_home = crate::utils::get_wardian_home()
        .ok_or_else(|| CodexSharedError::unsupported("Wardian home unavailable"))?;
    let _preparation = crate::utils::codex_home::acquire_preparation(&wardian_home, agent_id)
        .map_err(CodexSharedError::unsupported)?;
    let home = crate::utils::codex_home::owner_preparation_home(&wardian_home, agent_id)
        .map_err(CodexSharedError::unsupported)?;
    // Recover against the authoritative physical path before relocating it or
    // projecting config. Generic refresh cannot enter this migration boundary.
    super::launch_config::recover_launch_config(&home)?;
    let compact_home = crate::utils::codex_home::prepare_compact_home(&wardian_home, agent_id)
        .map_err(CodexSharedError::unsupported)?;
    let codex_home = attachment::canonical_home(&compact_home)?;
    crate::utils::fs::ensure_codex_home_projection(&habitat, workspace, agent_id)
        .map_err(CodexSharedError::unsupported)?;
    if let crate::utils::codex_messaging::Registration::Unavailable(reason) =
        crate::utils::codex_messaging::ensure_managed_messaging(&wardian_home, agent_id)
            .map_err(CodexSharedError::unsupported)?
    {
        return Err(CodexSharedError::unsupported(format!(
            "managed messaging unavailable: {reason}"
        )));
    }
    Ok((habitat, codex_home))
}

/// Launch identity only. The ordinary TUI must be the first thread loader.
#[derive(Clone)]
pub struct CodexTuiAttachment {
    pub expected_resume_id: Option<String>,
    /// Effective launch preference; never written into AgentConfig.model.
    pub model_override: Option<String>,
    pub codex_home: PathBuf,
    pub generation: u64,
}

/// The existing native broker registry is the sole owner of this resource.
pub struct CodexSharedOwner {
    pub client: Arc<CodexSharedClient>,
    pub observed_version: String,
    pub attachment: CodexTuiAttachment,
    child: Mutex<Option<Child>>,
    interactive: bool,
    initial_context: String,
    policy: attachment::ExpectedPolicy,
    socket: attachment::OwnedSocket,
    launch_config: Mutex<Option<super::launch_config::LaunchConfigGuard>>,
    _memory: Option<wardian_core::memory::MemoryCapabilityLease>,
    #[cfg(windows)]
    job: Mutex<Option<win32job::Job>>,
}

impl std::fmt::Debug for CodexSharedOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexSharedOwner")
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl CodexSharedOwner {
    /// Caller holds the agent lifecycle/ownership gate. No process is started by push.
    pub async fn start(
        spec: &NativeSessionSpec,
        cancelled: impl std::future::Future<Output = ()>,
    ) -> Result<Arc<Self>, CodexSharedError> {
        if spec.provider != "codex" || spec.target_agent_id != spec.config.session_id {
            return Err(CodexSharedError::unsupported(
                "invalid Codex owner identity",
            ));
        }
        crate::manager::validate_session_values_for_launch(
            &spec.target_agent_id,
            spec.config.resume_session.as_deref(),
        )
        .map_err(CodexSharedError::unsupported)?;
        let wardian_home = crate::utils::get_wardian_home()
            .ok_or_else(|| CodexSharedError::unsupported("Wardian home unavailable"))?;
        tokio::pin!(cancelled);
        tokio::select! {
            biased;
            _ = &mut cancelled => return Err(CodexSharedError::unsupported("Codex owner startup cancelled before home preparation")),
            result = crate::manager::codex_stop::await_quiescent(&wardian_home, &spec.target_agent_id) => {
                result.map_err(CodexSharedError::unsupported)?;
            }
        }
        let (habitat, codex_home) = prepare_owner_habitat(
            &spec.workspace,
            &spec.config.agent_class,
            &spec.target_agent_id,
        )?;
        let socket = attachment::default_socket(&codex_home)?;
        // Never attach to an existing listener, nor remove another owner's socket.
        if std::fs::symlink_metadata(&socket).is_ok() {
            return Err(CodexSharedError::unsupported(
                "default Codex socket already exists; previous generation must exit first",
            ));
        }
        let provider = ProviderFactory::resolve("codex").map_err(CodexSharedError::unsupported)?;
        let (program, prefix_args) = provider.get_executable();
        let mut args = crate::providers::CodexProvider::new()
            .shared_server_args(&spec.config)
            .map_err(CodexSharedError::unsupported)?;
        append_runtime_context(&mut args, spec, &habitat, &codex_home)?;
        let mut policy = attachment::ExpectedPolicy::from_server_args(&args)?;
        let configured_effort = spec.config.codex_config().reasoning_effort;
        let mut model_override = None;
        let generated_args = args.clone();
        drop(args.splice(0..0, prefix_args.clone()));
        args.extend(["--listen".into(), "unix://".into()]);
        let mut command = Command::new(&program);
        command
            .args(args)
            .current_dir(&spec.workspace)
            .env("CODEX_HOME", &codex_home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        crate::manager::apply_managed_cli_path_to_process(&mut command);
        crate::manager::apply_process_provider_runtime_env("codex", &mut command)
            .map_err(CodexSharedError::unsupported)?;
        let memory = crate::manager::headless::apply_headless_identity_env(
            &mut command,
            &spec.target_agent_id,
            Some(&spec.target_agent_id),
            crate::utils::memory_feature_enabled(),
        );
        for (key, value) in crate::manager::worktree_build_env(&spec.config) {
            command.env(key, value);
        }
        #[cfg(windows)]
        let job = crate::utils::process::create_kill_on_close_job("Codex shared owner")
            .map_err(CodexSharedError::unsupported)?;
        // Only an ordinary interactive TUI needs the temporary file overlay.
        // The daemon always retains its explicit policy arguments.
        let mut launch_config = if spec.config.is_off {
            None
        } else {
            Some(super::launch_config::prepare_launch_config(
                &codex_home,
                &generated_args,
            )?)
        };
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                restore_launch_overlay(&mut launch_config)?;
                return Err(CodexSharedError::unsupported(format!(
                    "Codex owner spawn failed: {error}"
                )));
            }
        };
        #[cfg(windows)]
        if let Err(error) = child
            .id()
            .ok_or_else(|| "Codex owner PID missing".to_string())
            .and_then(|pid| {
                crate::utils::process::assign_pid_to_job(&job, pid, "Codex shared owner")
            })
        {
            if let Some(pid) = child.id() {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::utils::process::force_kill_process_tree(pid)
                })
                .await;
            }
            terminate_starting_child(&mut child).await;
            restore_launch_overlay(&mut launch_config)?;
            return Err(CodexSharedError::unsupported(error));
        }
        let mut connected = None;
        let mut owned_socket = None;
        let start = async {
            // A private socket must appear while our daemon is alive. No controller
            // RPC is retried, and a proxy handshake failure is final.
            let wait_socket = wait_for_socket(
                &socket,
                tokio::time::Instant::now() + STARTUP_TIMEOUT,
                || attachment::child_alive(&mut child),
            );
            tokio::select! {
                biased;
                _ = &mut cancelled => return Err(CodexSharedError::unsupported("Codex owner startup cancelled")),
                result = wait_socket => result?,
            }
            let mut proxy_command = Command::new(&program);
            proxy_command.args(&prefix_args).current_dir(&spec.workspace).env("CODEX_HOME", &codex_home);
            // Reuse the owner's complete prepared environment, including the
            // identity capability retained by its memory lease.
            for (key, value) in command.as_std().get_envs() {
                if let Some(value) = value {
                    proxy_command.env(key, value);
                } else {
                    proxy_command.env_remove(key);
                }
            }
            let client = CodexSharedClient::connect_proxy(
                spec.target_agent_id.clone(), spec.generation, proxy_command, &socket, &mut cancelled,
            ).await?;
            connected = Some(client.clone());
            let initialize = async {
                let observed_version = if spec.config.is_off {
                    client.initialize_queued(&codex_home).await?
                } else {
                    client.initialize(&codex_home).await?
                };
                attachment::require_local_version(&observed_version)?;
                attachment::child_alive(&mut child)?;
                owned_socket = Some(attachment::OwnedSocket::capture(&socket)?);
                // Fresh private owner: the interactive TUI alone may load a thread.
                attachment::require_empty(&client).await?;
                model_override = super::launch_model::resolve_launch_model(
                    &client, spec.config.model.as_deref(), configured_effort.as_deref(),
                    spec.config.resume_session.as_deref(), &spec.workspace,
                ).await?;
                policy.expect_launch_model(model_override.as_deref())?;
                // Read-only preference lookup must not take the first-loader role.
                attachment::require_empty(&client).await?;
                if spec.config.is_off {
                    let response = if let Some(id) = spec.config.resume_session.as_deref().filter(|id| !id.is_empty()) {
                        client.request_with_timeout("thread/resume", policy.background_resume_params(id), STARTUP_TIMEOUT).await?
                    } else {
                        client.request_with_timeout("thread/start", json!({"cwd":spec.workspace}), STARTUP_TIMEOUT).await?
                    };
                    policy.validate(&response)?;
                    let thread_id = client.bind(&response)?;
                    if spec.config.resume_session.as_deref().is_some_and(|id| id != thread_id) {
                        return Err(CodexSharedError::unsupported("background resume returned a different thread"));
                    }
                    client.install_initial_context(&initialization_context(
                        &spec.target_agent_id, &spec.config.session_name,
                    )).await?;
                }
                attachment::child_alive(&mut child)?;
                Ok::<_, CodexSharedError>((client.clone(), observed_version))
            };
            tokio::select! {
                biased;
                _ = &mut cancelled => Err(CodexSharedError::unsupported("Codex initialization cancelled; no task submitted")),
                result = initialize => result,
            }
        }.await;
        match start {
            Ok((client, observed_version)) => Ok(Arc::new(Self {
                client,
                observed_version,
                attachment: CodexTuiAttachment {
                    model_override,
                    expected_resume_id: spec
                        .config
                        .resume_session
                        .clone()
                        .filter(|id| !id.is_empty()),
                    codex_home,
                    generation: spec.generation,
                },
                child: Mutex::new(Some(child)),
                interactive: !spec.config.is_off,
                initial_context: initialization_context(
                    &spec.target_agent_id,
                    &spec.config.session_name,
                ),
                policy,
                socket: owned_socket.expect("initialized owner captured its socket"),
                launch_config: Mutex::new(launch_config),
                _memory: memory,
                #[cfg(windows)]
                job: Mutex::new(Some(job)),
            })),
            Err(error) => {
                if let Some(client) = connected {
                    client.close().await;
                }
                #[cfg(windows)]
                drop(job);
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                terminate_starting_child(&mut child).await;
                restore_launch_overlay(&mut launch_config)?;
                if let Some(socket) = owned_socket {
                    socket.remove_after_exit()?;
                }
                Err(error)
            }
        }
    }

    /// Exclusive-startup evidence only, never continuous subscriber attestation.
    /// Caller holds the owner gate and verifies the captured PTY is still alive.
    pub(crate) async fn finalize_interactive(
        &self,
        mut tui_alive: impl FnMut() -> Result<(), CodexSharedError>,
    ) -> Result<Value, CodexSharedError> {
        if !self.interactive || self.client.receipt("already_bound").is_ok() {
            return Err(CodexSharedError::unsupported(
                "interactive attachment requires an unbound fresh owner",
            ));
        }
        let mut child = self.child.lock().await;
        let child = child
            .as_mut()
            .ok_or_else(|| CodexSharedError::unsupported("Codex owner already stopped"))?;
        let mut alive = || {
            attachment::child_alive(child)?;
            tui_alive()
        };
        let result = tokio::time::timeout(STARTUP_TIMEOUT, async {
            let id = attachment::wait_for_tui_thread(
                &self.client,
                self.attachment.expected_resume_id.as_deref(),
                &mut alive,
            )
            .await?;
            alive()?;
            attachment::inject_initial_context(&self.client, &id, &self.initial_context).await?;
            alive()?;
            let response = self
                .client
                .request_with_timeout("thread/resume", json!({"threadId":id}), STARTUP_TIMEOUT)
                .await?;
            if response["thread"]["id"].as_str() != Some(id.as_str()) {
                return Err(CodexSharedError::unsupported(
                    "attachment resume changed thread identity",
                ));
            }
            self.policy.validate(&response)?;
            attachment::require_observed_thread(&self.client, &id).await?;
            if response["thread"]["canAcceptDirectInput"] != true {
                return Err(CodexSharedError::unsupported(
                    "TUI thread lacks direct-input capability",
                ));
            }
            alive()?;
            // Both clients have loaded the selected policy. Restore file leaves
            // before the broker may publish any capable native binding.
            let mut overlay = self.launch_config.lock().await;
            restore_launch_overlay(&mut overlay)?;
            alive()?;
            Ok(response)
        })
        .await
        .map_err(|_| {
            CodexSharedError::unsupported("ordinary Codex TUI did not attach before deadline")
        })?;
        result
    }

    /// Return only after process termination; registry removal must follow this result.
    pub async fn shutdown(&self) -> Result<(), CodexSharedError> {
        self.client.close().await;
        let mut owned = self.child.lock().await;
        if let Some(child) = owned.as_mut() {
            #[cfg(windows)]
            {
                self.job.lock().await.take();
            }
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            if child
                .try_wait()
                .map_err(|_| CodexSharedError::uncertain("cannot inspect owner exit"))?
                .is_none()
            {
                child
                    .kill()
                    .await
                    .map_err(|_| CodexSharedError::uncertain("owner termination failed"))?;
            }
            child
                .wait()
                .await
                .map_err(|_| CodexSharedError::uncertain("owner exit was not observed"))?;
            owned.take();
        }
        let mut overlay = self.launch_config.lock().await;
        restore_launch_overlay(&mut overlay)?;
        self.socket.remove_after_exit()?;
        Ok(())
    }
}

/// Keep a failed restore armed for an explicit retry or the guard's token-safe
/// Drop fallback. Call only after policy validation or joined reader cleanup.
fn restore_launch_overlay(
    overlay: &mut Option<super::launch_config::LaunchConfigGuard>,
) -> Result<(), CodexSharedError> {
    if let Some(guard) = overlay.as_mut() {
        guard.restore()?;
        overlay.take();
    }
    Ok(())
}

/// Wait only for the private socket path to appear while the captured child
/// remains alive. Presence is not a handshake or attachment receipt. The caller
/// owns cancellation and joined process cleanup; this never spawns or connects.
pub(super) async fn wait_for_socket(
    socket: &std::path::Path,
    deadline: tokio::time::Instant,
    mut alive: impl FnMut() -> Result<(), CodexSharedError>,
) -> Result<(), CodexSharedError> {
    loop {
        alive()?;
        if std::fs::symlink_metadata(socket).is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CodexSharedError::unsupported(
                "Codex local socket startup timed out",
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Initialization is a non-cancelled owner task holding the generation gate.
/// Never release that gate after a best-effort kill without observing exit.
async fn terminate_starting_child(child: &mut Child) {
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.start_kill();
        if matches!(
            tokio::time::timeout(Duration::from_secs(1), child.wait()).await,
            Ok(Ok(_))
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn append_runtime_context(
    args: &mut Vec<String>,
    spec: &NativeSessionSpec,
    habitat: &std::path::Path,
    codex_home: &std::path::Path,
) -> Result<(), CodexSharedError> {
    if crate::utils::memory_feature_enabled() {
        let brief = wardian_core::memory::MemoryStore::from_default_home()
            .ok()
            .and_then(|store| {
                store
                    .compile_brief(
                        &wardian_core::memory::MemoryActor::agent(&spec.target_agent_id),
                        &spec.target_agent_id,
                        Some(spec.workspace.to_string_lossy().as_ref()),
                        "codex",
                        &format!("codex-shared:{}", spec.generation),
                        spec.config.resume_session.is_some(),
                        12_000,
                    )
                    .ok()
            });
        let text = brief
            .as_ref()
            .and_then(|brief| (!brief.is_empty).then_some(brief.context_text.as_str()));
        crate::utils::fs::append_habitat_memory_instructions(habitat, text)
            .map_err(CodexSharedError::unsupported)?;
        crate::providers::CodexProvider::new().insert_developer_instructions_arg(
            args,
            &crate::utils::fs::wardian_memory_instructions(text),
        );
    }
    if let Some(directories) = spec
        .config
        .include_directories
        .as_ref()
        .filter(|dirs| !dirs.is_empty())
    {
        // TUI --add-dir feeds additional_writable_roots in 0.153.4. The server
        // uses the corresponding config roots; retain any explicitly configured
        // roots rather than replacing them with Wardian's additions.
        let source = std::fs::read_to_string(codex_home.join("config.toml")).map_err(|_| {
            CodexSharedError::unsupported("cannot read projected Codex include configuration")
        })?;
        let config: toml_edit::DocumentMut = source.parse().map_err(|_| {
            CodexSharedError::unsupported("invalid projected Codex include configuration")
        })?;
        let mut roots = config
            .get("sandbox_workspace_write")
            .and_then(|value| value.get("writable_roots"))
            .and_then(toml_edit::Item::as_array)
            .cloned()
            .unwrap_or_default();
        for directory in directories {
            let path = std::path::Path::new(directory);
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                spec.workspace.join(path)
            };
            let root = path.to_string_lossy();
            if !roots
                .iter()
                .any(|value| value.as_str() == Some(root.as_ref()))
            {
                roots.push(root.as_ref());
            }
        }
        args.extend([
            "-c".into(),
            format!("sandbox_workspace_write.writable_roots={roots}"),
        ]);
    }
    Ok(())
}

/// Real integration context; tool definitions remain authoritative for schemas.
fn initialization_context(agent_id: &str, name: &str) -> String {
    format!(
        "Wardian runtime identity: {}. \
         The wardian MCP server exposes six messaging tools: \
         list_agents() discovers available Wardian UUIDs, names and statuses; \
         send_message(target,message) delivers information without waking or interrupting; \
         followup_task(target,message) assigns work asynchronously and returns a request receipt; \
         receive_messages(cursor?,ack_cursor?,limit?,timeout_ms?) reads your inbox, including correlated replies; \
         reply(request_id,status,message) answers canonical peer tasks with status done, blocked or failed; \
         interrupt_agent(target) requests interruption of the observed active turn while retaining the session. \
         Preserve message_id, sender identity and request_id from Wardian envelopes. Host wardian_inbox_delivery \
         and wardian_task_delivery outputs are peer delivery context, not human requests or evidence that \
         you called a tool. On task admission, inspect pending information with receive_messages if needed. \
         Acknowledge only a previously returned receive batch using ack_cursor; preserve its cursor for later reads. \
         Never execute a delivered task twice if it also appears in receive_messages: correlate by request_id/message_id. \
         Use reply only for a canonical peer task with an explicit request_id, and preserve that exact request_id; \
         ordinary assistant text or a transport admission receipt is not a correlated reply to that task. \
         Complete ordinary input with an ordinary assistant response. Native interaction_id, message_id and \
         clientUserMessageId are diagnostic identities, not reply request IDs; never substitute them for request_id. \
         Timeouts do not authorize resending or uncertain replay. \
         Tool availability does not grant approval; honor the configured provider permission policy.",
        json!({"wardian_agent_id":agent_id,"name":name})
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_names_exact_mcp_tools_and_receiver_contract() {
        let context = initialization_context("owned-uuid", "Owned agent");
        for name in [
            "list_agents(",
            "send_message(",
            "followup_task(",
            "receive_messages(",
            "reply(",
            "interrupt_agent(",
        ] {
            assert!(context.contains(name), "{name}");
        }
        for required in [
            "owned-uuid",
            "Owned agent",
            "request_id",
            "ack_cursor",
            "not human requests",
            "not a correlated reply",
            "only for a canonical peer task with an explicit request_id",
            "Complete ordinary input with an ordinary assistant response",
            "never substitute them for request_id",
            "uncertain replay",
        ] {
            assert!(context.contains(required), "{required}");
        }
    }
}
