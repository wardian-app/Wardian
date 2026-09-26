use crate::delivery::opencode_http::OpenCodeHttpLaunchPlan;
use crate::providers::antigravity::{
    changed_workspace_conversation, AntigravityConversationMessage, AntigravityProvider,
};
use crate::providers::claude::{
    classify_claude_user_event, claude_output_has_bypass_permissions_consent_prompt,
    effective_claude_permission_mode, ClaudeUserEventKind,
};
use crate::providers::codex::CodexProvider;
use crate::providers::pi::PiProvider;
use crate::providers::transcript::{
    bind_pi_watch_message, extract_transcript_message, CodexWatchBindingState,
};
use crate::providers::ProviderFactory;
use crate::state::{ActiveAgent, AgentWatchState, AppState};
use crate::utils::fs::*;
use crate::utils::logging::{log_debug, log_terminal_trace_bytes, log_terminal_trace_note};
use crate::utils::PtyUtf8Decoder;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, Write};
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::control::{ProviderInputReadiness, WatchTranscriptMessage};
use wardian_core::models::{AgentChatRole, AgentConfig, AgentEvent, ProviderConfig};

use super::codex_onboarding::{
    finalize_synchronous_codex, CodexAttachmentCompletion, CodexAttachmentCompletionContext,
    SpawnPublication, SpawnedAgent, SynchronousCodexFinalizationContext,
};
use super::codex_terminal_theme::CodexTerminalThemeProbeResponder;

use super::claude::{
    claude_log_paths, claude_permission_hook_matches_session, claude_project_dir_name,
    discover_claude_log_for_session_name,
};
use super::codex::{codex_provider_session_is_excluded, codex_session_file_path};
use super::opencode::{
    opencode_interactive_env, opencode_status_from_title, OpenCodeSessionDiscovery,
};
use super::session_identity::{
    apply_provider_identity, expected_caller_owned_identity, ProviderIdentityOutcome,
};
use super::{
    apply_agent_event, apply_agent_event_with_policy, apply_agent_status_event,
    apply_agent_status_event_with_policy, apply_terminal_identity_env, debug_preview_bytes,
    extract_terminal_titles, finalize_interactive_spawn_args, interactive_provider_args,
    interactive_provider_cwd, interactive_provider_launch, set_agent_status,
    ProviderStatusEventPolicy,
};
use crate::providers::gemini::gemini_status_from_title;

const OUTPUT_READY_EMIT_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);
const ANTIGRAVITY_TRANSCRIPT_OVERLAP_STEPS: u64 = 16;
const PROVIDER_SPAWN_LEASE_DURATION: chrono::Duration = chrono::Duration::minutes(20);
const PROVIDER_SPAWN_LEASE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);

fn interactive_provider_launch_cwd(
    provider: &str,
    session_id: &str,
    habitat_root: Option<&std::path::Path>,
    workspace_cwd: &std::path::Path,
    provider_cwd: &std::path::Path,
    is_restored: bool,
) -> Result<std::path::PathBuf, String> {
    if provider == "pi" && (is_restored || provider_cwd == workspace_cwd) {
        // A habitat alias changes the path Pi uses as its saved project
        // identity, including when it resolves to the same workspace. A fresh
        // long-path session is the exception: it starts in the habitat and
        // records the short alias as its project identity.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            if provider_cwd.as_os_str().encode_wide().count() > 258 {
                return Err("Pi's saved project directory exceeds the Windows PTY launch path limit; restoring from a short alias would change Pi's project identity and prompt to fork the session".into());
            }
        }
        return Ok(provider_cwd.to_path_buf());
    }
    Ok(crate::utils::codex_home::prepare_habitat_cwd_alias(
        session_id,
        habitat_root,
        workspace_cwd,
        provider_cwd,
    )?
    .unwrap_or_else(|| provider_cwd.to_path_buf()))
}

fn pi_session_project_cwd(
    agent_id: &str,
    workspace_cwd: &std::path::Path,
    habitat_root: Option<&std::path::Path>,
    session_file: Option<&std::path::Path>,
    is_restored: bool,
) -> std::path::PathBuf {
    if is_restored && session_file.is_some() {
        if let Some(session_file) = session_file {
            use std::io::BufRead;
            if let Ok(file) = std::fs::File::open(session_file) {
                let mut header = String::new();
                if std::io::BufReader::new(file).read_line(&mut header).is_ok() {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&header) {
                        if let Some(saved) = parsed.get("cwd").and_then(|cwd| cwd.as_str()) {
                            let saved = std::path::Path::new(saved);
                            if saved == workspace_cwd {
                                return workspace_cwd.to_path_buf();
                            }
                            if let Some(habitat_workspace) =
                                habitat_root.map(super::habitat_workspace_cwd)
                            {
                                if saved == habitat_workspace {
                                    return habitat_workspace;
                                }
                            }
                            if habitat_root.is_some_and(|root| {
                                crate::utils::codex_home::is_owned_habitat_workspace_alias(
                                    agent_id, root, saved,
                                )
                            }) {
                                return saved.to_path_buf();
                            }
                        }
                    }
                }
            }
        }
        return workspace_cwd.to_path_buf();
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if workspace_cwd.as_os_str().encode_wide().count() > 258 {
            if let Some(habitat_root) = habitat_root {
                return super::habitat_workspace_cwd(habitat_root);
            }
        }
    }
    workspace_cwd.to_path_buf()
}

/// The caller owns publication after the spawn watcher starts. Cancellation
/// before roster commit must stop renewal without claiming provider exit.
pub(crate) struct SpawnPublicationDisposition {
    failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    committed: bool,
}

impl SpawnPublicationDisposition {
    pub(crate) fn new() -> Self {
        Self {
            failed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            committed: false,
        }
    }

    pub(crate) fn failure_signal(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.failed.clone()
    }

    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }

    pub(crate) fn fail(&mut self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Release);
        self.committed = true;
    }
}

impl Drop for SpawnPublicationDisposition {
    fn drop(&mut self) {
        if !self.committed {
            self.failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

#[derive(Default)]
struct SpawnPublicationGate {
    unpublished_failure: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    registration: Option<std::sync::Arc<RegistrationPublicationState>>,
}

/// Registration may put a runtime in the map before its roster and provider
/// attachment are committed. Cancellation and errors must stop lease renewal.
#[derive(Default)]
pub(crate) struct RegistrationPublicationState(std::sync::atomic::AtomicU8);

impl RegistrationPublicationState {
    const PENDING: u8 = 0;
    const COMMITTED: u8 = 1;
    const FAILED: u8 = 2;

    pub(crate) fn commit(&self) {
        let _ = self.0.compare_exchange(
            Self::PENDING,
            Self::COMMITTED,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }

    pub(crate) fn fail(&self) {
        let _ = self.0.compare_exchange(
            Self::PENDING,
            Self::FAILED,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }

    fn state(&self) -> u8 {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[cfg(test)]
impl RegistrationPublicationState {
    pub(crate) fn is_failed(&self) -> bool {
        self.state() == Self::FAILED
    }
}

/// Build the shared-owner TUI invocation without config overrides, preserving
/// Codex's ordinary local-daemon discovery for both fresh and resumed threads.
pub(super) fn codex_shared_tui_args(
    mut prefix_args: Vec<String>,
    model_override: Option<&str>,
    expected_resume_id: Option<&str>,
    workspace: &std::path::Path,
) -> Vec<String> {
    if let Some(model) = model_override {
        prefix_args.extend(["--model".into(), model.into()]);
    }
    if let Some(id) = expected_resume_id {
        prefix_args.extend(["resume".into(), id.into()]);
    }
    prefix_args.extend(["--cd".into(), workspace.to_string_lossy().into_owned()]);
    prefix_args
}

type PendingMemoryInjection = (
    wardian_core::memory::MemoryStore,
    wardian_core::memory::CompiledMemoryBrief,
    String,
    String,
);

/// Retain Pi generation cleanup across fallible PTY setup after the bridge
/// plan has transferred ownership to the broker. The reader thread disarms
/// this guard once it can dispose the generation on process exit.
struct PiBridgeSpawnGuard {
    broker: std::sync::Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    agent_id: String,
    generation: u64,
    armed: bool,
}

fn pi_bridge_child_handoff_code(process_id: Option<u32>) -> &'static str {
    match process_id {
        Some(process_id) if process_id != 0 => "process_registered",
        Some(_) => "process_id_zero",
        None => "process_id_unavailable",
    }
}

fn retain_pi_fresh_provider_session(config: &mut AgentConfig) {
    if let Some(fresh) = config.fresh_provider_session_id.clone() {
        config.resume_session = Some(fresh);
    }
}

impl PiBridgeSpawnGuard {
    fn new(
        broker: std::sync::Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
        agent_id: String,
        generation: u64,
    ) -> Self {
        Self {
            broker,
            agent_id,
            generation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PiBridgeSpawnGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let broker = self.broker.clone();
        let agent_id = self.agent_id.clone();
        let generation = self.generation;
        tauri::async_runtime::spawn(async move {
            let _ = broker
                .dispose_pi_generation(&agent_id, Some(generation))
                .await;
        });
    }
}

fn record_pending_memory_injection(
    pending: &mut Option<PendingMemoryInjection>,
    agent_id: &str,
    provider: &str,
) -> bool {
    let Some((store, brief, workspace, process_key)) = pending.take() else {
        return false;
    };
    if let Err(error) = store.record_injection(
        &wardian_core::memory::MemoryActor::agent(agent_id),
        agent_id,
        Some(&workspace),
        provider,
        &process_key,
        &brief,
    ) {
        log_debug(&format!(
            "[Wardian] memory injection receipt unavailable for {agent_id}: {error}"
        ));
    }
    true
}

fn opencode_http_conflicting_custom_arg(argument: &str) -> bool {
    let argument = argument.trim().to_ascii_lowercase();
    matches!(
        argument.as_str(),
        "serve"
            | "attach"
            | "run"
            | "acp"
            | "--hostname"
            | "--port"
            | "--session"
            | "--continue"
            | "--fork"
            | "--dir"
            | "--config"
    ) || argument.starts_with("--hostname=")
        || argument.starts_with("--port=")
        || argument.starts_with("--session=")
        || argument.starts_with("--dir=")
        || argument.starts_with("--config=")
}

fn opencode_http_launch_identity_is_valid(session: Option<&str>) -> bool {
    session.is_none_or(|session| {
        session.starts_with("ses_")
            && session.len() <= 256
            && session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

/// Reserve the per-generation listener before every ordinary OpenCode TUI
/// spawn. A fresh TUI has no provider identity yet, so its pending owner is
/// rebound only after the launch-scoped discovery returns one exact session.
async fn prepare_opencode_http_launch(
    app_state: &AppState,
    config: &AgentConfig,
    provider_generation: u64,
) -> Option<OpenCodeHttpLaunchPlan> {
    if config.provider != "opencode"
        || !opencode_http_launch_identity_is_valid(config.resume_session.as_deref())
    {
        return None;
    }
    let custom_args = match config.custom_args.as_deref().map(str::trim) {
        Some(custom) if !custom.is_empty() => shlex::split(custom)?,
        _ => Vec::new(),
    };
    if custom_args
        .iter()
        .any(|argument| opencode_http_conflicting_custom_arg(argument))
    {
        return None;
    }
    let runtime_generation = app_state
        .terminal_sessions
        .next_runtime_generation(&config.session_id)
        .await
        .ok()?;
    let requested_port = config
        .opencode_config()
        .port
        .or(config.opencode_port)
        .filter(|port| *port != 0);
    let listener = std::net::TcpListener::bind(("127.0.0.1", requested_port.unwrap_or(0))).ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    let plan = OpenCodeHttpLaunchPlan::new(provider_generation, runtime_generation, port).ok()?;
    Some(plan)
}

async fn wait_for_opencode_http_listener(
    app: &AppHandle,
    agent_id: &str,
    runtime_generation: u64,
    endpoint: &reqwest::Url,
) -> bool {
    let Some(port) = endpoint.port() else {
        return false;
    };
    let address = format!("127.0.0.1:{port}");
    for _ in 0..120 {
        let runtime_matches = app
            .state::<AppState>()
            .terminal_sessions
            .broker_state(agent_id)
            .await
            .is_ok_and(|state| state.runtime_generation == runtime_generation);
        if !runtime_matches {
            return false;
        }
        if tokio::net::TcpStream::connect(&address).await.is_ok() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

struct OpenCodeHttpLaunchContext {
    app: AppHandle,
    broker: std::sync::Arc<crate::delivery::native_broker::NativeDeliveryBroker>,
    agent_id: String,
    plan: OpenCodeHttpLaunchPlan,
    provider_session_id: String,
    process_id: u32,
    workspace: std::path::PathBuf,
    config_fingerprint: String,
    runtime_generation: u64,
    provider_generation: u64,
}

async fn register_opencode_http_after_launch(context: OpenCodeHttpLaunchContext) {
    let OpenCodeHttpLaunchContext {
        app,
        broker,
        agent_id,
        plan,
        provider_session_id,
        process_id,
        workspace,
        config_fingerprint,
        runtime_generation,
        provider_generation,
    } = context;
    let endpoint = plan.endpoint().clone();
    if !wait_for_opencode_http_listener(&app, &agent_id, runtime_generation, &endpoint).await {
        broker
            .fail_opencode_http(&agent_id, provider_generation)
            .await;
        return;
    }

    let process_identity = format!("pid:{process_id}:runtime:{runtime_generation}");
    let listener_identity = format!(
        "loopback:127.0.0.1:{}:pid:{process_id}:runtime:{runtime_generation}",
        endpoint.port().unwrap_or_default()
    );
    if let Err(error) = broker
        .register_opencode_http(
            agent_id.clone(),
            plan,
            provider_session_id,
            process_identity,
            listener_identity,
            workspace,
            config_fingerprint,
        )
        .await
    {
        broker
            .fail_opencode_http(&agent_id, provider_generation)
            .await;
        log_debug(&format!(
            "[Wardian] OpenCode HTTP owner unavailable after launch: {error}"
        ));
        return;
    }

    // Owner registration can complete after the provider's one startup/idle
    // observation. Reuse the canonical status trigger so a task that remained
    // pending during the handshake gets one dispatch opportunity without
    // bypassing the normal busy, generation, or claim checks.
    let state = app.state::<crate::state::AppState>();
    crate::control::dispatch_agent_messaging_from_status_observation(
        Some(&app),
        state.inner(),
        &agent_id,
    )
    .await;
}

/// Selects the verified Antigravity conversation created by this launch for
/// log discovery and whether it should be persisted as the resume identity.
/// A workspace mapping that existed before launch belongs to the prior
/// provider conversation, so it must not be replayed by a fresh launch.
/// Where the mock provider mirrors its event stream.
///
/// Real providers are observed through a log they own, and the chat transcript
/// reads normalized events back from that log alone. The mock provider writes
/// only to the PTY, so without a log of its own its tool calls could never
/// reach the transcript and the chat surface stayed untestable offline.
fn mock_transcript_log_path(session_id: &str) -> Option<std::path::PathBuf> {
    // Reuses the conversations directory's own safety check on the id rather
    // than joining an unvalidated path component under the Wardian home.
    wardian_core::paths::agent_conversations_dir(session_id)
        .and_then(|dir| dir.parent().map(|agent_dir| agent_dir.to_path_buf()))
        .map(|dir| dir.join("mock-transcript.jsonl"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PiLogBaseline {
    path: std::path::PathBuf,
    cursor: PiLogCursor,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PiLogCursor {
    offset: u64,
    identity: Option<PiFileIdentity>,
    boundary_start: u64,
    boundary: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PiFileIdentity(u64, u64);

fn pi_file_identity(metadata: &std::fs::Metadata) -> Option<PiFileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(PiFileIdentity(metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Stable Rust does not expose Windows' by-handle file index yet.
        // Creation time is stable across appends, while the boundary check below
        // also detects in-place rewrites and same-timestamp replacements.
        Some(PiFileIdentity(metadata.creation_time(), 0))
    }
}

fn pi_log_boundary(file: &mut std::fs::File, offset: u64) -> Option<(u64, Vec<u8>)> {
    const BOUNDARY_BYTES: u64 = 4096;
    let boundary_start = offset.saturating_sub(BOUNDARY_BYTES);
    let boundary_len = offset.saturating_sub(boundary_start);
    file.seek(std::io::SeekFrom::Start(boundary_start)).ok()?;
    let mut boundary = Vec::new();
    std::io::Read::by_ref(file)
        .take(boundary_len)
        .read_to_end(&mut boundary)
        .ok()?;
    (boundary.len() as u64 == boundary_len).then_some((boundary_start, boundary))
}

fn refresh_pi_log_boundary(file: &mut std::fs::File, cursor: &mut PiLogCursor) -> Option<()> {
    let (boundary_start, boundary) = pi_log_boundary(file, cursor.offset)?;
    cursor.boundary_start = boundary_start;
    cursor.boundary = boundary;
    Some(())
}

fn pi_log_baseline(
    session_dir: &std::path::Path,
    provider_session_id: &str,
) -> Option<PiLogBaseline> {
    let path = PiProvider::session_file(session_dir, provider_session_id)?;
    let mut file = std::fs::File::open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    let mut cursor = PiLogCursor {
        offset: metadata.len(),
        identity: pi_file_identity(&metadata),
        ..Default::default()
    };
    refresh_pi_log_boundary(&mut file, &mut cursor)?;
    Some(PiLogBaseline { path, cursor })
}

fn restored_pi_log_baseline(config: &AgentConfig, is_restored: bool) -> Option<PiLogBaseline> {
    if !is_restored || config.provider != "pi" {
        return None;
    }
    let provider_session_id = config
        .resume_session
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())?;
    let session_dir = PiProvider::session_dir(&config.session_id)?;
    pi_log_baseline(&session_dir, provider_session_id)
}

fn open_pi_log_at_cursor(
    path: &std::path::Path,
    cursor: &mut PiLogCursor,
) -> Option<std::fs::File> {
    let mut file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let identity = pi_file_identity(&metadata);
    let identity_changed = cursor
        .identity
        .zip(identity)
        .is_some_and(|(before, after)| before != after);
    let boundary_changed = if cursor.boundary.is_empty() {
        false
    } else {
        let boundary_end = cursor
            .boundary_start
            .saturating_add(cursor.boundary.len() as u64);
        if metadata.len() < boundary_end {
            true
        } else {
            file.seek(std::io::SeekFrom::Start(cursor.boundary_start))
                .ok()?;
            let mut current_boundary = vec![0; cursor.boundary.len()];
            file.read_exact(&mut current_boundary).ok()?;
            current_boundary != cursor.boundary
        }
    };
    let reset = identity_changed || boundary_changed || metadata.len() < cursor.offset;
    if reset {
        cursor.offset = 0;
        cursor.boundary_start = 0;
        cursor.boundary.clear();
    }
    cursor.identity = identity;
    file.seek(std::io::SeekFrom::Start(cursor.offset)).ok()?;
    Some(file)
}

fn antigravity_watcher_conversation(
    existing: Option<String>,
    workspace_before: Option<&str>,
    discover: impl FnOnce() -> Option<String>,
) -> (Option<String>, bool) {
    if existing.is_some() {
        return (existing, false);
    }

    let discovered = discover();
    let conversation_id = changed_workspace_conversation(workspace_before, discovered.as_deref());
    let capture_identity = conversation_id.is_some();
    (conversation_id, capture_identity)
}

#[derive(Default)]
struct OutputReadyEmitGate {
    last_emit_at: Option<std::time::Instant>,
    delayed_emit_scheduled: bool,
}

impl OutputReadyEmitGate {
    fn after_buffer_append(&mut self, now: std::time::Instant) -> OutputReadyEmitAction {
        let elapsed = self
            .last_emit_at
            .map(|last_emit_at| now.saturating_duration_since(last_emit_at));
        if elapsed.is_none_or(|elapsed| elapsed >= OUTPUT_READY_EMIT_MIN_INTERVAL) {
            self.last_emit_at = Some(now);
            self.delayed_emit_scheduled = false;
            return OutputReadyEmitAction::EmitNow;
        }

        if self.delayed_emit_scheduled {
            return OutputReadyEmitAction::Suppress;
        }

        self.delayed_emit_scheduled = true;
        OutputReadyEmitAction::ScheduleAfter(OUTPUT_READY_EMIT_MIN_INTERVAL - elapsed.unwrap())
    }

    fn finish_delayed_emit(&mut self, buffer_has_output: bool, now: std::time::Instant) -> bool {
        self.delayed_emit_scheduled = false;
        if !buffer_has_output {
            return false;
        }

        let elapsed = self
            .last_emit_at
            .map(|last_emit_at| now.saturating_duration_since(last_emit_at));
        if elapsed.is_none_or(|elapsed| elapsed >= OUTPUT_READY_EMIT_MIN_INTERVAL) {
            self.last_emit_at = Some(now);
            return true;
        }

        false
    }
}

#[derive(Debug, PartialEq, Eq)]
enum OutputReadyEmitAction {
    EmitNow,
    ScheduleAfter(std::time::Duration),
    Suppress,
}

/// Antigravity's transcript marks every planner step as `DONE`, including
/// script execution and interim progress prose. A visible compose prompt is
/// the provider's actual end-of-turn boundary. This gate observes the PTY
/// output only while the submitted turn is processing and consumes the first
/// ready prompt, so terminal redraws cannot emit duplicate completions.
#[derive(Default)]
struct AntigravityTurnCompletionGate {
    tracking_processing_turn: bool,
    output_since_turn_started: String,
}

impl AntigravityTurnCompletionGate {
    fn observe_output(&mut self, provider_name: &str, current_status: &str, output: &str) -> bool {
        if provider_name != "antigravity" || current_status != "Processing..." {
            self.reset();
            return false;
        }

        if !self.tracking_processing_turn {
            self.tracking_processing_turn = true;
            self.output_since_turn_started.clear();
        }

        self.output_since_turn_started.push_str(output);
        const MAX_PROMPT_PROBE_CHARS: usize = 32_768;
        let char_count = self.output_since_turn_started.chars().count();
        if char_count > MAX_PROMPT_PROBE_CHARS {
            self.output_since_turn_started = self
                .output_since_turn_started
                .chars()
                .skip(char_count - MAX_PROMPT_PROBE_CHARS)
                .collect();
        }

        if crate::control::antigravity_output_has_ready_prompt(&self.output_since_turn_started) {
            self.reset();
            return true;
        }

        false
    }

    fn reset(&mut self) {
        self.tracking_processing_turn = false;
        self.output_since_turn_started.clear();
    }
}

#[derive(Default)]
struct AntigravityUserTurnReceiptTracker {
    initialized: bool,
    last_step_index: Option<u64>,
}

#[derive(Default)]
struct AntigravityTranscriptTracker {
    initialized: bool,
    observed_text: HashMap<(u64, &'static str), String>,
    latest_step_index: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AntigravityFileWatermark {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AntigravityDatabaseWatermark {
    database: AntigravityFileWatermark,
    wal: Option<AntigravityFileWatermark>,
}

fn antigravity_file_watermark(path: &std::path::Path) -> Option<AntigravityFileWatermark> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(AntigravityFileWatermark {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn antigravity_database_watermark(path: &std::path::Path) -> Option<AntigravityDatabaseWatermark> {
    let database = antigravity_file_watermark(path)?;
    let file_name = path.file_name()?.to_string_lossy();
    let wal = antigravity_file_watermark(&path.with_file_name(format!("{file_name}-wal")));
    Some(AntigravityDatabaseWatermark { database, wal })
}

fn startup_prompt_is_ready(
    provider: &str,
    startup_prompt_pending: bool,
    startup_screen: Option<&str>,
) -> bool {
    provider != "codex"
        && startup_prompt_pending
        && startup_screen.is_some_and(|output| {
            crate::control::provider_output_has_startup_ready_prompt(provider, output)
        })
}

impl AntigravityTranscriptTracker {
    fn minimum_step_index(&self) -> Option<u64> {
        self.latest_step_index
            .map(|index| index.saturating_sub(ANTIGRAVITY_TRANSCRIPT_OVERLAP_STEPS))
    }

    /// Projects provider-authored SQLite messages once. Restored agents first
    /// position at existing history, while fresh agents expose messages already
    /// present when Wardian discovers the provider-owned conversation.
    fn observe(
        &mut self,
        messages: &[AntigravityConversationMessage],
        skip_existing: bool,
    ) -> Vec<WatchTranscriptMessage> {
        let positioning_restored_history = !self.initialized && skip_existing;
        let mut projected = Vec::new();

        for message in messages {
            self.latest_step_index = Some(
                self.latest_step_index
                    .map_or(message.step_index, |current| {
                        current.max(message.step_index)
                    }),
            );
            let role = match message.role {
                AgentChatRole::User => "user",
                AgentChatRole::Assistant => "assistant",
                AgentChatRole::System => "system",
                AgentChatRole::Tool => "tool",
            };
            let key = (message.step_index, role);
            let changed = self.observed_text.get(&key) != Some(&message.text);
            self.observed_text.insert(key, message.text.clone());
            if changed && !positioning_restored_history {
                projected.push(WatchTranscriptMessage {
                    role: role.to_string(),
                    text: message.text.clone(),
                    provider: "antigravity".to_string(),
                    turn_id: None,
                    source: Some("antigravity_sqlite".to_string()),
                    provider_provenance: None,
                });
            }
        }

        if let Some(minimum_step_index) = self.minimum_step_index() {
            self.observed_text
                .retain(|(step_index, _), _| *step_index >= minimum_step_index);
        }
        self.initialized = true;
        projected
    }
}

fn should_auto_confirm_antigravity_workspace_trust(
    provider_name: &str,
    enabled: bool,
    already_confirmed: bool,
    output: &str,
) -> bool {
    let output = output.to_ascii_lowercase();
    provider_name == "antigravity"
        && enabled
        && !already_confirmed
        && output.contains("do you trust the contents of this project?")
        && output.contains("requires permission to read, edit, and execute files here")
}

fn should_auto_confirm_claude_bypass_permissions(
    provider_name: &str,
    enabled: bool,
    already_confirmed: bool,
    output: &str,
) -> bool {
    provider_name == "claude"
        && enabled
        && !already_confirmed
        && claude_output_has_bypass_permissions_consent_prompt(output)
}

impl AntigravityUserTurnReceiptTracker {
    /// Positions restored agents at their existing history while allowing a
    /// fresh conversation to acknowledge a user step already present by the
    /// time the watcher first observes the database.
    fn observe(&mut self, latest_step_index: Option<u64>, skip_existing: bool) -> bool {
        if !self.initialized {
            self.initialized = true;
            if skip_existing {
                self.last_step_index = latest_step_index;
                return false;
            }
        }

        let Some(latest_step_index) = latest_step_index else {
            return false;
        };
        if self
            .last_step_index
            .is_some_and(|last_step_index| latest_step_index < last_step_index)
        {
            self.last_step_index = Some(latest_step_index);
            return false;
        }
        if self
            .last_step_index
            .is_some_and(|last_step_index| latest_step_index == last_step_index)
        {
            return false;
        }

        self.last_step_index = Some(latest_step_index);
        true
    }
}

fn codex_cleared_provider_sessions(config: &AgentConfig) -> Vec<String> {
    config.codex_config().cleared_provider_sessions
}

#[cfg(target_os = "macos")]
use super::macos_extended_path;
#[cfg(windows)]
use super::{app_process_supervisor_active, assign_pid_to_job, create_kill_on_close_job};

pub(super) fn capture_init_timestamp(
    event: &AgentEvent,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
) {
    let AgentEvent::Init { timestamp, .. } = event else {
        return;
    };
    let Some(timestamp) = timestamp else {
        return;
    };
    if let Ok(mut current) = init_timestamp.lock() {
        if current.is_none() {
            *current = Some(timestamp.clone());
        }
    }
}

pub(super) fn handle_provider_init_event(
    provider: &str,
    event: &AgentEvent,
    config: &std::sync::Arc<std::sync::Mutex<AgentConfig>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
) -> Result<ProviderIdentityOutcome, String> {
    let AgentEvent::Init { session_id, .. } = event else {
        return Err(format!(
            "{provider} identity validation requires an initialization event"
        ));
    };

    let outcome = {
        let mut config = config
            .lock()
            .map_err(|_| format!("{provider} session configuration is unavailable"))?;
        if matches!(provider, "codex" | "opencode" | "antigravity")
            && config
                .resume_session
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(format!(
                "{provider} initialization has no pre-bound provider identity"
            ));
        }
        apply_provider_identity(provider, &mut config, session_id)?
    };
    capture_init_timestamp(event, init_timestamp);
    Ok(outcome)
}

fn mock_init_confirms_bootstrap(
    provider: &str,
    outcome: ProviderIdentityOutcome,
    event: &AgentEvent,
    expected_session: Option<&str>,
) -> bool {
    provider == "mock"
        && outcome == ProviderIdentityOutcome::Confirmed
        && matches!(event, AgentEvent::Init { session_id, .. } if expected_session == Some(session_id.trim()))
}

fn handle_provider_init_with_spawn_lease(
    provider: &str,
    event: &AgentEvent,
    config: &std::sync::Arc<std::sync::Mutex<AgentConfig>>,
    init_timestamp: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    bootstrap_complete: &std::sync::atomic::AtomicBool,
) -> Result<ProviderIdentityOutcome, String> {
    let outcome = handle_provider_init_event(provider, event, config, init_timestamp)?;
    // Mock has no later prompt/readiness signal. Its validated, caller-owned
    // Init is the authoritative bootstrap boundary for this exact spawn.
    let confirmed = {
        let config = config
            .lock()
            .map_err(|_| format!("{provider} session configuration is unavailable"))?;
        mock_init_confirms_bootstrap(
            provider,
            outcome,
            event,
            expected_caller_owned_identity(&config),
        )
    };
    if confirmed {
        bootstrap_complete.store(true, std::sync::atomic::Ordering::Release);
    }
    Ok(outcome)
}

fn codex_status_log_session(config: &AgentConfig) -> Option<String> {
    let cleared_provider_sessions = codex_cleared_provider_sessions(config);
    let candidate = config
        .resume_session
        .clone()
        .filter(|value| !value.trim().is_empty())?;

    if codex_provider_session_is_excluded(&candidate, &cleared_provider_sessions) {
        return None;
    }
    Some(candidate)
}

fn claude_status_log_session(config: &AgentConfig) -> String {
    config
        .resume_session
        .as_deref()
        .or(config.fresh_provider_session_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(config.session_id.as_str())
        .to_string()
}

fn provider_spawn_session_identity(config: &AgentConfig) -> &str {
    config
        .resume_session
        .as_deref()
        .filter(|session| !session.trim().is_empty())
        .or_else(|| {
            config
                .fresh_provider_session_id
                .as_deref()
                .filter(|session| !session.trim().is_empty())
        })
        .unwrap_or_default()
        .trim()
}

fn acquire_provider_spawn_lease(
    config: &AgentConfig,
) -> Result<wardian_core::conversation_lease::PersistedConversationLeaseGuard, String> {
    let now = chrono::Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let lease = wardian_core::conversation_lease::ConversationLease {
        agent_id: config.session_id.clone(),
        provider: config.provider.clone(),
        resume_session: provider_spawn_session_identity(config).to_string(),
        owner_kind: "provider_spawn".to_string(),
        owner_id: format!("{}:{}", std::process::id(), uuid::Uuid::new_v4()),
        acquisition_id: uuid::Uuid::new_v4().to_string(),
        owner_node_id: None,
        mode: "lifecycle_transition".to_string(),
        started_at: now_rfc3339.clone(),
        heartbeat_at: now_rfc3339.clone(),
        expires_at: (now + PROVIDER_SPAWN_LEASE_DURATION).to_rfc3339(),
    };

    match wardian_core::conversation_lease::try_acquire_lease(lease.clone(), &now_rfc3339) {
        Ok(wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired) => {
            let guard =
                wardian_core::conversation_lease::PersistedConversationLeaseGuard::new(&lease);
            check_provider_spawn_candidates(config)?;
            Ok(guard)
        }
        Ok(wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Conflict(
            conflict,
        )) => Err(format!(
            "provider startup was withheld because conversation {} is leased by {} {} ({})",
            config.session_id, conflict.owner_kind, conflict.owner_id, conflict.mode
        )),
        Err(error) => Err(format!(
            "provider startup was withheld because conversation ownership could not be verified: {error}"
        )),
    }
}

fn check_provider_spawn_candidates(config: &AgentConfig) -> Result<(), String> {
    let candidates = crate::utils::process::find_wardian_provider_process_candidates(
        &config.session_id,
        &config.provider,
        Some(std::process::id()),
    );
    if let Some(pid) = candidates.first() {
        return Err(format!(
            "provider startup was withheld because a matching provider process candidate already exists (PID {pid})"
        ));
    }
    Ok(())
}

fn validate_inherited_provider_spawn_lease(
    config: &AgentConfig,
    guard: &wardian_core::conversation_lease::PersistedConversationLeaseGuard,
) -> Result<(), String> {
    let now = chrono::Utc::now();
    match wardian_core::conversation_lease::retarget_lifecycle_lease_persisted(
        guard.owner(),
        &config.session_id,
        &config.provider,
        provider_spawn_session_identity(config),
        &now.to_rfc3339(),
        &(now + PROVIDER_SPAWN_LEASE_DURATION).to_rfc3339(),
    )? {
        wardian_core::conversation_lease::ConversationLeaseRetargetOutcome::Retargeted => {}
        wardian_core::conversation_lease::ConversationLeaseRetargetOutcome::Conflict(conflict) => {
            return Err(format!(
                "provider startup was withheld because saved conversation is leased by {} {} ({})",
                conflict.owner_kind, conflict.owner_id, conflict.mode
            ));
        }
        wardian_core::conversation_lease::ConversationLeaseRetargetOutcome::NotActive => {
            return Err("provider startup was withheld because its exact lifecycle lease is no longer active for this agent and provider".to_string());
        }
    }
    check_provider_spawn_candidates(config)
}

pub(crate) fn provider_spawn_lease_for_launch(
    config: &AgentConfig,
    inherited_lease: Option<wardian_core::conversation_lease::PersistedConversationLeaseGuard>,
) -> Result<wardian_core::conversation_lease::PersistedConversationLeaseGuard, String> {
    match inherited_lease {
        Some(lease) => {
            validate_inherited_provider_spawn_lease(config, &lease)?;
            Ok(lease)
        }
        None => acquire_provider_spawn_lease(config),
    }
}

async fn prepare_codex_owner_after_reservation<T, F, Fut>(
    config: &AgentConfig,
    inherited_lease: Option<wardian_core::conversation_lease::PersistedConversationLeaseGuard>,
    prepare: F,
) -> Result<
    (
        wardian_core::conversation_lease::PersistedConversationLeaseGuard,
        T,
    ),
    String,
>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    // Native owner preparation can start a provider writer before the PTY
    // exists. Reserve and scan first, then retain exclusion through any
    // cancellation or uncertain failure during owner preparation.
    let mut lease = provider_spawn_lease_for_launch(config, inherited_lease)?;
    lease.retain_on_drop();
    let owner = prepare().await?;
    Ok((lease, owner))
}

fn provider_spawn_lease_should_release(status: &str) -> bool {
    matches!(status, "Idle" | "Error" | "Off")
}

fn release_provider_spawn_lease_after_readiness<R: tauri::Runtime>(
    mut lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    current_status: std::sync::Arc<std::sync::Mutex<String>>,
    session_id: String,
    bootstrap_complete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app: AppHandle<R>,
    runtime_generation: u64,
    gate: SpawnPublicationGate,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let owner = lease.owner().clone();
        let mut last_renewal = std::time::Instant::now();
        loop {
            if gate
                .unpublished_failure
                .as_ref()
                .is_some_and(|failed| failed.load(std::sync::atomic::Ordering::Acquire))
                || gate
                    .registration
                    .as_ref()
                    .is_some_and(|state| state.state() == RegistrationPublicationState::FAILED)
            {
                log_debug(&format!(
                    "[Wardian] Unpublished provider for {session_id} retains conversation exclusion until lease expiry"
                ));
                lease.retain_until_expiry();
                return;
            }
            let status = current_status
                .lock()
                .map(|status| status.clone())
                .unwrap_or_else(|_| "Error".to_string());
            let bootstrap_observed = bootstrap_complete.load(std::sync::atomic::Ordering::Acquire);
            let ready = provider_spawn_lease_should_release(&status) || bootstrap_observed;
            let committed = gate
                .registration
                .as_ref()
                .is_none_or(|state| state.state() == RegistrationPublicationState::COMMITTED);
            if ready && committed {
                let state = app.state::<AppState>();
                let (published_generation, same_status_arc) = {
                    let agents = state.agents.lock().await;
                    let published = agents.get(&session_id);
                    (
                        published.and_then(|agent| agent.runtime_generation),
                        published.is_some_and(|agent| {
                            std::sync::Arc::ptr_eq(&agent.current_status, &current_status)
                        }),
                    )
                };
                let published = published_generation == Some(runtime_generation) && same_status_arc;
                if published {
                    if let Err(error) = lease.release() {
                        log_debug(&format!(
                            "[Wardian] Provider startup lease release failed for {session_id}: {error}"
                        ));
                    }
                    break;
                }
            }

            if last_renewal.elapsed() >= PROVIDER_SPAWN_LEASE_HEARTBEAT {
                let now = chrono::Utc::now();
                match wardian_core::conversation_lease::renew_lease_owner_persisted(
                    &owner,
                    &now.to_rfc3339(),
                    &(now + PROVIDER_SPAWN_LEASE_DURATION).to_rfc3339(),
                ) {
                    Ok(true) => last_renewal = std::time::Instant::now(),
                    Ok(false) => {
                        log_debug(&format!(
                            "[Wardian] Provider startup lease expired before readiness for {session_id}"
                        ));
                        break;
                    }
                    Err(error) => {
                        log_debug(&format!(
                            "[Wardian] Provider startup lease renewal failed for {session_id}: {error}"
                        ));
                        break;
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        drop(lease);
    })
}

fn pty_status_event_policy_for_provider(provider_name: &str) -> ProviderStatusEventPolicy {
    match provider_name {
        "claude" => ProviderStatusEventPolicy::PreserveActionRequiredUntilTurnCompleted,
        "codex" => ProviderStatusEventPolicy::PreserveActionRequired,
        "mock" => ProviderStatusEventPolicy::RequireTurnCompleted,
        _ => ProviderStatusEventPolicy::Normal,
    }
}

#[cfg(test)]
fn line_event_status_for_pty_provider(
    provider_name: &str,
    current_status: &str,
    event: &AgentEvent,
) -> Option<&'static str> {
    super::provider_status_from_event(
        current_status,
        event,
        pty_status_event_policy_for_provider(provider_name),
    )
}

fn persist_runtime_agent_configs(app: &AppHandle) {
    let state = app.state::<AppState>();
    let snapshot = tauri::async_runtime::block_on(async {
        let agents = state.agents.lock().await;
        let order = state.agent_order.lock().await;
        super::state_configs_snapshot(&agents, &order)
    });
    super::save_state_snapshot(app, &snapshot);
}

pub async fn spawn_agent(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
    unpublished_failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<ActiveAgent, String> {
    Ok(spawn_agent_inner(
        app,
        config,
        is_restored,
        initial_timestamp,
        SpawnPublication::Synchronous,
        None,
        SpawnPublicationGate {
            unpublished_failure: Some(unpublished_failure),
            registration: None,
        },
    )
    .await?
    .active)
}

/// Continue a lifecycle operation's exact persisted exclusion through the
/// replacement provider's bootstrap and publication without a reacquisition gap.
pub async fn spawn_agent_with_lease(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
    lease: wardian_core::conversation_lease::PersistedConversationLeaseGuard,
    unpublished_failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<ActiveAgent, String> {
    Ok(spawn_agent_inner(
        app,
        config,
        is_restored,
        initial_timestamp,
        SpawnPublication::Synchronous,
        Some(lease),
        SpawnPublicationGate {
            unpublished_failure: Some(unpublished_failure),
            registration: None,
        },
    )
    .await?
    .active)
}

pub(crate) async fn spawn_agent_provisionally(
    app: AppHandle,
    config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
    registration: std::sync::Arc<RegistrationPublicationState>,
) -> Result<SpawnedAgent, String> {
    spawn_agent_inner(
        app,
        config,
        is_restored,
        initial_timestamp,
        SpawnPublication::Provisional,
        None,
        SpawnPublicationGate {
            unpublished_failure: None,
            registration: Some(registration),
        },
    )
    .await
}

pub(crate) async fn spawn_agent_for_registration(
    app: AppHandle,
    config: AgentConfig,
    registration: std::sync::Arc<RegistrationPublicationState>,
) -> Result<ActiveAgent, String> {
    Ok(spawn_agent_inner(
        app,
        config,
        false,
        None,
        SpawnPublication::Synchronous,
        None,
        SpawnPublicationGate {
            unpublished_failure: None,
            registration: Some(registration),
        },
    )
    .await?
    .active)
}

async fn spawn_agent_inner(
    app: AppHandle,
    mut config: AgentConfig,
    is_restored: bool,
    initial_timestamp: Option<String>,
    publication: SpawnPublication,
    inherited_lease: Option<wardian_core::conversation_lease::PersistedConversationLeaseGuard>,
    gate: SpawnPublicationGate,
) -> Result<SpawnedAgent, String> {
    let spawn_started_at = std::time::Instant::now();
    super::validate_session_values_for_launch(
        &config.session_id,
        config.resume_session.as_deref(),
    )?;
    let provider = ProviderFactory::resolve(&config.provider)?;
    crate::providers::readiness::ensure_provider_available_for_launch(&config.provider)?;

    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);
    let antigravity_database_baseline = if config.provider == "antigravity"
        && config
            .resume_session
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        AntigravityProvider::antigravity_home()
            .map(|home| AntigravityProvider::conversation_database_ids(&home))
            .unwrap_or_default()
    } else {
        Default::default()
    };

    let expected_folder = if config.folder.is_empty() {
        cwd.to_string_lossy().to_string()
    } else {
        config.folder.clone()
    };

    // Phase 2: Record/Update agent in SQLite with explicit ISO 8601 timestamp
    let born_to_save = initial_timestamp
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let project = wardian_core::db::project_name_from_workspace(&expected_folder);
    if let Err(error) = wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
        session_id: &config.session_id,
        session_name: &config.session_name,
        description: &config.description,
        agent_class: &config.agent_class,
        provider: &config.provider,
        workspace: Some(&expected_folder),
        project: project.as_deref(),
        is_off: config.is_off,
        created_at: Some(&born_to_save),
    }) {
        let detail = error.to_string();
        if detail.to_ascii_lowercase().contains("unique") {
            return Err(format!(
                "An agent with the name '{}' already exists; choose a different name.",
                config.session_name
            ));
        }
        super::log_debug(&format!(
            "[WARDIAN] Failed to persist agent metadata during spawn: {detail}"
        ));
    }

    let app_state = app.state::<AppState>();
    if config.is_off {
        app_state
            .interactions
            .start_provider_input_generation(
                &config.session_id,
                ProviderInputReadiness::Unavailable,
                None,
            )
            .await;
        let _ = wardian_core::db::update_agent_status(&config.session_id, "Off", None);
        let session_id = config.session_id.clone();

        return Ok(SpawnedAgent::without_completion(ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(config)),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: None,
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save))),
            last_query_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Off".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
                session_id, 4096, 262_144,
            ))),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }));
    }

    let provider_generation = app_state
        .interactions
        .start_provider_input_generation(&config.session_id, ProviderInputReadiness::Booting, None)
        .await
        .generation;

    let config_lock = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));

    let live_conversation_started_at =
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    app_state
        .conversation_archive
        .begin_live_conversation(&config.session_id, &live_conversation_started_at)
        .map_err(|error| format!("Failed to establish chat conversation boundary: {error}"))?;

    crate::commands::terminal::log_terminal_runtime_diagnostics_once();

    let pty_system = NativePtySystem::default();

    let initial_geometry = app_state
        .terminal_sessions
        .spawn_geometry(&config.session_id)
        .await
        .map_err(|error| format!("Failed to read terminal spawn geometry: {error}"))?
        .unwrap_or(wardian_core::models::TerminalGeometry { cols: 80, rows: 24 });
    let (initial_cols, initial_rows) = (initial_geometry.cols, initial_geometry.rows);

    let pair = pty_system
        .openpty(PtySize {
            rows: initial_rows,
            cols: initial_cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open pty: {}", e))?;

    let (bin, mut provider_args) = provider.get_executable();
    let claude_hook = if config.provider == "claude" {
        Some(ensure_claude_permission_hook(&config.session_id)?)
    } else {
        None
    };
    let pi_receipt = if config.provider == "pi" {
        Some(super::pi_receipt::Receipt::prepare(&config)?)
    } else {
        None
    };
    let memory_process_key = if is_restored {
        config
            .resume_session
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| config.session_id.clone())
    } else {
        format!("fresh:{}:{born_to_save}", config.session_id)
    };
    let memory_enabled = crate::utils::memory_feature_enabled();
    let memory_brief_at = std::time::Instant::now();
    let memory_setup = if memory_enabled {
        match wardian_core::memory::MemoryStore::from_default_home() {
            Ok(store) => match store.compile_brief(
                &wardian_core::memory::MemoryActor::agent(&config.session_id),
                &config.session_id,
                Some(&expected_folder),
                &config.provider,
                &memory_process_key,
                is_restored,
                12_000,
            ) {
                Ok(brief) => Some((store, brief)),
                Err(error) => {
                    log_debug(&format!(
                        "[Wardian] memory recall unavailable for {}: {error}",
                        config.session_id
                    ));
                    None
                }
            },
            Err(error) => {
                log_debug(&format!(
                    "[Wardian] memory store unavailable for {}: {error}",
                    config.session_id
                ));
                None
            }
        }
    } else {
        None
    };
    let memory_brief_ms = memory_brief_at.elapsed().as_millis();
    // Codex config projection belongs to the exclusive owner, after recovery.
    // The manager only needs neutral habitat/instructions before owner creation.
    let habitat_at = std::time::Instant::now();
    let habitat_root = if config.provider == "codex" {
        Some(crate::utils::fs::prepare_habitat_workspace(
            &cwd,
            &config.agent_class,
            &config.session_id,
        )?)
    } else {
        prepare_provider_habitat(
            &config.provider,
            &cwd,
            &config.agent_class,
            Some(&config.session_id),
        )?
    };
    let habitat_ms = habitat_at.elapsed().as_millis();
    let mut memory_append_ms = 0;
    if let Some(root) = habitat_root.as_ref() {
        if memory_enabled {
            let append_at = std::time::Instant::now();
            crate::utils::fs::append_habitat_memory_instructions(
                root,
                memory_setup.as_ref().and_then(|(_, brief)| {
                    (!brief.is_empty).then_some(brief.context_text.as_str())
                }),
            )?;
            memory_append_ms = append_at.elapsed().as_millis();
        }
        if !crate::utils::fs::provider_uses_projected_workspace(&config.provider) {
            let include = root.to_string_lossy().to_string();
            let includes = config
                .system_include_directories
                .get_or_insert_with(Vec::new);
            if !includes.contains(&include) {
                includes.push(include);
            }
        }
    }
    let pi_resume_session_file = if config.provider == "pi" && is_restored {
        config
            .resume_session
            .as_deref()
            .map(str::trim)
            .filter(|session_id| !session_id.is_empty())
            .and_then(|provider_session_id| {
                PiProvider::session_dir(&config.session_id).and_then(|session_dir| {
                    PiProvider::session_file(&session_dir, provider_session_id)
                })
            })
    } else {
        None
    };
    let pi_has_saved_session = is_restored && pi_resume_session_file.is_some();
    let provider_cwd = if config.provider == "pi" {
        pi_session_project_cwd(
            &config.session_id,
            &cwd,
            habitat_root.as_deref(),
            pi_resume_session_file.as_deref(),
            is_restored,
        )
    } else {
        interactive_provider_cwd(&config.provider, &cwd, habitat_root.as_deref(), None)
    };
    // Keep provider_cwd as the logical workspace for arguments, config and
    // records; only the OS process cwd may use the short habitat alias.
    let launch_cwd = interactive_provider_launch_cwd(
        &config.provider,
        &config.session_id,
        habitat_root.as_deref(),
        &cwd,
        &provider_cwd,
        pi_has_saved_session,
    )?;
    let antigravity_workspace_before = if config.provider == "antigravity"
        && config
            .resume_session
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        let excluded = config.antigravity_config().cleared_conversations;
        AntigravityProvider::antigravity_home().and_then(|home| {
            AntigravityProvider::verified_conversation_for_workspace(
                &home,
                &provider_cwd,
                &excluded,
            )
        })
    } else {
        None
    };
    let fresh_claude_log_paths =
        if config.provider == "claude" && config.fresh_provider_session_id.is_some() {
            dirs::home_dir()
                .map(|home| {
                    claude_log_paths(
                        &home
                            .join(".claude")
                            .join("projects")
                            .join(claude_project_dir_name(&expected_folder)),
                    )
                })
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };

    if config.provider == "claude" {
        if let Some(hook) = claude_hook.as_ref() {
            provider_args.push("--settings".to_string());
            provider_args.push(hook.settings_arg.clone());
        }
    }

    let background_processes = Vec::new();
    let is_resume = config
        .resume_session
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let spawn_args = provider.get_spawn_args(&config, is_resume);
    let spawn_args = finalize_interactive_spawn_args(
        &config.provider,
        is_restored,
        &config.resume_session,
        spawn_args,
    );
    provider_args.extend(spawn_args);
    if config.provider == "codex" && memory_enabled {
        let runtime_instructions = wardian_memory_instructions(
            memory_setup
                .as_ref()
                .and_then(|(_, brief)| (!brief.is_empty).then_some(brief.context_text.as_str())),
        );
        CodexProvider::new()
            .insert_developer_instructions_arg(&mut provider_args, &runtime_instructions);
    }
    if let Some(receipt) = &pi_receipt {
        receipt.append_args(&mut provider_args);
    }
    provider_args = interactive_provider_args(&config.provider, &provider_cwd, &cwd, provider_args);

    let mut opencode_http_plan = if config.provider == "opencode" {
        prepare_opencode_http_launch(&app_state, &config, provider_generation).await
    } else {
        None
    };
    if let Some(plan) = opencode_http_plan.as_ref() {
        provider_args.extend(plan.network_args());
    }
    let opencode_http_config_fingerprint = opencode_http_plan.as_ref().map(|_| {
        let mut binding_config = config.clone();
        binding_config.folder = expected_folder.clone();
        crate::delivery::native_broker::opencode_http_config_fingerprint(&binding_config, &cwd)
    });
    if let (Some(_plan), Some(config_fingerprint)) = (
        opencode_http_plan.as_ref(),
        opencode_http_config_fingerprint.as_ref(),
    ) {
        app_state
            .native_delivery
            .prepare_opencode_http(
                &config.session_id,
                provider_generation,
                config_fingerprint.clone(),
            )
            .await
            .map_err(|error| format!("Failed to reserve OpenCode native ownership: {error}"))?;
    }

    let attachment_at = std::time::Instant::now();
    let (mut spawn_lease, codex_attachment) = if config.provider == "codex" {
        let (lease, attachment) =
            prepare_codex_owner_after_reservation(&config, inherited_lease, || async {
                app_state
                    .native_delivery
                    .prepare_codex_tui(crate::delivery::native_broker::NativeSessionSpec {
                        target_agent_id: config.session_id.clone(),
                        provider: "codex".into(),
                        generation: provider_generation,
                        workspace: provider_cwd.clone(),
                        config: config.clone(),
                    })
                    .await
                    .map_err(|error| error.to_string())
            })
            .await?;
        (lease, Some(attachment))
    } else {
        (
            provider_spawn_lease_for_launch(&config, inherited_lease)?,
            None,
        )
    };
    let mut codex_attach_guard = codex_attachment.as_ref().map(|_| {
        super::codex_shared::CodexAttachGuard::new(
            app_state.native_delivery.clone(),
            config.session_id.clone(),
            provider_generation,
        )
    });
    if let Some(attachment) = codex_attachment.as_ref() {
        // Both clients read the aligned private home; ordinary local discovery
        // requires no CLI key/value config overrides and no --remote mode.
        provider_args = codex_shared_tui_args(
            provider.get_executable().1,
            attachment.model_override.as_deref(),
            attachment.expected_resume_id.as_deref(),
            &provider_cwd,
        );
    }
    let attachment_ms = attachment_at.elapsed().as_millis();
    let mut pi_attachment = if config.provider == "pi" && is_restored {
        if let Some(session_file) = pi_resume_session_file {
            let extension_path =
                match crate::delivery::pi_bridge::PiBridgeLaunchPlan::materialize_extension(
                    &session_file,
                    provider_generation,
                ) {
                    Ok(path) => Some(path),
                    Err(error) => {
                        log_debug(&format!(
                            "[Wardian] Pi bridge unavailable for {}: {}",
                            config.session_id, error
                        ));
                        None
                    }
                };
            if let Some(extension_path) = extension_path {
                match app_state
                    .native_delivery
                    .prepare_pi_tui(
                        crate::delivery::native_broker::NativeSessionSpec {
                            target_agent_id: config.session_id.clone(),
                            provider: "pi".into(),
                            generation: provider_generation,
                            workspace: provider_cwd.clone(),
                            config: config.clone(),
                        },
                        session_file,
                        extension_path,
                    )
                    .await
                {
                    Ok(plan) => Some(plan),
                    Err(error) => {
                        log_debug(&format!(
                            "[Wardian] Pi bridge unavailable for {}: {}",
                            config.session_id, error
                        ));
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    if let Some(attachment) = pi_attachment.as_ref() {
        provider_args.push("-e".into());
        provider_args.push(attachment.extension_path().to_string_lossy().into_owned());
    }
    let launch_spec = interactive_provider_launch(&config.provider, &bin, &provider_args)?;
    log_debug(&format!(
        "[Wardian] PTY spawn: provider={} exe={} arg_count={} cwd={}",
        config.provider,
        launch_spec.executable,
        launch_spec.args.len(),
        provider_cwd.display()
    ));
    // Everything above happens before the provider process exists, so it is the
    // part of perceived spawn latency Wardian itself owns.
    log_debug(&format!(
        "[Wardian] Spawn prelaunch timing provider={} session={} memory_brief_ms={} habitat_ms={} memory_append_ms={} attachment_ms={} prelaunch_total_ms={}",
        config.provider,
        config.session_id,
        memory_brief_ms,
        habitat_ms,
        memory_append_ms,
        attachment_ms,
        spawn_started_at.elapsed().as_millis(),
    ));
    let mut cmd = CommandBuilder::new(&launch_spec.executable);
    for arg in &launch_spec.args {
        cmd.arg(arg);
    }
    cmd.cwd(&launch_cwd);
    apply_terminal_identity_env(&mut cmd);
    super::apply_managed_cli_path_to_pty(&mut cmd);
    super::apply_interactive_provider_runtime_env(&config.provider, &mut cmd)?;
    cmd.env("WARDIAN_SESSION_ID", &config.session_id);
    let memory_capability = memory_enabled
        .then(|| super::issue_memory_capability(&config.session_id))
        .flatten();
    if let Some(capability) = memory_capability.as_ref() {
        cmd.env(
            wardian_core::memory::MEMORY_CAPABILITY_ENV,
            capability.token(),
        );
    }
    for (key, value) in super::worktree_build_env(&config) {
        cmd.env(key, value);
    }

    if let Some(attachment) = &codex_attachment {
        cmd.env("CODEX_HOME", &attachment.codex_home);
    } else if let Some(attachment) = pi_attachment.as_ref() {
        cmd.env(crate::delivery::pi_bridge::ENV_CONFIG, attachment.config());
    } else if config.provider == "opencode" {
        for (key, value) in opencode_interactive_env(&provider_cwd, &config)? {
            cmd.env(key, value);
        }
        if let Some(plan) = opencode_http_plan.as_ref() {
            for (key, value) in plan.environment() {
                cmd.env(key, value);
            }
        }
    } else if config.provider == "mock" {
        let provider_session_id = expected_caller_owned_identity(&config).ok_or_else(|| {
            "mock provider launch has no caller-owned session identity".to_string()
        })?;
        cmd.env("WARDIAN_MOCK_SESSION_ID", provider_session_id);

        let mut has_config_scenario = false;
        let mut has_config_delay = false;
        if let ProviderConfig::Mock(mock) = &config.provider_config {
            if let Some(scenario) = mock.scenario.as_deref().filter(|value| !value.is_empty()) {
                cmd.env("WARDIAN_MOCK_SCENARIO", scenario);
                has_config_scenario = true;
            }
            if let Some(delay_ms) = mock.delay_ms {
                cmd.env("WARDIAN_MOCK_DELAY_MS", delay_ms.to_string());
                has_config_delay = true;
            }
        }
        for key in [
            "WARDIAN_MOCK_SCENARIO",
            "WARDIAN_MOCK_DELAY_MS",
            "WARDIAN_MOCK_SCRIPT",
        ] {
            if (key == "WARDIAN_MOCK_SCENARIO" && has_config_scenario)
                || (key == "WARDIAN_MOCK_DELAY_MS" && has_config_delay)
            {
                continue;
            }
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }

        // Mirrors the event stream to a provider log so the chat transcript can
        // read it back, matching how every real provider is observed.
        if let Some(path) = mock_transcript_log_path(&config.session_id) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(&path);
            cmd.env("WARDIAN_MOCK_LOG", &path);
        }
    }
    #[cfg(target_os = "macos")]
    cmd.env("PATH", macos_extended_path());

    log_debug(&format!(
        "[Wardian] Spawning {} agent. Session: {}, Resume: {}, Restored: {}",
        provider.name(),
        config.session_id,
        config
            .resume_session
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        is_restored
    ));

    // A restored Pi process can append its first turn immediately after spawn.
    // Capture the existing transcript boundary while the provider is still
    // unable to write, then start the watcher from that exact byte offset.
    let pi_log_baseline = restored_pi_log_baseline(&config, is_restored);

    // The transition lease acquired before native owner preparation remains
    // held through PTY creation and publication.
    let child_result = pair.slave.spawn_command(cmd);
    let child = match child_result {
        Ok(child) => child,
        Err(error) => {
            if opencode_http_plan.is_some() {
                app_state
                    .native_delivery
                    .fail_opencode_http(&config.session_id, provider_generation)
                    .await;
            }
            if let Some(attachment) = pi_attachment.as_ref() {
                attachment.owner().close();
            }
            return Err(format!("Failed to spawn command: {}", error));
        }
    };
    // After spawn_command succeeds, later setup errors and async cancellation
    // cannot prove the provider exited. The watcher explicitly releases this
    // exact guard only after published readiness.
    spawn_lease.retain_on_drop();

    let child = if let Some(receipt) = &pi_receipt {
        receipt.own_child(child)
    } else {
        child
    };
    let mut child = super::codex_shared::StartingCodexTui::new(
        child,
        codex_attachment.as_ref().map(|attachment| {
            (
                app_state.native_delivery.clone(),
                config.session_id.clone(),
                attachment.generation,
            )
        }),
    );
    if publication == SpawnPublication::Synchronous {
        if let Some(guard) = codex_attach_guard.as_mut() {
            // StartingCodexTui now owns joined PTY + owner cleanup on every error.
            guard.attached();
        }
    }
    let process_id = child.process_id();
    if pi_attachment.is_some() {
        log_debug(&format!(
            "[Wardian] Pi bridge stage=listener_child_handoff code={}",
            pi_bridge_child_handoff_code(process_id)
        ));
    }
    if let Some(attachment) = pi_attachment.as_mut() {
        if let Some(process_id) = process_id {
            attachment.register_process(process_id);
            attachment.attached();
        } else {
            attachment.owner().close();
            let _ = app_state
                .native_delivery
                .dispose_pi_generation(&config.session_id, Some(provider_generation))
                .await;
        }
    }
    let mut pi_spawn_guard = pi_attachment.as_ref().and_then(|_| {
        process_id.map(|_| {
            PiBridgeSpawnGuard::new(
                app_state.native_delivery.clone(),
                config.session_id.clone(),
                provider_generation,
            )
        })
    });

    // Phase 2: Record/Update status in SQLite with the real PID
    let _ = wardian_core::db::update_agent_status(
        &config.session_id,
        if config.is_off { "Off" } else { "Idle" },
        process_id,
    );

    #[cfg(windows)]
    let job_object = {
        if app_process_supervisor_active() {
            None
        } else if let Ok(job) = create_kill_on_close_job("agent fallback") {
            if let Some(pid) = process_id {
                if let Err(err) = assign_pid_to_job(&job, pid, "agent fallback") {
                    log_debug(&format!(
                        "[Wardian] Failed to assign session {} PID {} to fallback job: {}",
                        config.session_id, pid, err
                    ));
                }
            }
            Some(job)
        } else {
            None
        }
    };
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get pty reader: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get pty writer: {}", e))?;
    let codex_reader_alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let defer_codex_startup_readiness =
        config.provider == "codex" && publication == SpawnPublication::Provisional;
    let pty_master: crate::state::terminal_session::SharedPtyMaster =
        std::sync::Arc::new(std::sync::Mutex::new(pair.master));
    drop(pair.slave);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<
        crate::state::terminal_session::NativeTerminalWriteRequest,
    >(256);
    let terminal_runtime = crate::state::terminal_session::native_terminal_runtime(tx, pty_master);
    let terminal_runtime = match config.provider.as_str() {
        "codex" => terminal_runtime.ignore_scrollback_erase(),
        "pi" => terminal_runtime.reset_parser_on_scrollback_erase(),
        _ => terminal_runtime,
    };
    let runtime_generation = app_state
        .terminal_sessions
        .start_or_replace_runtime(&config.session_id, terminal_runtime, initial_geometry)
        .await
        .map_err(|error| format!("Failed to start terminal session broker: {error}"))?;
    child.runtime(app_state.terminal_sessions.clone(), runtime_generation);

    if config.resume_session.is_some() {
        if let (Some(plan), Some(process_id), Some(provider_session_id), Some(config_fingerprint)) = (
            opencode_http_plan.take(),
            process_id,
            config.resume_session.clone(),
            opencode_http_config_fingerprint,
        ) {
            tauri::async_runtime::spawn(register_opencode_http_after_launch(
                OpenCodeHttpLaunchContext {
                    app: app.clone(),
                    broker: app_state.native_delivery.clone(),
                    agent_id: config.session_id.clone(),
                    plan,
                    provider_session_id,
                    process_id,
                    workspace: cwd.clone(),
                    config_fingerprint,
                    runtime_generation,
                    provider_generation,
                },
            ));
        }
    }
    if let Some(receipt) = &pi_receipt {
        receipt.bind(runtime_generation);
    }
    let sid_for_input = config.session_id.clone();
    let provider_name_for_input = config.provider.clone();

    std::thread::spawn(move || {
        while let Some(input) = rx.blocking_recv() {
            let bytes = input.bytes;
            if provider_name_for_input == "opencode" {
                log_debug(&format!(
                    "[Wardian] OpenCode PTY input for session {}: {}",
                    sid_for_input,
                    debug_preview_bytes(&bytes, 128)
                ));
            }
            log_terminal_trace_bytes(&sid_for_input, &provider_name_for_input, "IN", &bytes);
            let write_result = writer
                .write_all(&bytes)
                .and_then(|_| writer.flush())
                .map_err(|error| error.to_string());
            match write_result {
                Ok(()) => {
                    let _ = input.completion.send(Ok(()));
                }
                Err(error) => {
                    let _ = input.completion.send(Err(error.clone()));
                    log_terminal_trace_note(
                        &sid_for_input,
                        &provider_name_for_input,
                        &format!("PTY input write failed: {error}"),
                    );
                    break;
                }
            }
        }
        log_terminal_trace_note(
            &sid_for_input,
            &provider_name_for_input,
            "input channel closed",
        );
    });

    let sid_out = config.session_id.clone();
    let provider_name_for_pty = config.provider.clone();
    let query_count = std::sync::Arc::new(std::sync::Mutex::new(0));
    let query_count_clone = query_count.clone();
    let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(Some(born_to_save)));
    let init_timestamp_clone = init_timestamp.clone();
    // A process can take several seconds to draw an interactive prompt. Until
    // provider-owned output (or an OpenCode title) proves that prompt exists,
    // accepting mailbox input races the provider's own startup sequence.
    let current_status = std::sync::Arc::new(std::sync::Mutex::new("Starting".to_string()));
    let current_status_clone = current_status.clone();
    let spawn_bootstrap_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let startup_observation = crate::control::startup_readiness::ProviderStartupObservation {
        input_generation: provider_generation,
        runtime_generation,
        current_status: current_status.clone(),
    };
    let watch_state = std::sync::Arc::new(std::sync::Mutex::new(AgentWatchState::new(
        config.session_id.clone(),
        4096,
        262_144,
    )));
    // Keep the attachment gate on the runtime's existing ActiveAgent-owned
    // watch-state Arc so every status/readiness/admission path sees it.
    let codex_attachment_ready = {
        let watch_state = watch_state
            .lock()
            .map_err(|_| "Agent watch state lock unavailable".to_string())?;
        watch_state.set_codex_attachment_ready(!defer_codex_startup_readiness);
        watch_state.codex_attachment_ready_flag()
    };
    let reader_alive = codex_reader_alive.clone();
    let reader_attachment_ready = codex_attachment_ready.clone();
    let watch_state_clone = watch_state.clone();
    let terminal_title = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let terminal_title_clone = terminal_title.clone();
    let last_output_at = std::sync::Arc::new(std::sync::Mutex::new(None));
    let last_output_at_clone = last_output_at.clone();
    let initial_log_path = pi_log_baseline
        .as_ref()
        .map(|baseline| baseline.path.clone());
    let log_path = std::sync::Arc::new(std::sync::Mutex::new(initial_log_path));
    // The mock provider writes its event stream to a file it owns, so its log
    // path is known up front and needs no discovery watcher. Without this the
    // chat transcript sees nothing for a mock agent: normalized tool events
    // are only ever read back from a provider log.
    if config.provider == "mock" {
        if let Some(path) = mock_transcript_log_path(&config.session_id) {
            if let Ok(mut lock) = log_path.lock() {
                *lock = Some(path);
            }
        }
    }
    // PTY reader thread: uses provider.parse_output() for event classification
    let pty_app = app.clone();
    let pty_provider = provider.clone();
    let sid_for_pty = sid_out.clone();
    let pty_emit_app = app.clone();
    let terminal_theme_for_pty = app_state.terminal_theme();
    let terminal_sessions = app_state.terminal_sessions.clone();
    let reader_runtime_generation = runtime_generation;
    let pty_spawn_bootstrap_complete = spawn_bootstrap_complete.clone();
    let codex_reader_owner = codex_attachment.as_ref().map(|_| {
        (
            app_state.native_delivery.clone(),
            config.session_id.clone(),
            provider_generation,
        )
    });
    let pty_config = config_lock.clone();
    let auto_confirm_antigravity_workspace_trust = config.provider == "antigravity"
        && config
            .antigravity_config()
            .dangerously_skip_permissions
            .unwrap_or(true);
    let auto_confirm_claude_bypass_permissions = config.provider == "claude"
        && effective_claude_permission_mode(config.claude_config().permission_mode.as_deref())
            == "bypassPermissions";
    let mut pending_memory_injection = memory_setup
        .map(|(store, brief)| (store, brief, expected_folder.clone(), memory_process_key));
    let pi_exit_broker = pi_attachment
        .as_ref()
        .map(|_| app_state.native_delivery.clone());
    let pi_exit_generation = provider_generation;
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        let mut current_line = String::new();
        let mut had_pty_output = false;
        let mut opencode_chunks_logged = 0usize;
        let mut codex_terminal_theme_responder = CodexTerminalThemeProbeResponder::default();
        let mut antigravity_turn_completion_gate = AntigravityTurnCompletionGate::default();
        let mut startup_prompt_pending = true;
        let mut codex_choice_pending = false;
        let mut antigravity_workspace_trust_confirmed = false;
        let mut claude_bypass_permissions_confirmed = false;
        let mut pty_decoder = PtyUtf8Decoder::new();
        let output_ready_emit_gate =
            std::sync::Arc::new(std::sync::Mutex::new(OutputReadyEmitGate::default()));
        let _codex_exit_guard = codex_reader_owner.map(|(broker, agent_id, generation)| {
            super::codex_shared::CodexAttachGuard::new(broker, agent_id, generation)
                .for_reader(reader_alive, reader_attachment_ready.clone())
        });
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log_terminal_trace_note(&sid_for_pty, &provider_name_for_pty, "pty EOF");
                    if provider_name_for_pty == "opencode" {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY EOF for session {} (had_output={})",
                            sid_for_pty, had_pty_output
                        ));
                    }
                    // If the process exited immediately with no output, surface a
                    // diagnostic message so the terminal is not silently blank.
                    if !had_pty_output && provider_name_for_pty == "opencode" {
                        let msg = concat!(
                            "\r\n[Wardian] OpenCode exited without producing any output.\r\n",
                            "Possible causes:\r\n",
                            "  - generated OpenCode runtime config is invalid (check ~/.wardian/wardian_debug.log)\r\n",
                            "  - OpenCode binary not found or failed to start\r\n",
                            "  - Authentication/config error in OpenCode\r\n",
                            "Check ~/.wardian/wardian_debug.log for the exact command and config used.\r\n",
                        );
                        let _ = crate::state::terminal_session::forward_terminal_output(
                            &terminal_sessions,
                            &sid_for_pty,
                            reader_runtime_generation,
                            msg.as_bytes(),
                        );
                        let _ = pty_emit_app.emit(
                            "agent-pty-output-ready",
                            serde_json::json!({ "session_id": sid_for_pty }),
                        );
                    }
                    break;
                }
                Ok(n) => {
                    crate::utils::runtime_profile::record_event(
                        crate::utils::runtime_profile::RuntimeMetric::PtyRead,
                        n as u64,
                    );
                    if let Err(error) = crate::state::terminal_session::forward_terminal_output(
                        &terminal_sessions,
                        &sid_for_pty,
                        reader_runtime_generation,
                        &buf[..n],
                    ) {
                        log_terminal_trace_note(
                            &sid_for_pty,
                            &provider_name_for_pty,
                            &format!("broker rejected PTY reader output: {error}"),
                        );
                        break;
                    }
                    let pty_postprocess_profile =
                        crate::utils::runtime_profile::RuntimeProfileSpan::wall(
                            crate::utils::runtime_profile::RuntimeMetric::PtyPostprocess,
                        );
                    if provider_name_for_pty == "opencode" && opencode_chunks_logged < 40 {
                        log_debug(&format!(
                            "[Wardian] OpenCode PTY chunk {} for session {}: {}",
                            opencode_chunks_logged + 1,
                            sid_for_pty,
                            debug_preview_bytes(&buf[0..n], 256)
                        ));
                        opencode_chunks_logged += 1;
                    }
                    had_pty_output = true;
                    codex_terminal_theme_responder.respond_to_output(
                        &terminal_sessions,
                        &sid_for_pty,
                        reader_runtime_generation,
                        &provider_name_for_pty,
                        &buf[0..n],
                        &terminal_theme_for_pty,
                    );
                    if let Ok(mut watch_state) = watch_state_clone.lock() {
                        watch_state.push_output(&buf[0..n]);
                    }
                    log_terminal_trace_bytes(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        "OUT",
                        &buf[0..n],
                    );
                    let text = pty_decoder.decode_chunk(&buf[0..n]);
                    let startup_output = if startup_prompt_pending {
                        watch_state_clone.lock().ok().and_then(|watch_state| {
                            watch_state
                                .snapshot_since(None, None)
                                .ok()
                                .map(|snapshot| snapshot.output.text)
                        })
                    } else {
                        None
                    };
                    let startup_screen = if provider_name_for_pty == "codex"
                        || (startup_prompt_pending
                            && matches!(
                                provider_name_for_pty.as_str(),
                                "claude" | "opencode" | "pi"
                            )) {
                        // Output was applied to the broker above. Read its current
                        // screen so chunk boundaries and erased startup messages
                        // cannot promote readiness or keep it blocked forever.
                        tauri::async_runtime::block_on(terminal_sessions.snapshot(&sid_for_pty))
                            .ok()
                            .filter(|snapshot| {
                                snapshot.runtime_generation == reader_runtime_generation
                            })
                            .map(|snapshot| snapshot.visible_grid)
                    } else {
                        startup_output.clone()
                    };
                    let startup_ready = startup_prompt_is_ready(
                        &provider_name_for_pty,
                        startup_prompt_pending,
                        startup_screen.as_deref(),
                    );
                    if startup_ready {
                        startup_prompt_pending = false;
                        record_pending_memory_injection(
                            &mut pending_memory_injection,
                            &sid_for_pty,
                            &provider_name_for_pty,
                        );
                        if !defer_codex_startup_readiness {
                            set_agent_status(&pty_app, &sid_for_pty, &current_status_clone, "Idle");
                            let readiness_app = pty_app.clone();
                            let readiness_session_id = sid_for_pty.clone();
                            let observation = startup_observation.clone();
                            tauri::async_runtime::spawn(async move {
                                let state = readiness_app.state::<AppState>();
                                crate::control::startup_readiness::publish_startup_readiness(
                                    Some(&readiness_app),
                                    state.inner(),
                                    &readiness_session_id,
                                    &observation,
                                    wardian_core::control::ProviderReadyEvidence::PromptDetected,
                                )
                                .await;
                                crate::control::spawn_agent_messaging_if_idle(
                                    &readiness_app,
                                    &readiness_session_id,
                                    "Idle",
                                );
                            });
                        }
                    } else if startup_output.as_deref().is_some_and(|output| {
                        should_auto_confirm_claude_bypass_permissions(
                            &provider_name_for_pty,
                            auto_confirm_claude_bypass_permissions,
                            claude_bypass_permissions_confirmed,
                            output,
                        )
                    }) {
                        match terminal_sessions.send_privileged_input_blocking(
                            &sid_for_pty,
                            reader_runtime_generation,
                            b"\x1b[B\r".to_vec(),
                        ) {
                            Ok(()) => claude_bypass_permissions_confirmed = true,
                            Err(error) => {
                                log_debug(&format!(
                                    "[WARDIAN] Failed to confirm Claude bypass-permissions consent for {}: {}",
                                    sid_for_pty, error
                                ));
                                set_agent_status(
                                    &pty_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    "Action Needed",
                                );
                            }
                        }
                    } else if startup_output.as_deref().is_some_and(|output| {
                        should_auto_confirm_antigravity_workspace_trust(
                            &provider_name_for_pty,
                            auto_confirm_antigravity_workspace_trust,
                            antigravity_workspace_trust_confirmed,
                            output,
                        )
                    }) {
                        match terminal_sessions.send_privileged_input_blocking(
                            &sid_for_pty,
                            reader_runtime_generation,
                            vec![b'\r'],
                        ) {
                            Ok(()) => antigravity_workspace_trust_confirmed = true,
                            Err(error) => {
                                log_debug(&format!(
                                    "[WARDIAN] Failed to confirm Antigravity workspace trust for {}: {}",
                                    sid_for_pty, error
                                ));
                                set_agent_status(
                                    &pty_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    "Action Needed",
                                );
                            }
                        }
                    } else if startup_screen.as_deref().is_some_and(|output| {
                        crate::control::provider_output_requires_startup_action(
                            &provider_name_for_pty,
                            output,
                        )
                    }) {
                        set_agent_status(
                            &pty_app,
                            &sid_for_pty,
                            &current_status_clone,
                            "Action Needed",
                        );
                    }
                    if provider_name_for_pty == "codex" {
                        if let Some(screen) = startup_screen.as_deref() {
                            if crate::delivery::codex_menu::current_screen_requires_choice(screen) {
                                codex_choice_pending = true;
                            } else if codex_choice_pending
                                && reader_attachment_ready
                                    .load(std::sync::atomic::Ordering::Acquire)
                                && crate::control::provider_output_has_ready_prompt("codex", screen)
                            {
                                codex_choice_pending = false;
                                let choice_app = pty_app.clone();
                                let choice_session = sid_for_pty.clone();
                                let observation = startup_observation.clone();
                                tauri::async_runtime::spawn(async move {
                                    let state = choice_app.state::<AppState>();
                                    crate::control::codex_menu_status::restore_after_choice(
                                        &choice_app,
                                        state.inner(),
                                        &choice_session,
                                        &observation,
                                    )
                                    .await;
                                });
                            }
                        }
                    }
                    if let Ok(mut stamp) = last_output_at_clone.lock() {
                        *stamp = Some(std::time::SystemTime::now());
                    }

                    let status_before_output = current_status_clone
                        .lock()
                        .map(|status| status.clone())
                        .unwrap_or_default();
                    if antigravity_turn_completion_gate.observe_output(
                        &provider_name_for_pty,
                        &status_before_output,
                        &text,
                    ) {
                        apply_agent_event(
                            &pty_app,
                            &sid_for_pty,
                            AgentEvent::TurnCompleted,
                            &query_count_clone,
                            &init_timestamp_clone,
                            &current_status_clone,
                        );
                    }

                    // Process stream events to capture Session ID / Status changes
                    // Use a simple line-based approach for stream-json events
                    for line in text.lines() {
                        if let Some(event) = pty_provider.parse_output(line) {
                            if matches!(&event, AgentEvent::Init { .. }) {
                                if let Err(error) = handle_provider_init_with_spawn_lease(
                                    &provider_name_for_pty,
                                    &event,
                                    &pty_config,
                                    &init_timestamp_clone,
                                    &pty_spawn_bootstrap_complete,
                                ) {
                                    log_debug(&format!(
                                        "[WARDIAN] Rejected {} initialization identity: {}",
                                        provider_name_for_pty, error
                                    ));
                                    set_agent_status(
                                        &pty_app,
                                        &sid_for_pty,
                                        &current_status_clone,
                                        "Error",
                                    );
                                    return;
                                }
                            }
                            apply_agent_status_event_with_policy(
                                &pty_app,
                                &sid_for_pty,
                                event,
                                &current_status_clone,
                                pty_status_event_policy_for_provider(&provider_name_for_pty),
                            );
                        }
                    }

                    if let Some(title) = extract_terminal_titles(&text).into_iter().last() {
                        if provider_name_for_pty == "opencode" {
                            log_debug(&format!(
                                "[Wardian] OpenCode backend title for session {}: {}",
                                sid_for_pty, title
                            ));
                        }
                        if let Ok(mut current_title) = terminal_title_clone.lock() {
                            *current_title = title.clone();
                        }
                        if provider_name_for_pty == "opencode" {
                            // Titles describe turns only after the canonical
                            // composer has ended startup. A generic title can
                            // arrive while the resumed session is still loading.
                            if let Some(next_status) = (!startup_prompt_pending)
                                .then(|| opencode_status_from_title(&title))
                                .flatten()
                            {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        } else if provider_name_for_pty == "gemini" {
                            if let Some(next_status) = gemini_status_from_title(&title) {
                                set_agent_status(
                                    &pty_emit_app,
                                    &sid_for_pty,
                                    &current_status_clone,
                                    next_status,
                                );
                            }
                        }
                    }
                    let output_ready_action = output_ready_emit_gate
                        .lock()
                        .map(|mut gate| gate.after_buffer_append(std::time::Instant::now()))
                        .unwrap_or(OutputReadyEmitAction::Suppress);
                    match output_ready_action {
                        OutputReadyEmitAction::EmitNow => {
                            let _ = pty_emit_app.emit(
                                "agent-pty-output-ready",
                                serde_json::json!({ "session_id": sid_for_pty }),
                            );
                        }
                        OutputReadyEmitAction::ScheduleAfter(delay) => {
                            let delayed_app = pty_emit_app.clone();
                            let delayed_session_id = sid_for_pty.clone();
                            let delayed_gate = output_ready_emit_gate.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(delay).await;
                                let should_emit = delayed_gate
                                    .lock()
                                    .map(|mut gate| {
                                        gate.finish_delayed_emit(true, std::time::Instant::now())
                                    })
                                    .unwrap_or(false);
                                if should_emit {
                                    let _ = delayed_app.emit(
                                        "agent-pty-output-ready",
                                        serde_json::json!({ "session_id": delayed_session_id }),
                                    );
                                }
                            });
                        }
                        OutputReadyEmitAction::Suppress => {}
                    }
                    current_line.push_str(&text);
                    loop {
                        if let Some(start) = current_line.find('{') {
                            let slice = &current_line[start..];
                            let mut stream = serde_json::Deserializer::from_str(slice)
                                .into_iter::<serde_json::Value>();
                            match stream.next() {
                                Some(Ok(parsed)) => {
                                    // Use provider to classify the raw JSON into an AgentEvent
                                    let raw_line = parsed.to_string();
                                    if let Some(message) = extract_transcript_message(
                                        &provider_name_for_pty,
                                        &raw_line,
                                    ) {
                                        if let Ok(mut watch_state) = watch_state_clone.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = pty_provider.parse_output(&raw_line) {
                                        if matches!(&event, AgentEvent::Init { .. }) {
                                            if let Err(error) =
                                                handle_provider_init_with_spawn_lease(
                                                    &provider_name_for_pty,
                                                    &event,
                                                    &pty_config,
                                                    &init_timestamp_clone,
                                                    &pty_spawn_bootstrap_complete,
                                                )
                                            {
                                                log_debug(&format!(
                                                    "[WARDIAN] Rejected {} initialization identity: {}",
                                                    provider_name_for_pty, error
                                                ));
                                                set_agent_status(
                                                    &pty_app,
                                                    &sid_for_pty,
                                                    &current_status_clone,
                                                    "Error",
                                                );
                                                return;
                                            }
                                        }

                                        apply_agent_event_with_policy(
                                            &pty_app,
                                            &sid_for_pty,
                                            event,
                                            &query_count_clone,
                                            &init_timestamp_clone,
                                            &current_status_clone,
                                            pty_status_event_policy_for_provider(
                                                &provider_name_for_pty,
                                            ),
                                        );
                                    }
                                    let _ = pty_emit_app.emit("agent-json-event", serde_json::json!({ "session_id": sid_out, "data": parsed }));
                                    let consumed = stream.byte_offset();
                                    current_line = current_line[start + consumed..].to_string();
                                    continue;
                                }
                                _ => break,
                            }
                        }
                        break;
                    }
                    if current_line.len() > 10000 {
                        current_line.clear();
                    }
                    pty_postprocess_profile.finish(n as u64);
                }
                Err(err) => {
                    log_terminal_trace_note(
                        &sid_for_pty,
                        &provider_name_for_pty,
                        &format!("pty read error: {}", err),
                    );
                    break;
                }
            }
        }
        // Process terminated (EOF or error) — mark status as Off
        set_agent_status(&pty_app, &sid_for_pty, &current_status_clone, "Off");
        if provider_name_for_pty == "pi" {
            if let Some(broker) = pi_exit_broker {
                let agent_id = sid_for_pty.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = broker
                        .dispose_pi_generation(&agent_id, Some(pi_exit_generation))
                        .await;
                });
            }
        }
    });
    if let Some(guard) = pi_spawn_guard.as_mut() {
        guard.disarm();
    }

    let mut codex_attachment_completion = None;
    if let Some(attachment) = &codex_attachment {
        if publication == SpawnPublication::Synchronous {
            config = finalize_synchronous_codex(SynchronousCodexFinalizationContext {
                native_delivery: &app_state.native_delivery,
                session_id: &config.session_id,
                provider_generation: attachment.generation,
                child: &mut child,
                config_lock: &config_lock,
                codex_attachment_ready: &codex_attachment_ready,
                codex_reader_alive: &codex_reader_alive,
                watch_state: &watch_state,
            })
            .await?;
            app_state
                .interactions
                .record_provider_input_state(
                    &config.session_id,
                    provider_generation,
                    ProviderInputReadiness::Ready,
                    None,
                )
                .await;
            set_agent_status(&app, &config.session_id, &current_status, "Idle");
        }
    }
    if codex_attachment.is_some() && publication == SpawnPublication::Synchronous {
        let observations = app_state
            .native_delivery
            .codex_observations(&config.session_id, provider_generation)
            .await
            .map_err(|error| error.to_string())?;
        super::codex_shared::observe_turn_activity(
            app.clone(),
            config.session_id.clone(),
            current_status.clone(),
            observations,
        );
    }
    if config.provider == "codex" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_config = config_lock.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let wardian_agent_dir = get_wardian_home()
            .map(|home| home.join("agents").join(&watcher_session))
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().to_string());

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut last_lookup_session = String::new();
            let mut last_log_path: Option<std::path::PathBuf> = None;
            let mut codex_watch_binding = CodexWatchBindingState::default();
            let mut positioned_initial_log = !watcher_skip_existing_log;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }
                let watcher_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                    crate::utils::runtime_profile::RuntimeMetric::CodexWatcherPoll,
                );

                let path = {
                    let lookup_session = watcher_config
                        .lock()
                        .ok()
                        .and_then(|cfg| codex_status_log_session(&cfg));
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(lookup_session) = lookup_session {
                        if last_lookup_session != lookup_session {
                            *lock = None;
                            offset = 0;
                            last_log_path = None;
                            codex_watch_binding.reset();
                            positioned_initial_log = !watcher_skip_existing_log;
                            last_lookup_session = lookup_session.clone();
                        }
                        if lock.is_none() {
                            *lock = codex_session_file_path(
                                &lookup_session,
                                wardian_agent_dir.as_deref(),
                            );
                        }
                        lock.clone()
                    } else {
                        *lock = None;
                        offset = 0;
                        last_lookup_session.clear();
                        last_log_path = None;
                        codex_watch_binding.reset();
                        None
                    }
                };

                if let Some(path) = path {
                    if last_log_path.as_ref() != Some(&path) {
                        codex_watch_binding.reset();
                        last_log_path = Some(path.clone());
                    }
                    codex_watch_binding
                        .set_source(&last_lookup_session, path.to_string_lossy().as_ref());
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                codex_watch_binding.reset();
                                codex_watch_binding.set_source(
                                    &last_lookup_session,
                                    path.to_string_lossy().as_ref(),
                                );
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                crate::utils::runtime_profile::record_event(
                                    crate::utils::runtime_profile::RuntimeMetric::ProviderLogRead,
                                    read as u64,
                                );
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    let raw_line = parsed.to_string();
                                    let event = watcher_provider.parse_output(&raw_line);
                                    codex_watch_binding.observe_record(&raw_line, event.as_ref());
                                    if let Some(message) =
                                        codex_watch_binding.extract_message(&raw_line)
                                    {
                                        if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = event {
                                        apply_agent_event_with_policy(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                            pty_status_event_policy_for_provider("codex"),
                                        );
                                    }
                                    let _ = watcher_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({ "session_id": watcher_session, "data": parsed }),
                                    );
                                } else {
                                    codex_watch_binding.reset();
                                }
                            }
                        }
                    }
                } else if last_log_path.take().is_some() {
                    codex_watch_binding.reset();
                }

                watcher_profile.finish(0);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    } else if config.provider == "pi" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_config = config_lock.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_initial_cursor = pi_log_baseline
            .as_ref()
            .map(|baseline| baseline.cursor.clone())
            .unwrap_or_default();

        let receipt = pi_receipt.clone().expect("Pi receipt prepared");
        let watcher_receipt = receipt.clone();
        let receipt_broker = app_state.terminal_sessions.clone();
        let receipt_executor = tokio::runtime::Handle::current();
        let watcher = std::thread::spawn(move || {
            let mut cursor = watcher_initial_cursor;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|status| status.clone())
                    .unwrap_or_else(|error| error.into_inner().clone());
                if current == "Off" || watcher_receipt.stopped() {
                    break;
                }
                let watcher_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                    crate::utils::runtime_profile::RuntimeMetric::PiWatcherPoll,
                );

                watcher_receipt.poll(
                    &receipt_broker,
                    &receipt_executor,
                    &watcher_watch_state,
                    &watcher_query_count,
                    &watcher_app,
                    &watcher_current_status,
                );

                let provider_session_id = watcher_config
                    .lock()
                    .ok()
                    .and_then(|config| expected_caller_owned_identity(&config).map(str::to_string));
                let path = provider_session_id
                    .as_deref()
                    .and_then(|provider_session_id| {
                        let cached = watcher_log_path
                            .lock()
                            .ok()
                            .and_then(|path| path.clone())
                            .filter(|path| path.is_file());
                        cached.or_else(|| {
                            PiProvider::session_dir(&watcher_session).and_then(|session_dir| {
                                PiProvider::session_file(&session_dir, provider_session_id)
                            })
                        })
                    });

                if let Some(path) = path {
                    if let Ok(mut stored_path) = watcher_log_path.lock() {
                        *stored_path = Some(path.clone());
                    }
                    if let Some(file) = open_pi_log_at_cursor(&path, &mut cursor) {
                        let mut reader = std::io::BufReader::new(file);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            let read = reader.read_line(&mut line).unwrap_or(0);
                            if read == 0 {
                                break;
                            }
                            crate::utils::runtime_profile::record_event(
                                crate::utils::runtime_profile::RuntimeMetric::ProviderLogRead,
                                read as u64,
                            );
                            cursor.offset += read as u64;
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
                            else {
                                continue;
                            };
                            let raw_line = parsed.to_string();
                            if let Some(mut message) = extract_transcript_message("pi", &raw_line) {
                                if let Some(provider_session_id) = provider_session_id.as_deref() {
                                    bind_pi_watch_message(
                                        &mut message,
                                        provider_session_id,
                                        path.to_string_lossy().as_ref(),
                                    );
                                }
                                if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                    watch_state.push_transcript(message);
                                }
                            }
                            if let Some(event) = watcher_provider.parse_output(&raw_line) {
                                if matches!(&event, AgentEvent::Init { .. }) {
                                    match handle_provider_init_event(
                                        "pi",
                                        &event,
                                        &watcher_config,
                                        &watcher_init_timestamp,
                                    ) {
                                        Ok(_) => {
                                            if let Ok(mut config) = watcher_config.lock() {
                                                retain_pi_fresh_provider_session(&mut config);
                                            }
                                            persist_runtime_agent_configs(&watcher_app);
                                        }
                                        Err(error) => {
                                            log_debug(&format!(
                                                "[WARDIAN] Rejected Pi initialization identity: {error}"
                                            ));
                                            set_agent_status(
                                                &watcher_app,
                                                &watcher_session,
                                                &watcher_current_status,
                                                "Error",
                                            );
                                            break;
                                        }
                                    }
                                }
                                // Hook owns starts/counts. Keep both transcript projection and
                                // raw JSON emission when the buffered user record arrives later.
                                if let Some(event) = super::pi_receipt::log_activity(event) {
                                    apply_agent_event(
                                        &watcher_app,
                                        &watcher_session,
                                        event,
                                        &watcher_query_count,
                                        &watcher_init_timestamp,
                                        &watcher_current_status,
                                    );
                                }
                            }
                            let _ = watcher_app.emit(
                                "agent-json-event",
                                serde_json::json!({
                                    "session_id": watcher_session,
                                    "data": parsed,
                                }),
                            );
                        }
                        let mut file = reader.into_inner();
                        let _ = refresh_pi_log_boundary(&mut file, &mut cursor);
                    }
                }
                watcher_profile.finish(0);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            watcher_receipt.stop();
        });
        receipt.retain_watcher(watcher);
    } else if config.provider == "claude" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_log_session = claude_status_log_session(&config);
        let watcher_can_capture_fresh_identity = config.fresh_provider_session_id.is_some();
        let watcher_session_name = config.session_name.clone();
        let watcher_config = config_lock.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_folder = expected_folder.clone();
        let watcher_fresh_claude_log_paths = fresh_claude_log_paths;
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let hook_event_log = claude_hook.as_ref().map(|hook| hook.event_log_path.clone());
        let waiting_for_permission = std::sync::Arc::new(std::sync::Mutex::new(false));
        let log_waiting_for_permission = waiting_for_permission.clone();

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut positioned_initial_log = !watcher_skip_existing_log;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }
                let watcher_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                    crate::utils::runtime_profile::RuntimeMetric::ClaudeWatcherPoll,
                );

                let (path, captured_identity) = {
                    let mut lock = watcher_log_path.lock().unwrap_or_else(|e| e.into_inner());
                    let mut captured_identity = false;
                    if lock.is_none() {
                        if let Some(home) = dirs::home_dir() {
                            let project_dir = home
                                .join(".claude")
                                .join("projects")
                                .join(claude_project_dir_name(&watcher_folder));
                            let candidate =
                                project_dir.join(format!("{}.jsonl", watcher_log_session));
                            if candidate.exists()
                                && !watcher_fresh_claude_log_paths.contains(&candidate)
                            {
                                *lock = Some(candidate);
                            } else if watcher_can_capture_fresh_identity {
                                if let Some((path, provider_session_id)) =
                                    discover_claude_log_for_session_name(
                                        &project_dir,
                                        &watcher_session_name,
                                        &watcher_fresh_claude_log_paths,
                                    )
                                {
                                    if let Ok(mut cfg) = watcher_config.lock() {
                                        cfg.resume_session = Some(provider_session_id);
                                        cfg.fresh_provider_session_id = None;
                                        captured_identity = true;
                                    }
                                    *lock = Some(path);
                                }
                            }
                        }
                    }
                    (lock.clone(), captured_identity)
                };
                if captured_identity {
                    persist_runtime_agent_configs(&watcher_app);
                }

                if let Some(path) = path {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                positioned_initial_log = true;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                crate::utils::runtime_profile::record_event(
                                    crate::utils::runtime_profile::RuntimeMetric::ProviderLogRead,
                                    read as u64,
                                );
                                offset += read as u64;
                                if let Some(message) =
                                    extract_transcript_message("claude", line.trim())
                                {
                                    if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                        watch_state.push_transcript(message);
                                    }
                                }
                                if let Some(event) = watcher_provider.parse_output(line.trim()) {
                                    let mut waiting = log_waiting_for_permission
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    if *waiting {
                                        match event {
                                            AgentEvent::UserQuery | AgentEvent::Generating => {
                                                if let Ok(parsed) =
                                                    serde_json::from_str::<serde_json::Value>(
                                                        line.trim(),
                                                    )
                                                {
                                                    let is_tool_result =
                                                        parsed.get("type").and_then(|v| v.as_str())
                                                            == Some("user")
                                                            && classify_claude_user_event(&parsed)
                                                                == ClaudeUserEventKind::ToolResult;
                                                    if is_tool_result {
                                                        *waiting = false;
                                                        apply_agent_status_event_with_policy(
                                                            &watcher_app,
                                                            &watcher_session,
                                                            event,
                                                            &watcher_current_status,
                                                            pty_status_event_policy_for_provider(
                                                                "claude",
                                                            ),
                                                        );
                                                    }
                                                }
                                            }
                                            AgentEvent::ModelResponse => {
                                                *waiting = false;
                                                apply_agent_status_event_with_policy(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                    pty_status_event_policy_for_provider("claude"),
                                                );
                                            }
                                            AgentEvent::ActionRequired { .. } => {
                                                apply_agent_status_event_with_policy(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                    pty_status_event_policy_for_provider("claude"),
                                                );
                                            }
                                            AgentEvent::TurnCompleted
                                            | AgentEvent::TurnInterrupted => {
                                                *waiting = false;
                                                apply_agent_status_event_with_policy(
                                                    &watcher_app,
                                                    &watcher_session,
                                                    event,
                                                    &watcher_current_status,
                                                    pty_status_event_policy_for_provider("claude"),
                                                );
                                            }
                                            AgentEvent::Init { .. }
                                            | AgentEvent::TurnStarted { .. }
                                            | AgentEvent::Unknown => {}
                                        }
                                    } else {
                                        apply_agent_event_with_policy(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                            pty_status_event_policy_for_provider("claude"),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                watcher_profile.finish(0);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });

        if let Some(hook_event_log) = hook_event_log {
            let hook_app = app.clone();
            let hook_session = config.session_id.clone();
            let hook_accepted_sessions = {
                let mut sessions = vec![config.session_id.clone()];
                if let Some(resume_session) = config
                    .resume_session
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(resume_session.to_string());
                }
                if let Some(fresh_provider_session_id) = config
                    .fresh_provider_session_id
                    .as_ref()
                    .map(|sid| sid.trim())
                    .filter(|sid| !sid.is_empty() && *sid != config.session_id)
                {
                    sessions.push(fresh_provider_session_id.to_string());
                }
                sessions
            };
            let hook_current_status = current_status.clone();
            let hook_waiting_for_permission = waiting_for_permission.clone();

            std::thread::spawn(move || {
                let mut offset = 0;
                loop {
                    let current = hook_current_status
                        .lock()
                        .map(|s| s.clone())
                        .unwrap_or_else(|e| e.into_inner().clone());
                    if current == "Off" {
                        break;
                    }
                    let watcher_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                        crate::utils::runtime_profile::RuntimeMetric::ClaudeHookPoll,
                    );

                    if let Ok(mut file) = std::fs::File::open(&hook_event_log) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                crate::utils::runtime_profile::record_event(
                                    crate::utils::runtime_profile::RuntimeMetric::ProviderLogRead,
                                    read as u64,
                                );
                                offset += read as u64;
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    if !hook_accepted_sessions.iter().any(|session_id| {
                                        claude_permission_hook_matches_session(&parsed, session_id)
                                    }) {
                                        continue;
                                    }
                                    if let Ok(mut waiting) = hook_waiting_for_permission.lock() {
                                        *waiting = true;
                                    }
                                    let tool_name = parsed
                                        .get("tool_name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Tool approval required")
                                        .to_string();
                                    apply_agent_status_event(
                                        &hook_app,
                                        &hook_session,
                                        AgentEvent::ActionRequired {
                                            message: tool_name.clone(),
                                        },
                                        &hook_current_status,
                                    );
                                    let _ = hook_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({
                                            "session_id": hook_session,
                                            "data": {
                                                "type": "system",
                                                "subtype": "permission_request",
                                                "tool_name": tool_name,
                                            }
                                        }),
                                    );
                                }
                            }
                        }
                    }

                    watcher_profile.finish(0);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        }
    } else if config.provider == "antigravity" {
        let watcher_app = app.clone();
        let watcher_provider = provider.clone();
        let watcher_session = config.session_id.clone();
        let watcher_query_count = query_count.clone();
        let watcher_init_timestamp = init_timestamp.clone();
        let watcher_current_status = current_status.clone();
        let watcher_log_path = log_path.clone();
        let watcher_config = config_lock.clone();
        let watcher_watch_state = watch_state.clone();
        let watcher_skip_existing_log = is_restored;
        let watcher_workspace = provider_cwd.clone();
        let watcher_workspace_before = antigravity_workspace_before.clone();
        let watcher_database_baseline = antigravity_database_baseline;

        std::thread::spawn(move || {
            let mut offset: u64 = 0;
            let mut positioned_initial_log = !watcher_skip_existing_log;
            let mut last_conversation_id = String::new();
            let mut user_turn_receipt_tracker = AntigravityUserTurnReceiptTracker::default();
            let mut transcript_tracker = AntigravityTranscriptTracker::default();
            let mut database_watermark = None;
            loop {
                let current = watcher_current_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|e| e.into_inner().clone());
                if current == "Off" {
                    break;
                }
                let watcher_profile = crate::utils::runtime_profile::RuntimeProfileSpan::start(
                    crate::utils::runtime_profile::RuntimeMetric::AntigravityWatcherPoll,
                );

                let home = AntigravityProvider::antigravity_home();
                let (conversation_id, captured_identity) = {
                    let mut cfg = watcher_config.lock().unwrap_or_else(|e| e.into_inner());
                    let existing = cfg
                        .resume_session
                        .as_ref()
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    let (conversation_id, capture_identity) = antigravity_watcher_conversation(
                        existing,
                        watcher_workspace_before.as_deref(),
                        || {
                            let excluded = cfg.antigravity_config().cleared_conversations;
                            home.as_ref().and_then(|home| {
                                AntigravityProvider::fresh_database_conversation_for_workspace(
                                    home,
                                    &watcher_workspace,
                                    &watcher_database_baseline,
                                    &excluded,
                                )
                                .or_else(|| {
                                    AntigravityProvider::verified_conversation_for_workspace(
                                        home,
                                        &watcher_workspace,
                                        &excluded,
                                    )
                                })
                            })
                        },
                    );
                    if capture_identity {
                        if let Some(conversation_id) = conversation_id.as_deref() {
                            if apply_provider_identity("antigravity", &mut cfg, conversation_id)
                                .is_ok()
                            {
                                (Some(conversation_id.to_string()), true)
                            } else {
                                (Some(conversation_id.to_string()), false)
                            }
                        } else {
                            (None, false)
                        }
                    } else {
                        (conversation_id, false)
                    }
                };
                if captured_identity {
                    persist_runtime_agent_configs(&watcher_app);
                }

                let path = conversation_id.as_deref().and_then(|conversation_id| {
                    let cached = watcher_log_path
                        .lock()
                        .ok()
                        .and_then(|path| path.clone())
                        .filter(|path| last_conversation_id == conversation_id && path.is_file());
                    cached.or_else(|| {
                        home.as_ref().and_then(|home| {
                            AntigravityProvider::conversation_log_path(home, conversation_id)
                        })
                    })
                });

                if let Some(conversation_id) = conversation_id.as_deref() {
                    if last_conversation_id != conversation_id {
                        offset = 0;
                        positioned_initial_log = !watcher_skip_existing_log;
                        user_turn_receipt_tracker = AntigravityUserTurnReceiptTracker::default();
                        transcript_tracker = AntigravityTranscriptTracker::default();
                        database_watermark = None;
                        last_conversation_id = conversation_id.to_string();
                    }
                }

                let database_path = if path
                    .as_ref()
                    .is_some_and(|path| path.extension().is_some_and(|extension| extension == "db"))
                {
                    path.clone()
                } else if path.is_none() {
                    conversation_id.as_deref().and_then(|conversation_id| {
                        home.as_ref().and_then(|home| {
                            let database = AntigravityProvider::conversation_database_path(
                                home,
                                conversation_id,
                            );
                            database.is_file().then_some(database)
                        })
                    })
                } else {
                    None
                };

                if let Some(database_path) = database_path.as_ref() {
                    let observed_watermark = antigravity_database_watermark(database_path);
                    let source_changed = observed_watermark.is_none()
                        || database_watermark.as_ref() != observed_watermark.as_ref();
                    if source_changed {
                        // Preserve the watermark seen before the query. If the
                        // provider commits during this read, the next poll sees
                        // the newer file/WAL state and cannot miss that change.
                        database_watermark = observed_watermark;
                        if let Ok(latest_step_index) =
                            AntigravityProvider::latest_user_message_step_index(database_path)
                        {
                            if user_turn_receipt_tracker
                                .observe(latest_step_index, watcher_skip_existing_log)
                            {
                                apply_agent_event(
                                    &watcher_app,
                                    &watcher_session,
                                    AgentEvent::UserQuery,
                                    &watcher_query_count,
                                    &watcher_init_timestamp,
                                    &watcher_current_status,
                                );
                            }
                        }
                        if let Ok(messages) =
                            AntigravityProvider::conversation_messages_from_database_since(
                                database_path,
                                transcript_tracker.minimum_step_index(),
                            )
                        {
                            let projected =
                                transcript_tracker.observe(&messages, watcher_skip_existing_log);
                            if !projected.is_empty() {
                                if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                    for message in projected {
                                        watch_state.push_transcript(message);
                                    }
                                }
                            }
                        }
                    }
                }

                if let (Some(_conversation_id), Some(path)) = (conversation_id, path) {
                    if let Ok(mut out) = watcher_log_path.lock() {
                        *out = Some(path.clone());
                    }
                    // Current Antigravity keeps interactive history in SQLite.
                    // The database projection above feeds live watch state; the
                    // streaming watcher below remains for legacy JSONL logs.
                    if path.extension().is_some_and(|extension| extension == "db") {
                        watcher_profile.finish(0);
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    }
                    if let Ok(mut file) = std::fs::File::open(&path) {
                        if let Ok(metadata) = file.metadata() {
                            if metadata.len() < offset {
                                offset = 0;
                                positioned_initial_log = true;
                            }
                            if !positioned_initial_log {
                                offset = metadata.len();
                                positioned_initial_log = true;
                            }
                        }
                        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                            let mut reader = std::io::BufReader::new(file);
                            let mut line = String::new();
                            loop {
                                line.clear();
                                let read = reader.read_line(&mut line).unwrap_or(0);
                                if read == 0 {
                                    break;
                                }
                                crate::utils::runtime_profile::record_event(
                                    crate::utils::runtime_profile::RuntimeMetric::ProviderLogRead,
                                    read as u64,
                                );
                                offset += read as u64;
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(trimmed)
                                {
                                    let raw_line = parsed.to_string();
                                    if let Some(message) =
                                        extract_transcript_message("antigravity", &raw_line)
                                    {
                                        if let Ok(mut watch_state) = watcher_watch_state.lock() {
                                            watch_state.push_transcript(message);
                                        }
                                    }
                                    if let Some(event) = watcher_provider.parse_output(&raw_line) {
                                        apply_agent_event(
                                            &watcher_app,
                                            &watcher_session,
                                            event,
                                            &watcher_query_count,
                                            &watcher_init_timestamp,
                                            &watcher_current_status,
                                        );
                                    }
                                    let _ = watcher_app.emit(
                                        "agent-json-event",
                                        serde_json::json!({ "session_id": watcher_session, "data": parsed }),
                                    );
                                }
                            }
                        }
                    }
                }

                watcher_profile.finish(0);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    }

    // OpenCode creates a provider-owned session only once its interactive TUI
    // begins a turn. Capture that local identity instead of bootstrapping it
    // with an extra `opencode run` model request, then bind the listener that
    // was reserved before this same child was spawned.
    if config.provider == "opencode" && config.resume_session.is_none() {
        let mut fresh_opencode_http_plan = opencode_http_plan.take();
        let fresh_process_id = process_id;
        let fresh_runtime_generation = runtime_generation;
        let fresh_provider_generation = provider_generation;
        let fresh_broker = app_state.native_delivery.clone();
        let fresh_app = app.clone();
        let fresh_expected_folder = expected_folder.clone();
        let fresh_workspace = cwd.clone();
        let watcher_app = app.clone();
        let watcher_config = config_lock.clone();
        let watcher_current_status = current_status.clone();
        let watcher_workspace = cwd.clone();
        let watcher_session = config.session_id.clone();
        let started_after_ms = chrono::Utc::now().timestamp_millis();
        let mut discovery = OpenCodeSessionDiscovery::default();
        std::thread::spawn(move || loop {
            let current = watcher_current_status
                .lock()
                .map(|status| status.clone())
                .unwrap_or_default();
            if current == "Off" {
                break;
            }
            if let Some(provider_session_id) = discovery.poll(
                &current,
                &watcher_workspace,
                started_after_ms,
                &watcher_session,
            ) {
                let binding_config = if let Ok(mut cfg) = watcher_config.lock() {
                    cfg.resume_session = Some(provider_session_id);
                    cfg.fresh_provider_session_id = None;
                    cfg.folder = fresh_expected_folder.clone();
                    Some(cfg.clone())
                } else {
                    None
                };
                persist_runtime_agent_configs(&watcher_app);
                let Some(mut binding_config) = binding_config else {
                    tauri::async_runtime::block_on(
                        fresh_broker
                            .fail_opencode_http(&watcher_session, fresh_provider_generation),
                    );
                    break;
                };
                let Some(plan) = fresh_opencode_http_plan.take() else {
                    break;
                };
                let Some(process_id) = fresh_process_id else {
                    tauri::async_runtime::block_on(
                        fresh_broker
                            .fail_opencode_http(&watcher_session, fresh_provider_generation),
                    );
                    break;
                };
                let provider_session_id = binding_config
                    .resume_session
                    .clone()
                    .expect("fresh OpenCode discovery set resume_session");
                let config_fingerprint = {
                    binding_config.folder = fresh_expected_folder.clone();
                    crate::delivery::native_broker::opencode_http_config_fingerprint(
                        &binding_config,
                        &fresh_workspace,
                    )
                };
                tauri::async_runtime::block_on(async move {
                    if let Err(error) = fresh_broker
                        .prepare_opencode_http(
                            &watcher_session,
                            fresh_provider_generation,
                            config_fingerprint.clone(),
                        )
                        .await
                    {
                        log_debug(&format!(
                            "[Wardian] OpenCode HTTP fresh owner preparation failed: {error}"
                        ));
                        fresh_broker
                            .fail_opencode_http(&watcher_session, fresh_provider_generation)
                            .await;
                        return;
                    }
                    register_opencode_http_after_launch(OpenCodeHttpLaunchContext {
                        app: fresh_app,
                        broker: fresh_broker,
                        agent_id: watcher_session,
                        plan,
                        provider_session_id,
                        process_id,
                        workspace: fresh_workspace,
                        config_fingerprint,
                        runtime_generation: fresh_runtime_generation,
                        provider_generation: fresh_provider_generation,
                    })
                    .await;
                });
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
    }

    // ── OpenCode log-file watcher ─────────────────────────────────────────
    {
        let mut cfg = config_lock.lock().unwrap();
        cfg.folder = expected_folder;
    }

    if publication == SpawnPublication::Synchronous {
        if let Some(guard) = codex_attach_guard.as_mut() {
            guard.attached();
        }
    }
    let active = ActiveAgent {
        config: config_lock.clone(),
        child_process: Some(child.attached()),
        background_processes,
        memory_capability,
        runtime_generation: Some(runtime_generation),
        process_id,
        query_count,
        init_timestamp,
        last_query_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
        current_status: current_status.clone(),
        last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
        watch_state: watch_state.clone(),
        terminal_title,
        last_output_at,
        log_path,
        log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
        #[cfg(windows)]
        job_object,
    };
    if publication == SpawnPublication::Provisional {
        codex_attachment_completion = codex_attachment.as_ref().map(|attachment| {
            CodexAttachmentCompletion::new(CodexAttachmentCompletionContext {
                app: app.clone(),
                session_id: config.session_id.clone(),
                provider_generation: attachment.generation,
                runtime_generation,
                config_lock,
                current_status: current_status.clone(),
                watch_state,
                native_delivery: app_state.native_delivery.clone(),
                codex_attachment_ready,
                codex_reader_alive,
                cleanup_guard: codex_attach_guard.take(),
            })
        });
    }
    release_provider_spawn_lease_after_readiness(
        spawn_lease,
        current_status,
        config.session_id.clone(),
        spawn_bootstrap_complete,
        app,
        runtime_generation,
        gate,
    );
    Ok(SpawnedAgent {
        active,
        completion: codex_attachment_completion,
    })
}

pub async fn resize_pty(
    session_id: String,
    cols: u16,
    rows: u16,
    state: &AppState,
) -> Result<(), String> {
    if cols < 10 {
        return Ok(());
    }
    let geometry = wardian_core::models::TerminalGeometry { cols, rows };
    match state
        .terminal_sessions
        .resize_legacy(&session_id, geometry)
        .await
    {
        Ok(result)
            if result.decision.status
                == wardian_core::models::TerminalLeaseDecisionStatus::Accepted =>
        {
            Ok(())
        }
        Ok(result) => Err(format!(
            "Terminal resize lease rejected: {}",
            result
                .decision
                .reason
                .map(|reason| format!("{reason:?}"))
                .unwrap_or_else(|| "unknown".to_string())
        )),
        Err(crate::state::terminal_session::TerminalBrokerError::SessionNotFound) => {
            let agents = state.agents.lock().await;
            if !agents.contains_key(&session_id) {
                return Err(format!("Agent {} not found", session_id));
            }
            drop(agents);
            state
                .terminal_sessions
                .remember_deferred_geometry(&session_id, "legacy-resize-adapter", geometry)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::models::{AgentProvider, CodexProviderConfig, ProviderConfig};

    #[test]
    fn pi_restore_keeps_the_saved_project_directory() {
        let dir = tempfile::tempdir().expect("test directory");
        let workspace = dir.path().join("workspace");
        let habitat = dir.path().join("habitat");
        let habitat_workspace = super::super::habitat_workspace_cwd(&habitat);
        let session = dir.path().join("session.jsonl");

        for saved in [&workspace, &habitat_workspace] {
            std::fs::write(
                &session,
                format!(
                    "{{\"type\":\"session\",\"id\":\"pi-test\",\"cwd\":{}}}\n",
                    serde_json::to_string(&saved.to_string_lossy()).expect("project path")
                ),
            )
            .expect("session header");
            assert_eq!(
                pi_session_project_cwd(
                    "pi-agent",
                    &workspace,
                    Some(&habitat),
                    Some(&session),
                    true
                ),
                saved.clone()
            );
            assert_eq!(
                interactive_provider_launch_cwd(
                    "pi",
                    "pi-agent",
                    Some(&habitat),
                    &workspace,
                    saved,
                    true,
                )
                .expect("launch path"),
                saved.clone()
            );
        }
    }

    #[test]
    fn pi_restore_rejects_an_unrelated_saved_project_directory() {
        let dir = tempfile::tempdir().expect("test directory");
        let workspace = dir.path().join("workspace");
        let habitat = dir.path().join("habitat");
        let session = dir.path().join("session.jsonl");
        std::fs::write(
            &session,
            "{\"type\":\"session\",\"id\":\"pi-test\",\"cwd\":\"C:/other-project\"}\n",
        )
        .expect("session header");
        assert_eq!(
            pi_session_project_cwd("pi-agent", &workspace, Some(&habitat), Some(&session), true),
            workspace
        );
    }

    #[cfg(windows)]
    #[test]
    fn fresh_pi_long_workspace_uses_habitat_project_identity() {
        let dir = tempfile::tempdir().expect("test directory");
        let workspace = dir.path().join("w".repeat(260));
        let habitat = dir.path().join("habitat");
        let habitat_workspace = super::super::habitat_workspace_cwd(&habitat);
        assert_eq!(
            pi_session_project_cwd("pi-agent", &workspace, Some(&habitat), None, false),
            habitat_workspace
        );
        // A Wardian agent may be restored before Pi writes its first JSONL.
        let provider_cwd =
            pi_session_project_cwd("pi-agent", &workspace, Some(&habitat), None, true);
        assert_eq!(provider_cwd, habitat_workspace);
        assert_eq!(
            interactive_provider_launch_cwd(
                "pi",
                "pi-agent",
                Some(&habitat),
                &workspace,
                &provider_cwd,
                false,
            )
            .expect("fresh Pi launch path"),
            habitat_workspace
        );
    }

    #[test]
    fn codex_status_log_session_does_not_use_latest_fallback() {
        let config = AgentConfig {
            provider: "codex".to_string(),
            resume_session: None,
            provider_config: ProviderConfig::Codex(CodexProviderConfig {
                cleared_provider_sessions: vec!["provider-session-1".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };

        let log_session = codex_status_log_session(&config);

        assert_eq!(log_session, None);
        assert_eq!(config.resume_session, None);
        assert_eq!(
            config.codex_config().cleared_provider_sessions,
            vec!["provider-session-1".to_string()]
        );
    }

    #[test]
    fn claude_status_log_session_prefers_the_provider_identity() {
        let config = AgentConfig {
            provider: "claude".to_string(),
            session_id: "wardian-session".to_string(),
            resume_session: Some("provider-session".to_string()),
            fresh_provider_session_id: Some("fresh-provider-session".to_string()),
            ..Default::default()
        };

        assert_eq!(claude_status_log_session(&config), "provider-session");

        let fresh_config = AgentConfig {
            resume_session: None,
            ..config
        };
        assert_eq!(
            claude_status_log_session(&fresh_config),
            "fresh-provider-session"
        );
    }

    #[test]
    fn provider_spawn_lease_excludes_cross_process_execution_at_launch_boundary() {
        let _home = crate::control::test_support::TestWardianHome::new();
        let now = chrono::Utc::now();
        let now_rfc3339 = now.to_rfc3339();
        let config = AgentConfig {
            session_id: "agent-1".into(),
            provider: "codex".into(),
            resume_session: Some("resume-1".into()),
            ..Default::default()
        };

        let spawn_lease = acquire_provider_spawn_lease(&config).expect("spawn reservation");
        let background_lease = wardian_core::conversation_lease::ConversationLease {
            agent_id: config.session_id.clone(),
            provider: config.provider.clone(),
            resume_session: "resume-1".into(),
            owner_kind: "automation_run".into(),
            owner_id: "run-1".into(),
            acquisition_id: "background-acquisition".into(),
            owner_node_id: Some("agent-1".into()),
            mode: "background_resume".into(),
            started_at: now_rfc3339.clone(),
            heartbeat_at: now_rfc3339.clone(),
            expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
        };
        let outcome = wardian_core::conversation_lease::try_acquire_lease(
            background_lease.clone(),
            &now_rfc3339,
        )
        .expect("competing background acquisition");
        assert!(matches!(
            outcome,
            wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Conflict(_)
        ));

        drop(spawn_lease);
        wardian_core::conversation_lease::try_acquire_lease(background_lease.clone(), &now_rfc3339)
            .expect("background lease after launch reservation release");
        assert!(acquire_provider_spawn_lease(&config).is_err());
    }

    #[tokio::test]
    async fn codex_native_owner_preparation_runs_only_after_spawn_reservation() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "codex".into(),
            resume_session: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        };
        let (mut lease, owner) = prepare_codex_owner_after_reservation(&config, None, || async {
            let leases = wardian_core::conversation_lease::load_leases_checked()?;
            assert!(leases.iter().any(|entry| {
                entry.agent_id == config.session_id && entry.mode == "lifecycle_transition"
            }));
            Ok("prepared")
        })
        .await
        .expect("prepare after reservation");
        assert_eq!(owner, "prepared");
        lease.release().expect("release test reservation");

        let now = chrono::Utc::now();
        let background = wardian_core::conversation_lease::ConversationLease {
            agent_id: config.session_id.clone(),
            provider: config.provider.clone(),
            resume_session: config.resume_session.clone().unwrap(),
            owner_kind: "automation_run".into(),
            owner_id: "other-writer".into(),
            acquisition_id: uuid::Uuid::new_v4().to_string(),
            owner_node_id: None,
            mode: "background_resume".into(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
        };
        assert!(matches!(
            wardian_core::conversation_lease::try_acquire_lease(background, &now.to_rfc3339())
                .expect("competing writer lease"),
            wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
        ));
        let prepared = std::sync::atomic::AtomicBool::new(false);
        let result = prepare_codex_owner_after_reservation(&config, None, || async {
            prepared.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        })
        .await;
        assert!(result.is_err());
        assert!(!prepared.load(std::sync::atomic::Ordering::Acquire));

        let cancelled_config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "codex".into(),
            resume_session: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        };
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            prepare_codex_owner_after_reservation(&cancelled_config, None, || async {
                std::future::pending::<Result<(), String>>().await
            }),
        )
        .await;
        assert!(cancelled.is_err());
        assert!(wardian_core::conversation_lease::load_leases_checked()
            .expect("retained lease after cancellation")
            .iter()
            .any(|entry| entry.agent_id == cancelled_config.session_id
                && entry.mode == "lifecycle_transition"));
    }

    #[tokio::test]
    async fn active_persisted_headless_lease_keeps_restore_headless_and_blocks_spawn() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            session_name: "restore-active-headless".into(),
            provider: "codex".into(),
            resume_session: Some("resume-1".into()),
            ..Default::default()
        };
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &config.session_id,
            session_name: &config.session_name,
            description: "",
            agent_class: "Coder",
            provider: &config.provider,
            workspace: None,
            project: None,
            is_off: false,
            created_at: None,
        })
        .expect("insert isolated persisted agent");
        wardian_core::db::update_agent_status(&config.session_id, "Processing...", Some(424242))
            .expect("persist old PID");

        let now = chrono::Utc::now();
        let lease = wardian_core::conversation_lease::ConversationLease {
            agent_id: config.session_id.clone(),
            provider: config.provider.clone(),
            resume_session: "resume-1".into(),
            owner_kind: "automation_run".into(),
            owner_id: "run-1".into(),
            acquisition_id: uuid::Uuid::new_v4().to_string(),
            owner_node_id: Some(config.session_id.clone()),
            mode: "background_resume".into(),
            started_at: now.to_rfc3339(),
            heartbeat_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(5)).to_rfc3339(),
        };
        assert!(matches!(
            wardian_core::conversation_lease::try_acquire_lease(lease.clone(), &now.to_rfc3339())
                .expect("persist active headless lease"),
            wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
        ));

        let leases = wardian_core::conversation_lease::load_leases_checked()
            .expect("read active headless lease");
        crate::reconcile_headless_agents(&leases)
            .await
            .expect("reconcile active headless execution");
        let persisted = wardian_core::db::get_all_agents()
            .expect("read reconciled agents")
            .into_iter()
            .find(|agent| agent.session_id == config.session_id)
            .expect("reconciled agent exists");
        assert_eq!(persisted.last_status.as_deref(), Some("Headless"));
        assert_eq!(persisted.last_pid, None);
        assert!(acquire_provider_spawn_lease(&config).is_err());
    }

    #[test]
    fn provider_spawn_lease_waits_for_readiness_or_terminal_failure() {
        for status in ["Starting", "Processing", "Action Needed"] {
            assert!(
                !provider_spawn_lease_should_release(status),
                "lease must remain while provider status is {status}"
            );
        }
        for status in ["Idle", "Error", "Off"] {
            assert!(
                provider_spawn_lease_should_release(status),
                "lease should release after provider status is {status}"
            );
        }
    }

    #[test]
    fn native_mock_init_shape_parses_across_pty_chunks_and_confirms_bootstrap() {
        let provider_session = "e2e-remote-gateway-diagnostic";
        let config = std::sync::Arc::new(std::sync::Mutex::new(AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "mock".into(),
            resume_session: Some(provider_session.into()),
            ..Default::default()
        }));
        let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(None));
        let bootstrap = std::sync::atomic::AtomicBool::new(false);
        // The native fixture writes an Init JSON line followed by a ready
        // marker. Split the JSON where a PTY read may end.
        let first = format!(
            "\x1b[0m{{\"type\":\"init\",\"session_id\":\"{provider_session}\",\"timestamp\":\"2026-09-24T00:00:00.000Z\""
        );
        let mut current_line = first;
        assert!(serde_json::Deserializer::from_str(
            &current_line[current_line.find('{').unwrap()..]
        )
        .into_iter::<serde_json::Value>()
        .next()
        .is_none_or(|result| result.is_err()));
        current_line.push_str("}\r\nREMOTE_BROKER_READY\r\n");
        let json_start = current_line.find('{').expect("Init object start");
        let mut stream = serde_json::Deserializer::from_str(&current_line[json_start..])
            .into_iter::<serde_json::Value>();
        let raw = stream
            .next()
            .expect("complete Init")
            .expect("valid Init")
            .to_string();
        let event = crate::providers::MockProvider::new()
            .parse_output(&raw)
            .expect("mock Init event");
        assert_eq!(
            handle_provider_init_with_spawn_lease(
                "mock",
                &event,
                &config,
                &init_timestamp,
                &bootstrap,
            ),
            Ok(ProviderIdentityOutcome::Confirmed)
        );
        assert!(bootstrap.load(std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn validated_mock_init_releases_spawn_lease_only_after_exact_publication() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "mock".into(),
            resume_session: Some("mock-provider-session".into()),
            ..Default::default()
        };
        let spawn_lease = acquire_provider_spawn_lease(&config).expect("spawn reservation");
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let current_status = std::sync::Arc::new(std::sync::Mutex::new("Starting".to_string()));
        let config_lock = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));
        let init_timestamp = std::sync::Arc::new(std::sync::Mutex::new(None));
        let bootstrap_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        release_provider_spawn_lease_after_readiness(
            spawn_lease,
            current_status.clone(),
            config.session_id.clone(),
            bootstrap_complete.clone(),
            app.handle().clone(),
            7,
            SpawnPublicationGate::default(),
        );
        let provider = crate::providers::MockProvider::new();
        let rejected = provider
            .parse_output(r#"{"type":"init","session_id":"other-session"}"#)
            .expect("mock Init event");

        assert!(handle_provider_init_with_spawn_lease(
            "mock",
            &rejected,
            &config_lock,
            &init_timestamp,
            &bootstrap_complete,
        )
        .is_err());
        assert!(!bootstrap_complete.load(std::sync::atomic::Ordering::Acquire));
        assert!(acquire_provider_spawn_lease(&config).is_err());

        let accepted = provider
            .parse_output(r#"{"type":"init","session_id":"mock-provider-session"}"#)
            .expect("mock Init event");
        assert!(matches!(
            handle_provider_init_with_spawn_lease(
                "mock",
                &accepted,
                &config_lock,
                &init_timestamp,
                &bootstrap_complete,
            ),
            Ok(ProviderIdentityOutcome::Confirmed)
        ));
        assert!(!mock_init_confirms_bootstrap(
            "mock",
            ProviderIdentityOutcome::Captured,
            &accepted,
            Some("mock-provider-session"),
        ));
        assert!(bootstrap_complete.load(std::sync::atomic::Ordering::Acquire));
        assert!(!provider_spawn_lease_should_release("Starting"));
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            acquire_provider_spawn_lease(&config).is_err(),
            "Init before publication must keep the lease"
        );
        *current_status.lock().unwrap() = "Idle".to_string();
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            acquire_provider_spawn_lease(&config).is_err(),
            "Idle before publication must keep the lease"
        );
        *current_status.lock().unwrap() = "Error".to_string();
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            acquire_provider_spawn_lease(&config).is_err(),
            "a failed replacement before publication retains uncertain ownership"
        );
        *current_status.lock().unwrap() = "Starting".to_string();

        let mut published = agent_without_pty();
        published.config = config_lock;
        published.current_status = current_status;
        published.runtime_generation = Some(6);
        app.state::<AppState>()
            .agents
            .lock()
            .await
            .insert(config.session_id.clone(), published);
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            acquire_provider_spawn_lease(&config).is_err(),
            "another runtime generation must not release the lease"
        );
        app.state::<AppState>()
            .agents
            .lock()
            .await
            .get_mut(&config.session_id)
            .unwrap()
            .runtime_generation = Some(7);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while wardian_core::conversation_lease::load_leases_checked()
            .expect("lease store")
            .iter()
            .any(|lease| lease.owner_kind == "provider_spawn")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "lease stayed reserved"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let _replacement = acquire_provider_spawn_lease(&config)
            .expect("validated and published mock runtime releases its reservation");
    }

    #[tokio::test]
    async fn unpublished_replacement_failure_stops_renewal_but_keeps_fence_until_expiry() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "mock".into(),
            resume_session: Some("failed-provider-session".into()),
            ..Default::default()
        };
        let lease = acquire_provider_spawn_lease(&config).expect("spawn reservation");
        let owner = lease.owner().clone();
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = release_provider_spawn_lease_after_readiness(
            lease,
            std::sync::Arc::new(std::sync::Mutex::new("Error".to_string())),
            config.session_id.clone(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            app.handle().clone(),
            9,
            SpawnPublicationGate {
                unpublished_failure: Some(failed.clone()),
                registration: None,
            },
        );
        assert!(acquire_provider_spawn_lease(&config).is_err());
        failed.store(true, std::sync::atomic::Ordering::Release);
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher)
            .await
            .expect("watcher stops after unpublished failure")
            .expect("watcher completed");
        let retained = wardian_core::conversation_lease::load_leases_checked()
            .expect("retained lease")
            .into_iter()
            .find(|lease| lease.owner() == owner)
            .expect("uncertain lease remains persisted");
        assert!(acquire_provider_spawn_lease(&config).is_err());

        let retry_at = chrono::DateTime::parse_from_rfc3339(&retained.expires_at)
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::seconds(1);
        let mut retry = retained;
        retry.owner_id = "retry-after-expiry".into();
        retry.acquisition_id = uuid::Uuid::new_v4().to_string();
        retry.started_at = retry_at.to_rfc3339();
        retry.heartbeat_at = retry.started_at.clone();
        retry.expires_at = (retry_at + PROVIDER_SPAWN_LEASE_DURATION).to_rfc3339();
        assert!(matches!(
            wardian_core::conversation_lease::try_acquire_lease(retry, &retry_at.to_rfc3339())
                .expect("retry after expiry"),
            wardian_core::conversation_lease::ConversationLeaseAcquireOutcome::Acquired
        ));
    }

    #[tokio::test]
    async fn cancelled_caller_after_watcher_handoff_retains_finite_spawn_lease() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            provider: "mock".into(),
            resume_session: Some("cancelled-publication".into()),
            ..Default::default()
        };
        let lease = acquire_provider_spawn_lease(&config).expect("spawn reservation");
        let owner = lease.owner().clone();
        let original_expiry = wardian_core::conversation_lease::load_leases_checked()
            .unwrap()
            .into_iter()
            .find(|entry| entry.owner() == owner)
            .unwrap()
            .expires_at;
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        let app_handle = app.handle().clone();
        let (ready, received) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            let disposition = SpawnPublicationDisposition::new();
            let watcher = release_provider_spawn_lease_after_readiness(
                lease,
                std::sync::Arc::new(std::sync::Mutex::new("Starting".to_string())),
                config.session_id,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                app_handle,
                9,
                SpawnPublicationGate {
                    unpublished_failure: Some(disposition.failure_signal()),
                    registration: None,
                },
            );
            ready.send(watcher).unwrap();
            std::future::pending::<()>().await;
        });
        let watcher = received.await.expect("watcher handoff");
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher)
            .await
            .expect("cancelled publication stops watcher renewal")
            .expect("watcher completed");
        let retained = wardian_core::conversation_lease::load_leases_checked()
            .unwrap()
            .into_iter()
            .find(|entry| entry.owner() == owner)
            .expect("uncertain provider lease remains");
        assert_eq!(retained.expires_at, original_expiry);
    }

    #[tokio::test]
    async fn registration_waits_for_commit_and_failed_publication_stops_renewal() {
        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let app = tauri::test::mock_app();
        app.manage(AppState::new());
        for (kind, fail) in [
            ("ordinary", false),
            ("ordinary", true),
            ("provisional", false),
            ("provisional", true),
        ] {
            let config = AgentConfig {
                session_id: uuid::Uuid::new_v4().to_string(),
                provider: "mock".into(),
                resume_session: Some(format!("{kind}-{fail}-registration")),
                ..Default::default()
            };
            let lease = acquire_provider_spawn_lease(&config).expect("registration lease");
            let owner = lease.owner().clone();
            let publication = std::sync::Arc::new(RegistrationPublicationState::default());
            let status = std::sync::Arc::new(std::sync::Mutex::new("Idle".to_string()));
            let mut agent = agent_without_pty();
            agent.config = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));
            agent.current_status = status.clone();
            agent.runtime_generation = Some(42);
            app.state::<AppState>()
                .agents
                .lock()
                .await
                .insert(config.session_id.clone(), agent);
            let watcher = release_provider_spawn_lease_after_readiness(
                lease,
                status,
                config.session_id.clone(),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                app.handle().clone(),
                42,
                SpawnPublicationGate {
                    unpublished_failure: None,
                    registration: Some(publication.clone()),
                },
            );
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            assert!(
                acquire_provider_spawn_lease(&config).is_err(),
                "{kind} Idle before registration commit must retain exclusion"
            );
            if fail {
                publication.fail();
            } else {
                publication.commit();
            }
            tokio::time::timeout(std::time::Duration::from_secs(2), watcher)
                .await
                .expect("watcher disposition")
                .expect("watcher completed");
            let persisted = wardian_core::conversation_lease::load_leases_checked()
                .expect("lease store")
                .into_iter()
                .find(|lease| lease.owner() == owner);
            assert_eq!(persisted.is_some(), fail, "{kind} publication disposition");
            if fail {
                assert!(acquire_provider_spawn_lease(&config).is_err());
            }
        }
    }

    #[tokio::test]
    async fn orphan_marked_python_server_does_not_block_restore_or_get_terminated() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct OwnedTestServer(Child);

        impl Drop for OwnedTestServer {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let _home = crate::control::test_support::TestWardianHome::new_async().await;
        let config = AgentConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            session_name: "restore-orphan-server".into(),
            provider: "codex".into(),
            ..Default::default()
        };
        wardian_core::db::upsert_agent(&wardian_core::db::AgentUpsert {
            session_id: &config.session_id,
            session_name: &config.session_name,
            description: "",
            agent_class: "Coder",
            provider: &config.provider,
            workspace: None,
            project: None,
            is_off: false,
            created_at: None,
        })
        .expect("insert isolated persisted agent");
        wardian_core::db::update_agent_status(&config.session_id, "Headless", Some(424242))
            .expect("persist stale Headless status and PID");

        let port = TcpListener::bind(("127.0.0.1", 0))
            .expect("reserve local test port")
            .local_addr()
            .expect("read local test port")
            .port();
        let port_text = port.to_string();
        let mut server = OwnedTestServer(
            Command::new(if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            })
            .args([
                "-u",
                "-m",
                "http.server",
                port_text.as_str(),
                "--bind",
                "127.0.0.1",
            ])
            .env("WARDIAN_SESSION_ID", &config.session_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated marked Python HTTP server"),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                server.0.try_wait().expect("poll test server").is_none(),
                "fixture server exited before startup"
            );
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fixture server did not become ready"
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        let persisted = wardian_core::db::get_all_agents()
            .expect("read persisted agents")
            .into_iter()
            .find(|agent| agent.session_id == config.session_id)
            .expect("persisted agent exists");
        assert_eq!(persisted.last_status.as_deref(), Some("Headless"));
        assert_eq!(persisted.last_pid, Some(424242));
        assert!(
            crate::utils::process::find_wardian_provider_process_candidates(
                &config.session_id,
                &config.provider,
                Some(std::process::id()),
            )
            .is_empty(),
            "the marked Python server is not a provider process candidate"
        );

        crate::reconcile_headless_agents(&[])
            .await
            .expect("reconcile stale Headless status from empty lease store");
        let reconciled = wardian_core::db::get_all_agents()
            .expect("read reconciled agents")
            .into_iter()
            .find(|agent| agent.session_id == config.session_id)
            .expect("reconciled agent exists");
        assert_eq!(reconciled.last_status.as_deref(), Some("Off"));
        assert_eq!(reconciled.last_pid, None);

        // This is the production pre-spawn gate. The fixture does not launch a
        // Tauri PTY or real Codex provider; it proves recovery reaches that
        // boundary while the unrelated marked server remains alive and serving.
        let spawn_lease = acquire_provider_spawn_lease(&config)
            .expect("provider spawn gate should admit restore");
        drop(spawn_lease);
        assert!(server
            .0
            .try_wait()
            .expect("poll surviving test server")
            .is_none());

        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("reconnect to server");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("set bounded test read");
        stream
            .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("request HTTP response");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read HTTP response");
        assert!(response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn pi_bridge_child_handoff_diagnostic_distinguishes_pid_states() {
        assert_eq!(pi_bridge_child_handoff_code(Some(42)), "process_registered");
        assert_eq!(pi_bridge_child_handoff_code(Some(0)), "process_id_zero");
        assert_eq!(pi_bridge_child_handoff_code(None), "process_id_unavailable");
    }

    #[test]
    fn pi_fresh_provider_session_promotion_retains_fresh_marker() {
        let mut config = AgentConfig {
            provider: "pi".to_string(),
            session_id: "wardian-session".to_string(),
            fresh_provider_session_id: Some("pi-fresh-session".to_string()),
            ..Default::default()
        };

        retain_pi_fresh_provider_session(&mut config);

        assert_eq!(config.resume_session.as_deref(), Some("pi-fresh-session"));
        assert_eq!(
            config.fresh_provider_session_id.as_deref(),
            Some("pi-fresh-session")
        );
    }

    #[test]
    fn opencode_http_launch_accepts_fresh_identity_then_rejects_invalid_identity() {
        assert!(opencode_http_launch_identity_is_valid(None));
        assert!(opencode_http_launch_identity_is_valid(Some("ses_exact")));
        assert!(!opencode_http_launch_identity_is_valid(Some("ses/other")));
        assert!(!opencode_http_launch_identity_is_valid(Some(
            "wardian-agent"
        )));
    }

    #[test]
    fn restored_pi_baseline_preserves_events_appended_before_first_watcher_poll() {
        let dir = tempfile::tempdir().expect("Pi session directory");
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
                "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"old\",\"stopReason\":\"stop\"}}\n",
            ),
        )
        .expect("existing Pi transcript");
        let baseline = pi_log_baseline(dir.path(), "pi-session").expect("Pi baseline");

        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append Pi turn before watcher starts");
        writeln!(
            append,
            "{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"content\":\"new\",\"stopReason\":\"stop\"}}}}"
        )
        .expect("new Pi turn");
        drop(append);

        let mut cursor = baseline.cursor;
        let mut file = open_pi_log_at_cursor(&baseline.path, &mut cursor).expect("positioned log");
        let mut observed = String::new();
        file.read_to_string(&mut observed).expect("new Pi events");

        assert!(!observed.contains("old"));
        assert!(observed.contains("new"));
    }

    #[test]
    fn restored_pi_cursor_resets_for_larger_same_path_replacement() {
        let dir = tempfile::tempdir().expect("Pi session directory");
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
                "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"old\",\"stopReason\":\"stop\"}}\n",
            ),
        )
        .expect("existing Pi transcript");
        let baseline = pi_log_baseline(dir.path(), "pi-session").expect("Pi baseline");
        let replacement = dir.path().join("replacement.jsonl");
        let new_content = format!(
            "{}{}{}",
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"new\",\"stopReason\":\"stop\"}}\n",
            "x".repeat(baseline.cursor.offset as usize),
        );
        std::fs::write(&replacement, new_content).expect("replacement Pi transcript");
        std::fs::remove_file(&path).expect("remove old Pi transcript");
        std::fs::rename(&replacement, &path).expect("replace Pi transcript at same path");

        let mut cursor = baseline.cursor;
        let mut file = open_pi_log_at_cursor(&baseline.path, &mut cursor).expect("replacement log");
        let mut observed = String::new();
        file.read_to_string(&mut observed)
            .expect("replacement Pi events");

        assert_eq!(cursor.offset, 0);
        assert!(observed.contains("new"));
        assert!(!observed.contains("old"));
    }

    #[test]
    fn restored_pi_cursor_resets_for_in_place_rewrite_after_prefix() {
        let dir = tempfile::tempdir().expect("Pi session directory");
        let path = dir.path().join("session.jsonl");
        let padding = "x".repeat(8192);
        let old_content = format!(
            "{}{{\"type\":\"padding\",\"content\":\"{}\"}}\n{}",
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            padding,
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":\"old\",\"stopReason\":\"stop\"}}\n",
        );
        std::fs::write(&path, &old_content).expect("existing Pi transcript");
        let baseline = pi_log_baseline(dir.path(), "pi-session").expect("Pi baseline");
        let new_content = old_content.replace("\"content\":\"old\"", "\"content\":\"new\"");
        assert_eq!(new_content.len(), old_content.len());
        assert_eq!(&new_content[..4096], &old_content[..4096]);
        std::fs::write(&path, new_content).expect("in-place Pi transcript rewrite");

        let mut cursor = baseline.cursor;
        let mut file = open_pi_log_at_cursor(&baseline.path, &mut cursor).expect("rewritten log");
        let mut observed = String::new();
        file.read_to_string(&mut observed)
            .expect("rewritten Pi events");
        let assistant_messages = observed
            .lines()
            .filter_map(|line| extract_transcript_message("pi", line))
            .collect::<Vec<_>>();

        assert_eq!(cursor.offset, 0);
        assert!(assistant_messages
            .iter()
            .any(|message| message.text == "new"));
        assert!(!assistant_messages
            .iter()
            .any(|message| message.text == "old"));
    }

    fn agent_without_pty() -> crate::state::ActiveAgent {
        crate::state::ActiveAgent {
            config: std::sync::Arc::new(std::sync::Mutex::new(AgentConfig::default())),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: None,
            process_id: None,
            query_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            init_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            last_query_timestamp: std::sync::Arc::new(std::sync::Mutex::new(None)),
            current_status: std::sync::Arc::new(std::sync::Mutex::new("Restoring".to_string())),
            last_status_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            watch_state: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::AgentWatchState::new("restoring-agent".to_string(), 4096, 262_144),
            )),
            terminal_title: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            last_output_at: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
            log_last_modified: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    #[test]
    fn active_agent_lease_survives_reader_lifecycle_until_runtime_drop() {
        let temp = tempfile::tempdir().unwrap();
        let store = wardian_core::memory::MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let lease = store.issue_process_capability("agent-a").unwrap();
        let token = lease.token().to_string();
        let mut agent = agent_without_pty();
        agent.memory_capability = Some(lease);

        // PTY readers hold no revoker. A reader/broker exit therefore leaves
        // the provider runtime's ActiveAgent-owned authority intact.
        assert!(store.validate_capability("agent-a", &token).unwrap());
        drop(agent);
        assert!(!store.validate_capability("agent-a", &token).unwrap());
    }

    // A resize that arrives while the agent is still a "Restoring" placeholder
    // is retained by the broker and seeds the native runtime when spawn begins.
    #[tokio::test]
    async fn resize_without_pty_records_size_for_spawn() {
        let state = AppState::new();
        state
            .agents
            .lock()
            .await
            .insert("restoring-agent".to_string(), agent_without_pty());

        let result = resize_pty("restoring-agent".to_string(), 124, 30, &state).await;

        assert!(result.is_ok());
        assert_eq!(
            state
                .terminal_sessions
                .spawn_geometry("restoring-agent")
                .await
                .expect("spawn geometry"),
            Some(wardian_core::models::TerminalGeometry {
                cols: 124,
                rows: 30
            })
        );
    }

    #[tokio::test]
    async fn resize_unknown_agent_still_errors() {
        let state = AppState::new();
        let result = resize_pty("missing".to_string(), 124, 30, &state).await;
        assert!(result.is_err());
        assert_eq!(
            state
                .terminal_sessions
                .spawn_geometry("missing")
                .await
                .expect("missing geometry"),
            None
        );
    }

    #[test]
    fn codex_line_status_preserves_action_needed_until_completion() {
        assert_eq!(
            line_event_status_for_pty_provider(
                "codex",
                "Idle",
                &AgentEvent::ActionRequired {
                    message: "approve command".to_string(),
                },
            ),
            Some("Action Needed")
        );
        assert_eq!(
            line_event_status_for_pty_provider("codex", "Action Needed", &AgentEvent::Generating),
            None
        );
        assert_eq!(
            line_event_status_for_pty_provider(
                "codex",
                "Action Needed",
                &AgentEvent::TurnCompleted,
            ),
            Some("Idle")
        );
    }

    #[test]
    fn mock_line_status_waits_for_explicit_turn_completion() {
        assert_eq!(
            line_event_status_for_pty_provider("mock", "Processing...", &AgentEvent::ModelResponse),
            None
        );
        assert_eq!(
            line_event_status_for_pty_provider("mock", "Processing...", &AgentEvent::TurnCompleted),
            Some("Idle")
        );
    }

    #[test]
    fn output_ready_emit_gate_coalesces_repeats_after_throttle() {
        let mut gate = OutputReadyEmitGate::default();
        let start = std::time::Instant::now();

        assert_eq!(
            gate.after_buffer_append(start),
            OutputReadyEmitAction::EmitNow
        );
        assert_eq!(
            gate.after_buffer_append(start + OUTPUT_READY_EMIT_MIN_INTERVAL / 2),
            OutputReadyEmitAction::ScheduleAfter(OUTPUT_READY_EMIT_MIN_INTERVAL / 2)
        );
        assert_eq!(
            gate.after_buffer_append(start + OUTPUT_READY_EMIT_MIN_INTERVAL / 2),
            OutputReadyEmitAction::Suppress
        );
        assert!(gate.finish_delayed_emit(true, start + OUTPUT_READY_EMIT_MIN_INTERVAL));
    }

    #[test]
    fn antigravity_completion_gate_emits_once_for_the_ready_prompt() {
        let mut gate = AntigravityTurnCompletionGate::default();

        assert!(!gate.observe_output(
            "antigravity",
            "Processing...",
            "Running the synchronization script...\r\n",
        ));
        assert!(gate.observe_output(
            "antigravity",
            "Processing...",
            "\r\n>\r\n? for shortcuts\r\n",
        ));
        assert!(!gate.observe_output("antigravity", "Idle", "\r\n>\r\n? for shortcuts\r\n",));
    }

    #[test]
    fn antigravity_completion_gate_ignores_ready_prompt_before_processing() {
        let mut gate = AntigravityTurnCompletionGate::default();

        assert!(!gate.observe_output("antigravity", "Idle", "\r\n>\r\n? for shortcuts\r\n",));
    }

    #[test]
    fn antigravity_workspace_trust_auto_confirmation_is_exact_and_one_shot() {
        let prompt = "Do you trust the contents of this project?\nAntigravity CLI requires permission to read, edit, and execute files here.";
        assert!(should_auto_confirm_antigravity_workspace_trust(
            "antigravity",
            true,
            false,
            prompt,
        ));
        assert!(!should_auto_confirm_antigravity_workspace_trust(
            "antigravity",
            false,
            false,
            prompt,
        ));
        assert!(!should_auto_confirm_antigravity_workspace_trust(
            "antigravity",
            true,
            true,
            prompt,
        ));
        assert!(!should_auto_confirm_antigravity_workspace_trust(
            "antigravity",
            true,
            false,
            "Requesting permission for: run_shell_command",
        ));
    }

    #[test]
    fn claude_bypass_permissions_consent_auto_confirmation_is_exact_and_one_shot() {
        let prompt = "WARNING: Claude Code running in Bypass Permissions mode\nBy proceeding, you accept all responsibility for actions taken while running in Bypass Permissions mode.\nNo, exit\nYes, I accept";
        assert!(should_auto_confirm_claude_bypass_permissions(
            "claude", true, false, prompt,
        ));
        assert!(!should_auto_confirm_claude_bypass_permissions(
            "claude", false, false, prompt,
        ));
        assert!(!should_auto_confirm_claude_bypass_permissions(
            "claude", true, true, prompt,
        ));
        assert!(!should_auto_confirm_claude_bypass_permissions(
            "antigravity",
            true,
            false,
            prompt,
        ));
        assert!(!should_auto_confirm_claude_bypass_permissions(
            "claude",
            true,
            false,
            "Allow Bash command? Yes / No",
        ));
    }

    #[test]
    fn claude_startup_prompt_rejects_pending_remote_connection() {
        use crate::control::provider_output_has_startup_ready_prompt as ready;
        assert!(!ready(
            "claude",
            "Claude Code v2.1.263\n❯ Try ask Claude\nshift+tab to cycle · /rc connecting…",
        ));
        assert!(!ready(
            "claude",
            "https://claude.ai/code/session_01ABC?from=cli /rc"
        ));
        assert!(ready(
            "claude",
            "Claude Code v2.1.263\n❯ Try ask Claude\nshift+tab to cycle · /rc",
        ));
    }

    #[test]
    fn claude_startup_readiness_uses_canonical_screen_after_partial_repaint() {
        let partial_repaint = "\x1b[4;1H\x1b[2KHaiku 4.5 | workspace | /rc";
        let canonical_screen = "Claude Code v2.1.270\n❯ Try fix typecheck errors\n────────\nHaiku 4.5 | workspace | /rc\n⏵⏵ bypass permissions on (shift+tab to cycle)";

        assert!(!startup_prompt_is_ready(
            "claude",
            true,
            Some(partial_repaint),
        ));
        assert!(startup_prompt_is_ready(
            "claude",
            true,
            Some(canonical_screen),
        ));
        assert!(!startup_prompt_is_ready(
            "claude",
            false,
            Some(canonical_screen),
        ));
    }

    #[tokio::test]
    async fn startup_readiness_tracks_canonical_screen_across_partial_repaints() {
        let broker =
            std::sync::Arc::new(crate::state::terminal_session::TerminalSessionBroker::default());
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(1);
        let generation = broker
            .start_or_replace_runtime(
                "startup",
                crate::state::terminal_session::TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                wardian_core::models::TerminalGeometry {
                    cols: 120,
                    rows: 24,
                },
            )
            .await
            .unwrap();
        for (provider, repaint, expected_ready) in [
            ("codex", "│ model: loading │\r\nResuming session…\r\n›", false),
            ("codex", "\x1b[1;10Hgpt-5.4-mini\x1b[K\x1b[2;1H\x1b[2K", true),
            ("claude", "\x1b[2J\x1b[HClaude Code v2.1.263\r\n❯ Try fix typecheck errors", false),
            ("claude", "\r\n────────\r\nHaiku 4.5 | workspace | /rc connecting…\r\n⏵⏵ bypass permissions on (shift+tab to cycle)", false),
            ("claude", "\x1b[4;1H\x1b[2KHaiku 4.5 | workspace | /rc", true),
            ("pi", "\x1b[2J\x1b[Hpi v0.84.2\r\n────────────────\r\n<workspace-root>/habitat/workspace (test/provider-conformanc...\r\n$0.000 (sub) 0.0%/272k (auto) (openai-codex) gpt-5.4-mini • medium", true),
            ("pi", "\x1b[2J\x1b[Hpi v0.84.2\r\n────────────────\r\n<workspace-root>/habitat/workspace\r\nLoading model…", false),
            ("opencode", "\x1b[2J\x1b[HLoading session...\r\nAsk anything...\r\nBuild  mimo-v2.5-free\r\nctrl+p commands", false),
            ("opencode", "\x1b[1;1H\x1b[2K", true),
            ("opencode", "\x1b[1;1HPermission required", false),
            ("opencode", "\x1b[1;1H\x1b[2K", true),
        ] {
            let output_broker = broker.clone();
            tokio::task::spawn_blocking(move || {
                output_broker.process_output_blocking("startup", generation, repaint.as_bytes().to_vec())
            }).await.unwrap().unwrap();
            let screen = broker.snapshot("startup").await.unwrap();
            assert_eq!(
                crate::control::provider_output_has_startup_ready_prompt(provider, &screen.visible_grid),
                expected_ready,
                "{provider}: {}", screen.visible_grid,
            );
        }
        assert!(
            input_rx.try_recv().is_err(),
            "observing startup never submits input"
        );
    }

    #[test]
    fn antigravity_user_turn_receipt_tracker_skips_restored_history_and_deduplicates() {
        let mut tracker = AntigravityUserTurnReceiptTracker::default();

        assert!(!tracker.observe(Some(8), true));
        assert!(!tracker.observe(Some(8), true));
        assert!(tracker.observe(Some(12), true));
        assert!(!tracker.observe(Some(12), true));
    }

    #[test]
    fn antigravity_user_turn_receipt_tracker_accepts_first_fresh_step() {
        let mut tracker = AntigravityUserTurnReceiptTracker::default();

        assert!(!tracker.observe(None, false));
        assert!(tracker.observe(Some(4), false));
        assert!(!tracker.observe(Some(4), false));
    }

    fn antigravity_message(
        step_index: u64,
        role: AgentChatRole,
        text: &str,
    ) -> AntigravityConversationMessage {
        AntigravityConversationMessage {
            step_index,
            source: None,
            role,
            text: text.to_string(),
        }
    }

    #[test]
    fn antigravity_transcript_tracker_projects_fresh_messages_and_changed_steps_once() {
        let mut tracker = AntigravityTranscriptTracker::default();
        let initial = vec![
            antigravity_message(2, AgentChatRole::User, "Run the check."),
            antigravity_message(3, AgentChatRole::Assistant, "Working."),
        ];

        let projected = tracker.observe(&initial, false);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].role, "user");
        assert_eq!(projected[1].text, "Working.");
        assert!(tracker.observe(&initial, false).is_empty());

        let changed = vec![
            initial[0].clone(),
            antigravity_message(3, AgentChatRole::Assistant, "Finished."),
        ];
        let projected = tracker.observe(&changed, false);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].text, "Finished.");
        assert_eq!(projected[0].source.as_deref(), Some("antigravity_sqlite"));
    }

    #[test]
    fn antigravity_transcript_tracker_positions_restored_history_before_projecting_new_rows() {
        let mut tracker = AntigravityTranscriptTracker::default();
        let history = vec![antigravity_message(
            8,
            AgentChatRole::Assistant,
            "Historical answer.",
        )];

        assert!(tracker.observe(&history, true).is_empty());
        let current = vec![
            history[0].clone(),
            antigravity_message(9, AgentChatRole::User, "New request."),
        ];
        let projected = tracker.observe(&current, true);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].text, "New request.");
    }

    #[test]
    fn antigravity_transcript_tracker_bounds_retained_history_to_the_overlap() {
        let mut tracker = AntigravityTranscriptTracker::default();
        let messages = (0..64)
            .map(|index| antigravity_message(index, AgentChatRole::Assistant, "progress"))
            .collect::<Vec<_>>();

        tracker.observe(&messages, false);

        assert_eq!(tracker.latest_step_index, Some(63));
        assert_eq!(tracker.minimum_step_index(), Some(47));
        assert!(tracker
            .observed_text
            .keys()
            .all(|(step_index, _)| *step_index >= 47));
        assert!(tracker.observed_text.len() <= 17);
    }

    #[test]
    fn antigravity_database_watermark_changes_with_database_or_wal_content() {
        let temp = tempfile::tempdir().expect("temp dir");
        let database = temp.path().join("conversation.db");
        let wal = temp.path().join("conversation.db-wal");
        std::fs::write(&database, b"database").expect("write database");

        let initial = antigravity_database_watermark(&database).expect("initial watermark");
        assert_eq!(
            antigravity_database_watermark(&database),
            Some(initial.clone())
        );

        std::fs::write(&wal, b"wal").expect("write wal");
        let with_wal = antigravity_database_watermark(&database).expect("wal watermark");
        assert_ne!(with_wal, initial);

        std::fs::write(&database, b"database-expanded").expect("update database");
        let expanded = antigravity_database_watermark(&database).expect("expanded watermark");
        assert_ne!(expanded, with_wal);
    }

    #[test]
    fn antigravity_fresh_launch_ignores_preexisting_workspace_mapping() {
        let (conversation_id, capture_identity) =
            antigravity_watcher_conversation(None, Some("conversation-123"), || {
                Some("conversation-123".to_string())
            });

        assert_eq!(conversation_id, None);
        assert!(!capture_identity);
    }

    #[test]
    fn antigravity_restored_identity_skips_conversation_discovery() {
        let discovery_called = std::cell::Cell::new(false);

        let (conversation_id, capture_identity) = antigravity_watcher_conversation(
            Some("restored-conversation".to_string()),
            None,
            || {
                discovery_called.set(true);
                Some("different-conversation".to_string())
            },
        );

        assert_eq!(conversation_id.as_deref(), Some("restored-conversation"));
        assert!(!capture_identity);
        assert!(!discovery_called.get());
    }

    #[test]
    fn opencode_composer_readiness_records_receipt_and_enables_resume_delta() {
        let temp = tempfile::tempdir().unwrap();
        let store = wardian_core::memory::MemoryStore::open(temp.path().join("memory.db")).unwrap();
        let agent_id = "opencode-memory-agent";
        let workspace = temp.path().to_string_lossy().to_string();
        let process_key = "opencode-provider-session";
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: agent_id.into(),
                    workspace: Some(workspace.clone()),
                    kind: wardian_core::memory::MemoryKind::Stable,
                    text: "Initial OpenCode memory".into(),
                    evidence_excerpt: "Established before interactive startup.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let brief = store
            .compile_brief(
                &wardian_core::memory::MemoryActor::agent(agent_id),
                agent_id,
                Some(&workspace),
                "opencode",
                process_key,
                false,
                8_000,
            )
            .unwrap();
        let mut pending = Some((store, brief, workspace.clone(), process_key.into()));

        let title_event = "\u{1b}]0;OpenCode\u{7}";
        let title = extract_terminal_titles(title_event)
            .into_iter()
            .last()
            .expect("OpenCode title");
        assert!(!crate::control::provider_output_has_startup_ready_prompt(
            "opencode", &title
        ));
        assert!(pending.is_some());
        assert!(crate::control::provider_output_has_startup_ready_prompt(
            "opencode",
            "Ask anything...\nBuild  mimo-v2.5-free\nctrl+p commands",
        ));
        assert!(record_pending_memory_injection(
            &mut pending,
            agent_id,
            "opencode"
        ));
        assert!(!record_pending_memory_injection(
            &mut pending,
            agent_id,
            "opencode"
        ));
        assert!(pending.is_none());

        let store = wardian_core::memory::MemoryStore::open(temp.path().join("memory.db")).unwrap();
        assert_eq!(
            store
                .list_events(&wardian_core::memory::MemoryActor::Operator, agent_id,)
                .unwrap()
                .into_iter()
                .filter(|event| event.action == "loaded")
                .count(),
            1
        );
        store
            .save(
                &wardian_core::memory::MemoryActor::Operator,
                wardian_core::memory::SaveMemoryRequest {
                    agent_id: agent_id.into(),
                    workspace: Some(workspace.clone()),
                    kind: wardian_core::memory::MemoryKind::Current,
                    text: "Later OpenCode memory".into(),
                    evidence_excerpt: "Established after the startup receipt.".into(),
                    sources: vec![],
                    idempotency_key: None,
                },
            )
            .unwrap();
        let resumed = store
            .compile_brief(
                &wardian_core::memory::MemoryActor::agent(agent_id),
                agent_id,
                Some(&workspace),
                "opencode",
                process_key,
                true,
                8_000,
            )
            .unwrap();
        assert_eq!(
            resumed.kind,
            wardian_core::memory::MemoryBriefKind::ResumeDelta
        );
        assert!(resumed.context_text.contains("Later OpenCode memory"));
        assert!(!resumed.context_text.contains("Initial OpenCode memory"));
    }
}
