//! Interactive Claude delivery through the bundled generation-owned mod.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Manager};
use wardian_core::claude_mod::{ClaudeModLaunchPlan, ClaudeModResolver, FallbackReason, ModAsset};
use wardian_core::control::{DeliveryDetail, DeliveryTransportKind};
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind, AgentChatRole};
use wardian_core::models::chat_presentation::ChatSubmissionStatus;
use wardian_core::models::AgentConfig;
use wardian_core::native_transport::{
    NativeDeliveryPhase, NativeDeliveryRecord, NativeMessageEnvelope, NativeMessageOperation,
};

use super::live_surface::{
    LiveSurfaceDeliveryError, LiveSurfacePromptRequest, LiveSurfacePromptResult,
};
use crate::state::AppState;

type Resolver = Arc<tokio::sync::Mutex<ClaudeModResolver>>;

/// Materialize immutable bundled assets before the generation's process exists.
pub(crate) fn materialize_launch(config: &AgentConfig, generation: u64) -> Result<PathBuf, String> {
    let home = crate::utils::fs::get_wardian_home().ok_or("Claude mod home unavailable")?;
    let expected = crate::manager::session_identity::expected_caller_owned_identity(config)
        .ok_or("Claude launch has no expected provider session")?;
    let plan = ClaudeModLaunchPlan::new(&home, &config.session_id, generation, expected)
        .map_err(|error| error.to_string())?;
    plan.materialize(&[
        ModAsset {
            relative_path: ".claude-plugin/plugin.json",
            contents: PLUGIN.as_bytes(),
        },
        ModAsset {
            relative_path: "hooks/hooks.json",
            contents: HOOKS.as_bytes(),
        },
        ModAsset {
            relative_path: "hooks/register.ts",
            contents: REGISTER.as_bytes(),
        },
    ])
    .map_err(|error| error.to_string())?;
    Ok(plan.mod_dir())
}

fn resolver(agent_id: &str, generation: u64) -> Result<Resolver, String> {
    static RESOLVERS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Resolver>>> = OnceLock::new();
    let inbox = generation_root(agent_id, generation)?.join("inbox");
    let mut resolvers = RESOLVERS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Claude mod resolver lock poisoned")?;
    if let Some(resolver) = resolvers.get(&inbox) {
        return Ok(resolver.clone());
    }
    let resolver = Arc::new(tokio::sync::Mutex::new(
        ClaudeModResolver::open(&inbox).map_err(|error| error.to_string())?,
    ));
    resolvers.insert(inbox, resolver.clone());
    Ok(resolver)
}

fn before_submit(message: impl Into<String>) -> LiveSurfaceDeliveryError {
    LiveSurfaceDeliveryError {
        message: message.into(),
        detail: None,
        retry_safe: true,
    }
}

/// Returns None only before publication, when terminal fallback is still safe.
/// No terminal sink is constructed or invoked once the inbox owns this delivery.
pub(crate) async fn try_deliver(
    app: Option<&AppHandle>,
    state: &AppState,
    request: &LiveSurfacePromptRequest,
    config: &AgentConfig,
    interaction_id: &str,
    generation: u64,
    authority: Option<&crate::remote::client_messages::ClientSendAuthority>,
) -> Result<Result<LiveSurfacePromptResult, LiveSurfaceDeliveryError>, FallbackReason> {
    let as_user = is_composer_delivery("claude", request);
    // Ownership precedes readiness. A stale heartbeat must never reroute a
    // delivery already published to a still-possibly-live older generation.
    match wardian_core::db::native_delivery(interaction_id) {
        Ok(Some(record)) if record.transport == "claude_mod" => {
            if record.phase != NativeDeliveryPhase::FailedBeforeSubmit {
                return Ok(Err(LiveSurfaceDeliveryError {
                    message:
                        "Claude mod already owns this delivery; reconcile the original generation"
                            .into(),
                    detail: Some(delivery_detail(request, config, &record)),
                    retry_safe: false,
                }));
            }
            if record.envelope.generation == generation {
                return Err(FallbackReason::ModNotLoaded);
            }
        }
        Err(error) => return Ok(Err(before_submit(error.to_string()))),
        _ => {}
    }
    // Information/reply delivery releases lifecycle ownership before coming here.
    // Tasks already hold it. Match the existing fenced-terminal admission rules.
    let _lifecycle = if request.write_fence.is_some() {
        Some(state.lock_agent_lifecycle(&request.session_id).await)
    } else {
        None
    };
    let resolver =
        resolver(&request.session_id, generation).map_err(|_| FallbackReason::ModNotLoaded)?;
    let mut resolver = resolver.lock().await;
    let tracked: BTreeSet<_> = tracked_sessions(config).into_iter().collect();
    resolver
        .readiness(chrono::Utc::now(), &tracked, as_user)
        .map_err(|_| FallbackReason::ModNotLoaded)??;
    if let Some(fence) = &request.write_fence {
        let agents = state.agents.lock().await;
        let Some(agent) = agents.get(&request.session_id) else {
            return Ok(Err(before_submit("Claude mod target deleted")));
        };
        if !Arc::ptr_eq(&agent.config, &fence.agent_config_identity)
            || agent.runtime_generation != Some(generation)
        {
            return Ok(Err(before_submit("Claude mod target generation changed")));
        }
        let current = agent
            .config
            .lock()
            .map_err(|_| FallbackReason::SessionMismatch)?;
        if current.is_off
            || current.provider != "claude"
            || current.resume_session.as_deref() != Some(fence.provider_conversation_id.as_str())
        {
            return Ok(Err(before_submit("Claude mod target conversation changed")));
        }
    }
    let input_generation = state
        .interactions
        .current_provider_input_generation(&request.session_id)
        .await
        .unwrap_or(0);
    let Some(_generation_fence) = state
        .interactions
        .fence_provider_input_generation(&request.session_id, input_generation)
        .await
    else {
        return Ok(Err(before_submit(
            "Claude mod provider generation unavailable",
        )));
    };
    if request
        .write_fence
        .as_ref()
        .is_some_and(|fence| fence.provider_input_generation != input_generation)
    {
        return Ok(Err(before_submit("Claude mod provider generation changed")));
    }
    let conversation = crate::manager::session_identity::expected_caller_owned_identity(config)
        .unwrap_or_default();
    let admission = crate::state::terminal_session::AutomatedConversationLeaseAdmission::new(
        &request.session_id,
        "claude",
        conversation,
    );
    let mut lease = match admission.acquire_with_handoff_hook(|| {}).await {
        Ok(lease) => lease,
        Err(error) => return Ok(Err(before_submit(error.to_string()))),
    };
    let existing = match wardian_core::db::native_delivery(interaction_id) {
        Ok(existing) => existing,
        Err(error) => return Ok(Err(before_submit(error.to_string()))),
    };
    let record = if let Some(record) =
        existing.filter(|record| record.phase != NativeDeliveryPhase::FailedBeforeSubmit)
    {
        // A previous publication never becomes a fresh terminal/mod submission.
        let _ = lease.release();
        return Ok(Err(LiveSurfaceDeliveryError {
            message: format!("Claude delivery already owns phase {:?}", record.phase),
            detail: None,
            retry_safe: false,
        }));
    } else {
        let now = chrono::Utc::now().to_rfc3339();
        use sha2::{Digest, Sha256};
        NativeDeliveryRecord {
            envelope: NativeMessageEnvelope {
                interaction_id: interaction_id.into(),
                message_id: request
                    .delivery_message_id
                    .clone()
                    .unwrap_or_else(|| interaction_id.into()),
                target_agent_id: request.session_id.clone(),
                sender_agent_id: request.origin.as_ref().map(|origin| match origin {
                    wardian_core::control::MessageOrigin::WardianAgent { session_id } => {
                        session_id.clone()
                    }
                }),
                parent_interaction_id: state
                    .interactions
                    .interaction(interaction_id)
                    .await
                    .and_then(|record| record.parent_interaction_id),
                caller_idempotency_key: None,
                generation,
                operation: NativeMessageOperation::StartTurn,
                deadline_at: None,
                body: request.prompt.clone(),
            },
            canonical_hash: format!("{:x}", Sha256::digest(request.prompt.as_bytes())),
            provider: "claude".into(),
            transport: "claude_mod".into(),
            claude_mod_as_user: as_user,
            phase: NativeDeliveryPhase::SubmittedUnconfirmed,
            provider_request_id: None,
            provider_turn_id: None,
            detail: Some("waiting on the Claude delivery mod; age=0s".into()),
            created_at: now.clone(),
            updated_at: now,
        }
    };
    let remote_permit = if let Some(authority) = authority {
        match authority.acquire().await {
            Ok(permit) => Some(permit),
            Err(code) => {
                let _ = lease.release();
                return Ok(Err(before_submit(code)));
            }
        }
    } else {
        None
    };
    if let Err(error) = wardian_core::db::upsert_native_delivery(&record) {
        let _ = lease.release();
        return Ok(Err(before_submit(error.to_string())));
    }
    // Sequence allocation is serialized by lifecycle ownership. Fixed width
    // filenames preserve order under the mod's lexical sort across restarts.
    let seq = std::fs::read_dir(resolver.config().inbox.join("pending"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .split('-')
                .next()?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let tagged_prompt;
    let text = if as_user {
        tagged_prompt = format!(
            "{}\n{}",
            record.envelope.transcript_header(),
            request.prompt
        );
        &tagged_prompt
    } else {
        &request.prompt
    };
    let publication = resolver.publish(seq, interaction_id, text, as_user);
    drop(remote_permit);
    let _ = lease.release();
    drop(_generation_fence);
    drop(_lifecycle);
    drop(resolver);
    let mut detail = delivery_detail(request, config, &record);
    if let Err(error) = publication {
        detail.reason = Some(format!("Claude inbox publication uncertain: {error}"));
    }
    if let Err(error) = persist_detail(state, interaction_id, input_generation, &detail).await {
        return Ok(Err(LiveSurfaceDeliveryError {
            message: error,
            detail: Some(detail),
            retry_safe: false,
        }));
    }
    crate::control::push_delivery_for_delivery_service(state, &request.session_id, &detail).await;
    // Continue reconciliation after a timeout or a generation change. No new
    // transport is selected by this observer.
    if let Some(app) = app {
        observe(
            app.clone(),
            config.clone(),
            interaction_id.into(),
            generation,
            input_generation,
        );
    }
    let started = std::time::Instant::now();
    loop {
        if let Ok(current) = reconcile(config, interaction_id, generation).await {
            if current.phase != NativeDeliveryPhase::SubmittedUnconfirmed {
                detail = delivery_detail(request, config, &current);
                if matches!(
                    current.phase,
                    NativeDeliveryPhase::ProviderAccepted
                        | NativeDeliveryPhase::TurnStarted
                        | NativeDeliveryPhase::Completed
                ) {
                    return Ok(Ok(LiveSurfacePromptResult { detail }));
                }
                return Ok(Err(LiveSurfaceDeliveryError {
                    message: current
                        .detail
                        .unwrap_or_else(|| "Claude mod refused delivery".into()),
                    detail: Some(detail),
                    retry_safe: current.phase == NativeDeliveryPhase::FailedBeforeSubmit,
                }));
            }
        }
        if started.elapsed() >= std::time::Duration::from_secs(10) {
            return Ok(Err(LiveSurfaceDeliveryError {
                message:
                    "waiting on the Claude delivery mod; delivery remains owned by its generation"
                        .into(),
                detail: Some(detail),
                retry_safe: false,
            }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// A composer send with terminal-only input must use the existing input sink.
pub(crate) fn is_composer_delivery(provider: &str, request: &LiveSurfacePromptRequest) -> bool {
    provider == "claude"
        && request.chat_composer
        && request.input_mode == wardian_core::control::MessageInputMode::Message
        && request.approval_action.is_none()
        && !request
            .prompt
            .chars()
            .any(|character| character.is_ascii_control() && !matches!(character, '\t' | '\r' | '\n'))
        // The mod submits literal text and Claude expands @file only in the TUI.
        && !request
            .prompt
            .split_whitespace()
            .any(|word| word.starts_with('@') && word.len() > 1)
}

/// Peer messages and automatic follow-ups keep the framed mod delivery.
pub(crate) fn is_peer_delivery(provider: &str, request: &LiveSurfacePromptRequest) -> bool {
    provider == "claude"
        && matches!(
            request.input_mode,
            wardian_core::control::MessageInputMode::Message
        )
        && (request.origin.is_some() || request.runtime_state.starts_with("agent_messaging_"))
}

const PLUGIN: &str = include_str!("../../resources/claude-delivery/.claude-plugin/plugin.json");
const HOOKS: &str = include_str!("../../resources/claude-delivery/hooks/hooks.json");
const REGISTER: &str = include_str!("../../resources/claude-delivery/hooks/register.ts");

fn generation_root(agent_id: &str, generation: u64) -> Result<PathBuf, String> {
    if agent_id.is_empty()
        || !agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        || generation == 0
    {
        return Err("Invalid Claude mod generation identity".into());
    }
    crate::utils::fs::get_wardian_home()
        .map(|home| {
            home.join("agents")
                .join(agent_id)
                .join("claude")
                .join("delivery")
                .join(generation.to_string())
        })
        .ok_or_else(|| "Claude mod home unavailable".into())
}

/// Include provider sessions already attributed by Wardian, independently of
/// the mod's session history. A new heartbeat session alone cannot grant admission.
fn tracked_sessions(config: &AgentConfig) -> HashSet<String> {
    let mut sessions: HashSet<_> = crate::manager::claude::claude_accepted_sessions(config)
        .into_iter()
        .collect();
    let observed = wardian_core::db::get_db_conn(|conn| {
        let mut statement = conn.prepare(
            "SELECT DISTINCT provider_session_id FROM telemetry_sources WHERE session_id=?1 AND provider='claude' AND provider_session_id IS NOT NULL",
        )?;
        let rows = statement.query_map([&config.session_id], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    });
    if let Ok(observed) = observed {
        sessions.extend(observed);
    }
    sessions
}

fn transcript_paths(config: &AgentConfig, sessions: &HashSet<String>) -> Vec<PathBuf> {
    let cwd = crate::utils::fs::resolve_cwd(&config.folder, &config.session_id);
    let project = crate::manager::claude::claude_project_dir_name(&cwd.to_string_lossy());
    let Some(projects) = dirs::home_dir().map(|home| home.join(".claude").join("projects")) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for session in sessions {
        if !session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            continue;
        }
        let name = format!("{session}.jsonl");
        let primary = projects.join(&project).join(&name);
        if primary.is_file() {
            paths.push(primary);
        } else if let Ok(directories) = std::fs::read_dir(&projects) {
            for directory in directories.flatten() {
                let path = directory.path().join(&name);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

fn delivery_detail(
    request: &LiveSurfacePromptRequest,
    config: &AgentConfig,
    record: &NativeDeliveryRecord,
) -> DeliveryDetail {
    let (state, phase) = phase_names(record.phase);
    DeliveryDetail {
        uuid: request.session_id.clone(),
        name: config.session_name.clone(),
        provider: "claude".into(),
        runtime_state: request.runtime_state.into(),
        delivery_state: state.into(),
        input_mode: request.input_mode,
        queue_policy: request.queue_policy,
        message_id: Some(record.envelope.message_id.clone()),
        delivery_phase: Some(phase.into()),
        observed_state: Some(format!(
            "claude_mod;runtime_generation={}{}",
            record.envelope.generation,
            if record.claude_mod_as_user {
                ";as_user=true"
            } else {
                ""
            }
        )),
        reason: record.detail.clone(),
        profile: Some("claude".into()),
        error: None,
    }
}

fn phase_names(phase: NativeDeliveryPhase) -> (&'static str, &'static str) {
    match phase {
        NativeDeliveryPhase::ProviderAccepted => ("provider_accepted", "provider_accepted"),
        NativeDeliveryPhase::TurnStarted => ("provider_accepted", "turn_started"),
        NativeDeliveryPhase::FailedBeforeSubmit => ("failed", "failed_before_submit"),
        NativeDeliveryPhase::Failed => ("failed", "failed"),
        _ => ("submit_sent_unconfirmed", "submitted_unconfirmed"),
    }
}

async fn persist_detail(
    state: &AppState,
    id: &str,
    input_generation: u64,
    detail: &DeliveryDetail,
) -> Result<(), String> {
    state
        .interactions
        .record_delivery_attempt_durable(
            id,
            &detail.uuid,
            DeliveryTransportKind::ProviderPlugin,
            input_generation,
            &detail.runtime_state,
            &detail.delivery_state,
            detail.delivery_phase.clone(),
            detail.observed_state.clone(),
            detail.reason.clone(),
            detail.error.clone(),
        )
        .await
        .map(|_| ())
}

pub(crate) const PROMPT_STATUS: &str = "_wardian_claude_prompt_status";
pub(crate) const PROMPT_DELIVERY: &str = "_wardian_claude_prompt_delivery";

/// A private, bounded page or capture lookup. Reading the optional badge cannot
/// wait on an interaction writer or hold Wardian's global database mutex.
#[derive(Default)]
pub(crate) struct ChatPromptReceipts(std::cell::OnceCell<Option<rusqlite::Connection>>);

impl ChatPromptReceipts {
    fn connection(&self) -> Option<&rusqlite::Connection> {
        self.0
            .get_or_init(|| {
                let connection = rusqlite::Connection::open_with_flags(
                    wardian_core::paths::state_db_path()?,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .ok()?;
                connection.busy_timeout(std::time::Duration::ZERO).ok()?;
                Some(connection)
            })
            .as_ref()
    }

    /// Recover a terminal state whose durable delivery update raced capture.
    /// Nonterminal deliveries keep the window eligible for receipt polling.
    pub(crate) fn terminal_status(
        &self,
        event: &AgentChatEvent,
    ) -> io::Result<(Option<ChatSubmissionStatus>, bool)> {
        let Some(binding) = event.metadata[PROMPT_DELIVERY].as_str() else {
            return Ok((None, false));
        };
        let (generation, interaction) = binding
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid archived prompt receipt selector"))?;
        let phase = wardian_core::db::claude_composer_delivery_phase(
            self.connection()
                .ok_or_else(|| io::Error::other("prompt receipt database unavailable"))?,
            &event.session_id,
            interaction,
            generation.parse().map_err(io::Error::other)?,
        )
        .map_err(io::Error::other)?;
        Ok(match phase.as_deref() {
            Some("failed") => (Some(ChatSubmissionStatus::Failed), false),
            Some("cancelled") => (Some(ChatSubmissionStatus::Cancelled), false),
            Some(
                "queued"
                | "dispatching"
                | "submitted_unconfirmed"
                | "provider_accepted"
                | "turn_started"
                | "cancel_requested",
            ) => (None, true),
            _ => (None, false),
        })
    }

    /// Keep proof on this source event only. The archived identity allows a
    /// terminal delivery outcome to follow the prompt through capture or restart.
    pub(crate) fn annotate_owned_prompt(
        &self,
        event: &mut AgentChatEvent,
        provider_session: &str,
        raw: &str,
        provisional: bool,
    ) {
        if event.provider != "claude"
            || event.kind != AgentChatEventKind::Message
            || event.role != Some(AgentChatRole::User)
        {
            return;
        }
        let Some((delivery, status)) =
            self.transcript_prompt_status(&event.session_id, provider_session, raw)
        else {
            return;
        };
        if provisional
            || matches!(
                status,
                ChatSubmissionStatus::Failed | ChatSubmissionStatus::Cancelled
            )
        {
            event.metadata[PROMPT_STATUS] = serde_json::json!(status);
        }
        if !provisional {
            event.metadata[PROMPT_DELIVERY] = serde_json::json!(delivery);
        }
    }

    /// Verify a token from an already owned native row. Lookups are keyed by the
    /// untrusted selector, then fenced by the full envelope and its mod generation.
    fn transcript_prompt_status(
        &self,
        agent_id: &str,
        provider_session: &str,
        raw: &str,
    ) -> Option<(String, ChatSubmissionStatus)> {
        if !raw.contains("[Wardian message_id=") {
            return None;
        }
        let row = serde_json::from_str(raw).ok()?;
        let (interaction, header) =
            wardian_core::claude_mod::composer_transcript_token(&row, provider_session)?;
        let record = wardian_core::db::native_delivery_with_conn(self.connection()?, &interaction)
            .ok()??;
        if record.provider != "claude"
            || record.transport != "claude_mod"
            || !record.claude_mod_as_user
            || record.envelope.interaction_id != interaction
            || record.envelope.target_agent_id != agent_id
            || record.envelope.transcript_header() != header
        {
            return None;
        }
        let inbox = generation_root(agent_id, record.envelope.generation)
            .ok()?
            .join("inbox");
        let resolver = ClaudeModResolver::open(&inbox).ok()?;
        if !resolver
            .has_composer_source_claim(&interaction, provider_session)
            .ok()?
        {
            return None;
        }
        let status = match record.phase {
            NativeDeliveryPhase::Failed => ChatSubmissionStatus::Failed,
            NativeDeliveryPhase::Cancelled => ChatSubmissionStatus::Cancelled,
            NativeDeliveryPhase::Completed | NativeDeliveryPhase::TurnStarted => {
                ChatSubmissionStatus::Delivered
            }
            _ => match resolver.project(&interaction).ok()?.phase {
                NativeDeliveryPhase::ProviderAccepted => ChatSubmissionStatus::Queued,
                NativeDeliveryPhase::Failed => ChatSubmissionStatus::Failed,
                // The source row proves that this delivery is already visible.
                _ => ChatSubmissionStatus::Delivered,
            },
        };
        Some((
            format!("{}:{interaction}", record.envelope.generation),
            status,
        ))
    }
}

fn transcript_contains(
    config: &AgentConfig,
    resolver: &ClaudeModResolver,
    record: &NativeDeliveryRecord,
) -> bool {
    let id = &record.envelope.interaction_id;
    let tracked = tracked_sessions(config).into_iter().collect();
    let Ok(sessions) = resolver.session_history(&tracked) else {
        return false;
    };
    for path in transcript_paths(config, &sessions.iter().cloned().collect()) {
        let Some(session) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains(id) {
                continue;
            }
            if let Ok(row) = serde_json::from_str(&line) {
                let matches = if record.claude_mod_as_user {
                    wardian_core::claude_mod::composer_transcript_row_matches(
                        &row,
                        session,
                        &sessions,
                        &record.envelope,
                    )
                } else {
                    wardian_core::claude_mod::transcript_row_matches(&row, session, &sessions, id)
                };
                if matches {
                    return true;
                }
            }
        }
    }
    false
}

/// Late provider evidence advances the existing attempt; it never publishes a
/// replacement pending file or chooses another transport.
async fn reconcile(
    config: &AgentConfig,
    id: &str,
    generation: u64,
) -> Result<NativeDeliveryRecord, String> {
    let resolver = resolver(&config.session_id, generation)?;
    let resolver = resolver.lock().await;
    let mut record = wardian_core::db::native_delivery(id)
        .map_err(|error| error.to_string())?
        .ok_or("Claude mod delivery record missing")?;
    if record.transport != "claude_mod"
        || record.envelope.generation != generation
        || record.phase.is_terminal()
    {
        // A live caller can settle after this observer first saw a terminal drop.
        // Reapply the terminal receipt so canonical uncertainty cannot latch.
        wardian_core::db::agent_messaging::reconcile_claude_mod_receipt(id)
            .map_err(|error| error.to_string())?;
        return Ok(record);
    }
    let projection = resolver.project(id).map_err(|error| error.to_string())?;
    let bound_claim = !record.claude_mod_as_user
        || resolver
            .has_bound_claim(id)
            .map_err(|error| error.to_string())?;
    let next = if bound_claim && transcript_contains(config, &resolver, &record) {
        NativeDeliveryPhase::TurnStarted
    } else {
        projection.phase
    };
    if record.phase.can_transition_to(next) {
        record.phase = next;
        record.detail = projection.detail.or_else(|| {
            (next == NativeDeliveryPhase::SubmittedUnconfirmed).then(|| {
                let age = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .map(|created| {
                        (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_seconds()
                    })
                    .unwrap_or(0);
                format!("waiting on the Claude delivery mod; age={age}s")
            })
        });
    }
    if record.phase == NativeDeliveryPhase::SubmittedUnconfirmed {
        if let Some(proof) = resolver
            .resolve_rejected(id)
            .map_err(|error| error.to_string())?
        {
            wardian_core::db::agent_messaging::resolve_claude_mod_non_submission(&proof)
                .map_err(|error| error.to_string())?;
            record.phase = record
                .phase
                .resolve_proven_non_submission(&proof)
                .expect("submitted phase checked");
        }
    }
    record.updated_at = chrono::Utc::now().to_rfc3339();
    wardian_core::db::upsert_native_delivery(&record).map_err(|error| error.to_string())?;
    wardian_core::db::agent_messaging::reconcile_claude_mod_receipt(id)
        .map_err(|error| error.to_string())?;
    Ok(record)
}

fn observe(
    app: AppHandle,
    config: AgentConfig,
    id: String,
    generation: u64,
    input_generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        let mut last = None;
        loop {
            let state = app.state::<AppState>();
            if let Ok(record) = reconcile(&config, &id, generation).await {
                if last != Some(record.phase) {
                    let mut request = LiveSurfacePromptRequest::message(&config.session_id, "");
                    request.runtime_state = "claude_delivery_mod";
                    let detail = delivery_detail(&request, &config, &record);
                    if persist_detail(&state, &id, input_generation, &detail)
                        .await
                        .is_err()
                    {
                        break;
                    }
                    crate::control::push_delivery_for_delivery_service(
                        &state,
                        &config.session_id,
                        &detail,
                    )
                    .await;
                    last = Some(record.phase);
                }
                if record.phase.is_terminal() || record.phase == NativeDeliveryPhase::TurnStarted {
                    break;
                }
            }
            let active = state
                .agents
                .lock()
                .await
                .get(&config.session_id)
                .is_some_and(|agent| agent.runtime_generation == Some(generation));
            if !active {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}

/// Called only with a receipt produced by the exact contained process stop.
#[cfg(windows)]
pub(crate) async fn resolve_generation_exit(
    config: &AgentConfig,
    generation: u64,
    exit: &impl wardian_core::claude_mod::GenerationExitVerifier,
) -> Result<(), String> {
    for record in outstanding_deliveries(&config.session_id, Some(generation))? {
        let id = &record.envelope.interaction_id;
        // The final transcript can become durable just before containment closes.
        // Reconcile it before deciding that a surviving claim stays uncertain.
        let _ = reconcile(config, id, generation).await?;
        let resolver = resolver(&config.session_id, generation)?;
        let resolver = resolver.lock().await;
        if let Some(proof) = resolver
            .resolve_after_verified_exit(id, exit)
            .map_err(|error| error.to_string())?
        {
            wardian_core::db::agent_messaging::resolve_claude_mod_non_submission(&proof)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// EOF is a trigger to verify the retained containment, never exit evidence in
/// itself. Detach handles under the lifecycle gate, then wait without a roster lock.
#[cfg(windows)]
pub(crate) async fn finish_after_eof(app: AppHandle, agent_id: String, generation: u64) {
    let state = app.state::<AppState>();
    let _lifecycle = state.lock_agent_lifecycle(&agent_id).await;
    let runtime = {
        let mut agents = state.agents.lock().await;
        let Some(agent) = agents
            .get_mut(&agent_id)
            .filter(|agent| agent.runtime_generation == Some(generation))
        else {
            return;
        };
        if !agent.background_processes.is_empty()
            || agent.child_process.is_none()
            || !agent
                .job_object
                .as_ref()
                .is_some_and(|job| job.contained_from_launch())
        {
            return;
        }
        let Ok(config) = agent.config.lock().map(|config| config.clone()) else {
            return;
        };
        agent.process_id = None;
        Some((
            config,
            agent.job_object.take().expect("job checked"),
            agent.child_process.take().expect("child checked"),
        ))
    };
    if let Some((config, job, mut child)) = runtime {
        match job
            .stop_and_join_for_claude_mod(&agent_id, generation, child.as_mut())
            .await
        {
            Ok(exit) => {
                if let Err(error) = resolve_generation_exit(&config, generation, &exit).await {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Claude mod exit reconciliation failed: {error}"
                    ));
                }
            }
            Err(error) => crate::utils::logging::log_debug(&format!(
                "[Wardian] Claude mod generation exit remains unproven: {error}"
            )),
        }
    }
}

/// Reconcile surviving generation records after restoration, including a claimed
/// prompt whose only positive evidence was persisted in a later Claude session.
pub(crate) fn resume_observers(app: &AppHandle, config: &AgentConfig, input_generation: u64) {
    for record in outstanding_deliveries(&config.session_id, None).unwrap_or_default() {
        observe(
            app.clone(),
            config.clone(),
            record.envelope.interaction_id,
            record.envelope.generation,
            input_generation,
        );
    }
}

fn outstanding_deliveries(
    agent_id: &str,
    generation: Option<u64>,
) -> Result<Vec<NativeDeliveryRecord>, String> {
    wardian_core::db::get_db_conn(|conn| {
        let mut statement = conn.prepare("SELECT record_json FROM native_deliveries WHERE target_agent_id=?1 AND (?2 IS NULL OR generation=?2) AND phase IN ('submitted_unconfirmed','provider_accepted')")?;
        let rows = statement.query_map(rusqlite::params![agent_id, generation], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for row in rows {
            let record: NativeDeliveryRecord = serde_json::from_str(&row?)?;
            if record.provider == "claude" && record.transport == "claude_mod" { records.push(record); }
        }
        Ok(records)
    }).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::control::{MessageInputMode, MessageOrigin};

    #[test]
    fn peer_messages_stay_on_the_existing_framed_mod_route() {
        let mut request = LiveSurfacePromptRequest::message("agent", "prompt");
        request.require_provider_turn_receipt = true;
        assert!(!is_peer_delivery("claude", &request));
        request.origin = Some(MessageOrigin::WardianAgent {
            session_id: "sender".into(),
        });
        assert!(is_peer_delivery("claude", &request));
        assert!(!is_peer_delivery("pi", &request));
        request.input_mode = MessageInputMode::Command;
        assert!(!is_peer_delivery("claude", &request));
        request.input_mode = MessageInputMode::ApprovalAction;
        assert!(!is_peer_delivery("claude", &request));
        request.origin = None;
        request.input_mode = MessageInputMode::Message;
        request.runtime_state = "agent_messaging_reply_continuation";
        assert!(is_peer_delivery("claude", &request));
    }

    #[test]
    fn composer_selects_the_mod_only_for_claude_prompt_input() {
        let mut request = LiveSurfacePromptRequest::message("agent", "Inspect the notes.");
        assert!(!is_composer_delivery("claude", &request));
        request.chat_composer = true;
        assert!(is_composer_delivery("claude", &request));
        assert!(!is_composer_delivery("codex", &request));
        assert!(!is_peer_delivery("claude", &request));

        request.prompt = "Inspect the notes.\n\nAttached files:\n- /workspace/notes.md".into();
        assert!(is_composer_delivery("claude", &request));
        request.prompt = "Inspect @/workspace/notes.md".into();
        assert!(!is_composer_delivery("claude", &request));
        request.prompt = "\u{1b}".into();
        assert!(!is_composer_delivery("claude", &request));
        request.prompt = "1".into();
        request.input_mode = MessageInputMode::ApprovalAction;
        assert!(!is_composer_delivery("claude", &request));
        request.input_mode = MessageInputMode::Command;
        assert!(!is_composer_delivery("claude", &request));
    }

    #[test]
    fn queued_receipt_does_not_claim_turn_started() {
        assert_eq!(
            phase_names(NativeDeliveryPhase::ProviderAccepted),
            ("provider_accepted", "provider_accepted")
        );
        assert_eq!(
            phase_names(NativeDeliveryPhase::TurnStarted),
            ("provider_accepted", "turn_started")
        );
        assert_eq!(
            phase_names(NativeDeliveryPhase::SubmittedUnconfirmed),
            ("submit_sent_unconfirmed", "submitted_unconfirmed")
        );
        assert_eq!(
            phase_names(NativeDeliveryPhase::Failed),
            ("failed", "failed")
        );
    }
}
