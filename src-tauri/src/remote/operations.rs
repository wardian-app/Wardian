use crate::remote::models::{
    RemoteAgentActionRequest, RemoteAgentSummary, RemoteAutomationMonitorRun,
    RemoteAutomationMonitorSchedule, RemoteAutomationMonitorSnapshot, RemoteInboxActionRequest,
    RemoteTerminalSnapshot, RemoteWatchlistResponse,
};
use crate::state::AppState;
use crate::utils::strip_ansi_controls;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use wardian_core::control::{
    ControlRequest, InboxListResponse, InboxNotificationKind, InteractionStatus, MessageInputMode,
};
use wardian_core::models::chat::AgentChatEvent;
use wardian_core::models::AgentConfig;

const REMOTE_AUTOMATION_MONITOR_PAGE_SIZE: usize = 25;
const REMOTE_RUN_FAILURE_SUMMARY: &str = "Run failed. Open Wardian desktop for details.";
const REMOTE_SCHEDULE_FAILURE_SUMMARY: &str = "Last run failed. Open Wardian desktop for details.";

#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteAgentChatPage {
    pub events: Vec<AgentChatEvent>,
    pub has_older: bool,
    pub next_before: Option<usize>,
}

pub async fn remote_agent_roster(state: &AppState) -> Vec<RemoteAgentSummary> {
    // `set_agent_status` persists observations while it owns the global agent
    // map. Waiting here made the remote shell inherit provider/SQLite stalls.
    // A remote read must never make the phone wait for that write to finish.
    let Ok(agents) = state.agents.try_lock() else {
        return remote_agent_roster_fallback(state);
    };
    let order = {
        let Ok(order) = state.agent_order.try_lock() else {
            return remote_agent_roster_fallback(state);
        };
        order.clone()
    };
    // Keep the map guard until the status/config snapshots and roster cache
    // are committed. A pause/resume replacement cannot retire these Arcs
    // between the read and a fallback-cache write.
    let agent_snapshot_handles = agents
        .iter()
        .map(|(session_id, agent)| {
            (
                session_id.clone(),
                agent.config.clone(),
                agent.current_status.clone(),
            )
        })
        .collect::<Vec<_>>();

    // During asynchronous startup the live map can be empty for a short
    // period even though the atomic persisted snapshot already has the full
    // roster. Return that roster instead of declaring the desktop empty.
    if agent_snapshot_handles.is_empty() {
        if let Some(persisted) = persisted_remote_agent_roster() {
            if !persisted.is_empty() {
                return apply_cached_agent_statuses(state, persisted);
            }
        }
    }

    let previous_by_id = state
        .remote_agent_roster_snapshot()
        .unwrap_or_default()
        .into_iter()
        .map(|summary| (summary.session_id.clone(), summary))
        .collect::<HashMap<_, _>>();
    let persisted_by_id = persisted_remote_agent_roster()
        .unwrap_or_default()
        .into_iter()
        .map(|summary| (summary.session_id.clone(), summary))
        .collect::<HashMap<_, _>>();
    let Some(summaries) = agent_snapshot_handles
        .iter()
        .map(|(session_id, config_lock, status_lock)| {
            let config = config_lock.try_lock().ok().map(|config| config.clone());
            let live_status = status_lock.try_lock().ok();
            let live_status_value = live_status.as_deref().cloned();
            if let Some(status) = live_status_value.as_deref() {
                // A successful live snapshot is newer evidence than the
                // fallback snapshot. Publish it so a later global-lock miss
                // cannot overlay an older cached status onto this roster.
                // Keep the status guard held through sequence allocation so a
                // transition cannot receive a lower sequence and then lose
                // its cache update to this older snapshot.
                let sequence = state.next_status_observation_sequence(session_id);
                state.set_remote_agent_status(session_id, status, sequence);
            }
            drop(live_status);
            let cached_status = || state.remote_agent_status(session_id);
            match config {
                Some(config) => Some(remote_agent_summary(
                    config,
                    live_status_value
                        .or_else(cached_status)
                        .or_else(|| {
                            previous_by_id
                                .get(session_id)
                                .map(|summary| summary.status.clone())
                        })
                        .unwrap_or_else(|| "Restoring".to_string()),
                )),
                None => {
                    let mut summary = previous_by_id
                        .get(session_id)
                        .cloned()
                        .or_else(|| persisted_by_id.get(session_id).cloned())?;
                    if let Some(status) = live_status_value.or_else(cached_status) {
                        summary.status = status;
                    }
                    Some(summary)
                }
            }
        })
        .collect::<Option<Vec<_>>>()
    else {
        return remote_agent_roster_fallback(state);
    };

    if agent_snapshot_handles
        .iter()
        .any(|(session_id, _, status)| {
            !agents
                .get(session_id)
                .is_some_and(|agent| std::sync::Arc::ptr_eq(&agent.current_status, status))
        })
    {
        return remote_agent_roster_fallback(state);
    }

    let ordered = order_remote_agent_summaries(summaries, &order);
    state.set_remote_agent_roster_snapshot(ordered.clone());
    ordered
}

fn remote_agent_summary(config: AgentConfig, status: String) -> RemoteAgentSummary {
    RemoteAgentSummary {
        session_id: config.session_id,
        session_name: config.session_name,
        agent_class: config.agent_class,
        provider: config.provider,
        workspace: config.folder,
        status,
        latest_text: None,
    }
}

fn order_remote_agent_summaries(
    summaries: Vec<RemoteAgentSummary>,
    order: &[String],
) -> Vec<RemoteAgentSummary> {
    let mut summaries_by_id = summaries
        .into_iter()
        .map(|summary| (summary.session_id.clone(), summary))
        .collect::<HashMap<_, _>>();

    let mut ordered = Vec::with_capacity(summaries_by_id.len());
    for session_id in order {
        if let Some(summary) = summaries_by_id.remove(session_id) {
            ordered.push(summary);
        }
    }

    let mut remaining = summaries_by_id.into_values().collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.session_name
            .cmp(&right.session_name)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    ordered.extend(remaining);
    ordered
}

fn remote_agent_roster_fallback(state: &AppState) -> Vec<RemoteAgentSummary> {
    let snapshot = state
        .remote_agent_roster_snapshot()
        .or_else(persisted_remote_agent_roster)
        .unwrap_or_default();
    apply_cached_agent_statuses(state, snapshot)
}

fn apply_cached_agent_statuses(
    state: &AppState,
    snapshot: Vec<RemoteAgentSummary>,
) -> Vec<RemoteAgentSummary> {
    let statuses = state.remote_agent_statuses();
    snapshot
        .into_iter()
        .map(|mut summary| {
            if let Some(status) = statuses.get(&summary.session_id) {
                summary.status = status.clone();
            }
            summary
        })
        .collect()
}

fn persisted_remote_agent_roster() -> Option<Vec<RemoteAgentSummary>> {
    let home = crate::utils::fs::get_wardian_home()?;
    let data = std::fs::read_to_string(home.join("settings").join("state.json")).ok()?;
    let configs = serde_json::from_str::<Vec<AgentConfig>>(&data).ok()?;
    Some(
        configs
            .into_iter()
            .map(|config| {
                let status = if config.is_off { "Off" } else { "Restoring" };
                remote_agent_summary(config, status.to_string())
            })
            .collect(),
    )
}

pub fn remote_watchlist_state() -> Result<RemoteWatchlistResponse, String> {
    let Some(home) = crate::utils::fs::get_wardian_home() else {
        return Ok(RemoteWatchlistResponse {
            watchlists: serde_json::json!([]),
            teams: serde_json::json!([]),
            prefs: None,
        });
    };

    let persisted_state = std::fs::read_to_string(home.join("watchlists").join("index.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let (watchlists, teams) = if let Some(state) = persisted_state.as_object() {
        (
            state
                .get("watchlists")
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            state
                .get("teams")
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
    } else if persisted_state.is_array() {
        (persisted_state, serde_json::json!([]))
    } else {
        (serde_json::json!([]), serde_json::json!([]))
    };
    let prefs = std::fs::read_to_string(home.join("watchlists").join("prefs.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok());

    Ok(RemoteWatchlistResponse {
        watchlists,
        teams,
        prefs,
    })
}

pub fn remote_automation_monitor_snapshot(
    active_offset: usize,
    recent_offset: usize,
    schedule_offset: usize,
) -> Result<RemoteAutomationMonitorSnapshot, String> {
    let root = wardian_core::paths::automation_runs_dir().ok_or("no wardian home")?;
    let schedules = wardian_core::schedule::try_load_schedules()
        .map_err(|_| "automation_schedules_unavailable")?;
    remote_automation_monitor_snapshot_from(
        &root,
        schedules,
        active_offset,
        recent_offset,
        schedule_offset,
    )
}

fn remote_automation_monitor_snapshot_from(
    root: &std::path::Path,
    schedules: Vec<wardian_core::models::AutomationSchedule>,
    active_offset: usize,
    recent_offset: usize,
    schedule_offset: usize,
) -> Result<RemoteAutomationMonitorSnapshot, String> {
    let schedule_names = schedules
        .iter()
        .map(|schedule| (schedule.id.clone(), schedule.name.clone()))
        .collect::<HashMap<_, _>>();
    let mut blueprint_names = HashMap::<String, String>::new();
    let active_retain = active_offset.saturating_add(REMOTE_AUTOMATION_MONITOR_PAGE_SIZE + 1);
    let recent_retain = recent_offset.saturating_add(REMOTE_AUTOMATION_MONITOR_PAGE_SIZE + 1);
    let mut active_runs = Vec::new();
    let mut recent_runs = Vec::new();

    if root.exists() {
        for blueprint_entry in std::fs::read_dir(root)
            .map_err(|_| "automation_runs_unavailable")?
            .flatten()
        {
            if !blueprint_entry.path().is_dir() {
                continue;
            }
            let Ok(run_entries) = std::fs::read_dir(blueprint_entry.path()) else {
                continue;
            };
            for run_entry in run_entries.flatten() {
                let run_root = run_entry.path();
                if !run_root.is_dir() {
                    continue;
                }
                let Some(state) = wardian_core::engine::store::read_checkpoint(&run_root)
                    .ok()
                    .flatten()
                else {
                    continue;
                };
                let schedule_id = crate::automation::runs::read_run_invocation(&run_root)
                    .ok()
                    .flatten()
                    .and_then(|invocation| invocation.schedule_id);
                let automation_name = schedule_id
                    .as_ref()
                    .and_then(|id| schedule_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| blueprint_name(&state.blueprint_id, &mut blueprint_names));
                let (started_at, updated_at) =
                    crate::commands::automation::event_log_timestamp_bounds(&run_root);
                let completed_at =
                    matches!(state.status, wardian_core::engine::RunStatus::Completed)
                        .then(|| updated_at.clone())
                        .flatten();
                let run = RemoteAutomationMonitorRun {
                    run_id: state.run_id,
                    blueprint_id: state.blueprint_id,
                    automation_name: bounded_remote_text(&automation_name, 160),
                    schedule_id,
                    status: run_status_label(state.status).to_string(),
                    node_count: state.nodes.len(),
                    completed_node_count: None,
                    failure: project_remote_failure(state.failure, REMOTE_RUN_FAILURE_SUMMARY),
                    started_at,
                    updated_at,
                    completed_at,
                };
                let (target, retain) = if matches!(
                    state.status,
                    wardian_core::engine::RunStatus::Running
                        | wardian_core::engine::RunStatus::AwaitingApproval
                ) {
                    (&mut active_runs, active_retain)
                } else {
                    (&mut recent_runs, recent_retain)
                };
                target.push(run);
                target.sort_by(compare_remote_runs);
                if target.len() > retain {
                    target.pop();
                }
            }
        }
    }

    let (active_runs, active_runs_truncated, active_runs_next_offset) =
        page_remote_items(active_runs, active_offset);
    let (recent_runs, recent_runs_truncated, recent_runs_next_offset) =
        page_remote_items(recent_runs, recent_offset);

    let mut projected_schedules = schedules
        .into_iter()
        .filter(|schedule| schedule.is_paused || schedule.next_run_epoch_ms.is_some())
        .map(project_remote_schedule)
        .collect::<Vec<_>>();
    projected_schedules.sort_by(compare_remote_schedules);
    let (schedules, schedules_truncated, schedules_next_offset) =
        page_remote_items(projected_schedules, schedule_offset);

    Ok(RemoteAutomationMonitorSnapshot {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        active_runs,
        active_runs_truncated,
        active_runs_next_offset,
        recent_runs,
        recent_runs_truncated,
        recent_runs_next_offset,
        schedules,
        schedules_truncated,
        schedules_next_offset,
    })
}

fn page_remote_items<T>(items: Vec<T>, offset: usize) -> (Vec<T>, bool, Option<usize>) {
    let page_end = offset.saturating_add(REMOTE_AUTOMATION_MONITOR_PAGE_SIZE);
    let truncated = items.len() > page_end;
    (
        items
            .into_iter()
            .skip(offset)
            .take(REMOTE_AUTOMATION_MONITOR_PAGE_SIZE)
            .collect(),
        truncated,
        truncated.then_some(page_end),
    )
}

fn blueprint_name(blueprint_id: &str, cache: &mut HashMap<String, String>) -> String {
    if let Some(name) = cache.get(blueprint_id) {
        return name.clone();
    }
    let name = wardian_core::automation::resolve_blueprint_path(blueprint_id)
        .and_then(|path| wardian_core::automation::parse_file(&path).ok())
        .map(|blueprint| blueprint.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| blueprint_id.to_string());
    cache.insert(blueprint_id.to_string(), name.clone());
    name
}

fn run_status_label(status: wardian_core::engine::RunStatus) -> &'static str {
    match status {
        wardian_core::engine::RunStatus::Running => "running",
        wardian_core::engine::RunStatus::AwaitingApproval => "awaiting_approval",
        wardian_core::engine::RunStatus::Completed => "completed",
        wardian_core::engine::RunStatus::Failed => "failed",
    }
}

fn compare_remote_runs(
    left: &RemoteAutomationMonitorRun,
    right: &RemoteAutomationMonitorRun,
) -> std::cmp::Ordering {
    remote_run_time(right)
        .cmp(&remote_run_time(left))
        .then_with(|| right.run_id.cmp(&left.run_id))
}

fn remote_run_time(run: &RemoteAutomationMonitorRun) -> Option<&str> {
    run.updated_at
        .as_deref()
        .or(run.completed_at.as_deref())
        .or(run.started_at.as_deref())
}

fn project_remote_schedule(
    schedule: wardian_core::models::AutomationSchedule,
) -> RemoteAutomationMonitorSchedule {
    let mut target_labels = schedule
        .assignments
        .iter()
        .map(|(role, assignment)| match assignment {
            wardian_core::models::AutomationRoleAssignment::Agent { .. } => {
                format!("{} · Agent", bounded_remote_text(role, 80))
            }
            wardian_core::models::AutomationRoleAssignment::TemporaryProvider {
                provider, ..
            } => {
                format!(
                    "{} · Temporary {}",
                    bounded_remote_text(role, 80),
                    bounded_remote_text(provider, 80)
                )
            }
        })
        .collect::<Vec<_>>();
    for role in schedule.bindings.keys() {
        if !schedule.assignments.contains_key(role) {
            target_labels.push(format!("{} · Assigned", bounded_remote_text(role, 80)));
        }
    }
    target_labels.sort();
    target_labels.dedup();
    RemoteAutomationMonitorSchedule {
        id: schedule.id,
        blueprint_id: schedule.blueprint_id,
        automation_name: bounded_remote_text(&schedule.name, 160),
        schedule: schedule.schedule,
        next_run_epoch_ms: schedule.next_run_epoch_ms,
        is_paused: schedule.is_paused,
        last_run_status: schedule.last_run_status.filter(|value| {
            matches!(
                value.as_str(),
                "running" | "awaiting_approval" | "completed" | "failed"
            )
        }),
        last_run_error: project_remote_failure(
            schedule.last_run_error,
            REMOTE_SCHEDULE_FAILURE_SUMMARY,
        ),
        last_run_epoch_ms: schedule.last_run_epoch_ms,
        target_labels,
    }
}

fn compare_remote_schedules(
    left: &RemoteAutomationMonitorSchedule,
    right: &RemoteAutomationMonitorSchedule,
) -> std::cmp::Ordering {
    left.is_paused
        .cmp(&right.is_paused)
        .then_with(|| {
            if left.is_paused {
                right.last_run_epoch_ms.cmp(&left.last_run_epoch_ms)
            } else {
                left.next_run_epoch_ms.cmp(&right.next_run_epoch_ms)
            }
        })
        .then_with(|| left.automation_name.cmp(&right.automation_name))
        .then_with(|| left.id.cmp(&right.id))
}

fn bounded_remote_text(value: &str, max_chars: usize) -> String {
    strip_ansi_controls(value)
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .take(max_chars)
        .collect()
}

/// Failure payloads can contain provider output and local paths. The remote
/// trust boundary exposes only a stable outcome summary; full diagnostics stay
/// on the desktop.
fn project_remote_failure(raw_failure: Option<String>, summary: &str) -> Option<String> {
    raw_failure.map(|_| summary.to_string())
}

/// Builds the fast, durable portion of the Inbox projection. Live automation
/// run reconciliation is refreshed separately so a filesystem-heavy run
/// history cannot make the remote compatibility endpoint time out.
pub async fn remote_queue_items(state: &AppState) -> Vec<serde_json::Value> {
    let mut items = remote_durable_queue_items(state).await;
    if let Some(runtime_items) = state.remote_inbox_runtime_items() {
        items.extend(runtime_items);
    }
    sort_remote_queue_items(&mut items);
    items
}

/// Returns the durable Inbox immediately and schedules one background
/// reconciliation of automation approvals/completions. The next request sees
/// the cached runtime projection without waiting for directory scans.
pub async fn remote_queue_items_for_app(
    app: &AppHandle,
    state: &AppState,
) -> Vec<serde_json::Value> {
    let items = remote_queue_items(state).await;
    if let Some(refresh_generation) = state.try_start_remote_inbox_runtime_refresh() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            let runtime_items = remote_runtime_inbox_items(state.inner()).await;
            state
                .inner()
                .set_remote_inbox_runtime_items(refresh_generation, runtime_items);
        });
    }
    items
}

/// Builds the action lookup from an authoritative notification projection.
/// Read requests use a bounded timeout so the remote shell stays responsive;
/// mutations must wait for the durable interaction source instead of turning
/// lock contention into a false `inbox_item_not_found` or no-op bulk action.
async fn remote_queue_items_for_mutation(state: &AppState) -> Vec<serde_json::Value> {
    let mut items = remote_durable_queue_items_authoritative(state).await;
    if let Some(runtime_items) = state.remote_inbox_runtime_items() {
        items.extend(runtime_items);
    } else {
        items.extend(remote_runtime_inbox_items(state).await);
    }
    sort_remote_queue_items(&mut items);
    items
}

async fn remote_durable_queue_items(state: &AppState) -> Vec<serde_json::Value> {
    let cutoff = chrono::Utc::now().timestamp_millis() - QUEUE_MAX_AGE_MS;
    let queue_metadata = wardian_core::queue::load_recent_items(MAX_INBOX_SOURCE_ITEMS, 0, cutoff);
    let read_notification_ids = queue_metadata.read_notification_ids;
    let context = InboxProjectionContext {
        cutoff,
        read_notification_ids: &read_notification_ids,
        persisted_automation_runs: &queue_metadata.automation_runs,
        types: &[],
        sources: &[],
        unread: false,
    };
    let mut items = Vec::new();
    if let Ok(Ok(page)) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        remote_inbox_source_page(
            state,
            InboxSource::Notifications,
            0,
            MAX_INBOX_SOURCE_ITEMS,
            &context,
        ),
    )
    .await
    {
        items.extend(page.items);
    }
    items.extend(persisted_queue_items().into_iter().filter(|item| {
        item_timestamp(item) > cutoff
            && is_legacy_queue_item(item)
            && item.get("dismissed").and_then(serde_json::Value::as_bool) != Some(true)
    }));
    items
}

async fn remote_durable_queue_items_authoritative(state: &AppState) -> Vec<serde_json::Value> {
    let cutoff = chrono::Utc::now().timestamp_millis() - QUEUE_MAX_AGE_MS;
    let queue_metadata = wardian_core::queue::load_recent_items(MAX_INBOX_SOURCE_ITEMS, 0, cutoff);
    let context = InboxProjectionContext {
        cutoff,
        read_notification_ids: &queue_metadata.read_notification_ids,
        persisted_automation_runs: &queue_metadata.automation_runs,
        types: &[],
        sources: &[],
        unread: false,
    };
    let mut items = Vec::new();
    if let Ok(page) = remote_inbox_source_page(
        state,
        InboxSource::Notifications,
        0,
        MAX_INBOX_SOURCE_ITEMS,
        &context,
    )
    .await
    {
        items.extend(page.items);
    }
    items.extend(persisted_queue_items().into_iter().filter(|item| {
        item_timestamp(item) > cutoff
            && is_legacy_queue_item(item)
            && item.get("dismissed").and_then(serde_json::Value::as_bool) != Some(true)
    }));
    items
}

async fn remote_runtime_inbox_items(state: &AppState) -> Vec<serde_json::Value> {
    let cutoff = chrono::Utc::now().timestamp_millis() - QUEUE_MAX_AGE_MS;
    let queue_metadata = wardian_core::queue::load_recent_items(MAX_INBOX_SOURCE_ITEMS, 0, cutoff);
    let context = InboxProjectionContext {
        cutoff,
        read_notification_ids: &queue_metadata.read_notification_ids,
        persisted_automation_runs: &queue_metadata.automation_runs,
        types: &[],
        sources: &[],
        unread: false,
    };
    let mut items = Vec::new();
    for source in [
        InboxSource::AutomationApprovals,
        InboxSource::AutomationTerminals,
    ] {
        if let Ok(page) =
            remote_inbox_source_page(state, source, 0, MAX_INBOX_SOURCE_ITEMS, &context).await
        {
            items.extend(page.items);
        }
    }
    items
}

fn sort_remote_queue_items(items: &mut Vec<serde_json::Value>) {
    items.sort_by(|left, right| {
        item_timestamp(left)
            .cmp(&item_timestamp(right))
            .then_with(|| item_id(left).cmp(item_id(right)))
            .reverse()
    });
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_automation = std::collections::HashSet::new();
    items.retain(|item| {
        let id = item_id(item);
        if !id.is_empty() && !seen_ids.insert(id.to_string()) {
            return false;
        }
        if matches!(
            item.get("type").and_then(serde_json::Value::as_str),
            Some("automation_completed" | "workflow_completed")
        ) {
            if let Some(identity) = automation_identity(item) {
                return seen_automation.insert(identity);
            }
        }
        true
    });
}

/// Builds one bounded page of the Inbox projection while preserving the
/// durable-notification pagination boundary for callers that need to continue
/// reading older events.
const MAX_INBOX_SOURCE_ITEMS: usize = 200;
const QUEUE_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub async fn remote_queue_items_page(
    state: &AppState,
    offset: usize,
) -> (Vec<serde_json::Value>, bool, Option<usize>) {
    remote_inbox_list_page(state, offset, &[], &[], false, MAX_INBOX_SOURCE_ITEMS)
        .await
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
enum InboxSource {
    Notifications,
    AutomationApprovals,
    AutomationTerminals,
    LegacyQueue,
}

struct InboxSourcePage {
    items: Vec<serde_json::Value>,
    truncated: bool,
}

struct InboxProjectionContext<'a> {
    cutoff: i64,
    read_notification_ids: &'a std::collections::HashSet<String>,
    persisted_automation_runs: &'a std::collections::HashSet<(String, String)>,
    types: &'a [String],
    sources: &'a [String],
    unread: bool,
}

/// Builds one globally ordered, filter-aware Inbox page. Each source is
/// advanced independently behind a single merged cursor so filtering and
/// pagination cannot skip records from another source.
pub async fn remote_inbox_list_page(
    state: &AppState,
    offset: usize,
    types: &[String],
    sources: &[String],
    unread: bool,
    limit: usize,
) -> Result<(Vec<serde_json::Value>, bool, Option<usize>), String> {
    if limit == 0 {
        return Ok((Vec::new(), false, None));
    }
    if offset > wardian_core::control::MAX_INBOX_OFFSET
        || limit > wardian_core::control::MAX_INBOX_PAGE_LIMIT
        || offset.saturating_add(limit) > wardian_core::control::MAX_INBOX_OFFSET
    {
        return Err("Inbox page exceeds the accepted offset or limit bounds".to_string());
    }
    let cutoff = chrono::Utc::now().timestamp_millis() - QUEUE_MAX_AGE_MS;
    let queue_metadata = wardian_core::queue::load_recent_items(MAX_INBOX_SOURCE_ITEMS, 0, cutoff);
    let read_notification_ids = queue_metadata.read_notification_ids;
    let persisted_automation_runs = queue_metadata.automation_runs;
    let source_kinds = [
        InboxSource::Notifications,
        InboxSource::AutomationApprovals,
        InboxSource::AutomationTerminals,
        InboxSource::LegacyQueue,
    ];
    let context = InboxProjectionContext {
        cutoff,
        read_notification_ids: &read_notification_ids,
        persisted_automation_runs: &persisted_automation_runs,
        types,
        sources,
        unread,
    };
    let source_page_limit = offset.saturating_add(limit).saturating_add(1);
    let mut pages = Vec::with_capacity(source_kinds.len());
    for source in source_kinds {
        pages.push(
            remote_inbox_source_page(state, source, 0, source_page_limit, &context)
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    let mut offsets = [0usize; 4];
    let mut skipped = 0usize;
    let mut items = Vec::with_capacity(limit);
    loop {
        for index in 0..pages.len() {
            if pages[index].items.is_empty() && pages[index].truncated {
                offsets[index] = offsets[index].saturating_add(source_page_limit);
                pages[index] = remote_inbox_source_page(
                    state,
                    source_kinds[index],
                    offsets[index],
                    source_page_limit,
                    &context,
                )
                .await
                .map_err(|error| error.to_string())?;
            }
        }
        let Some(source_index) = pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                page.items
                    .first()
                    .map(|item| (index, item_timestamp(item), item_id(item)))
            })
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| left.2.cmp(right.2)))
            .map(|candidate| candidate.0)
        else {
            break;
        };

        let item = pages[source_index].items.remove(0);
        if !inbox_item_matches(&item, types, sources, unread) {
            continue;
        }
        if skipped < offset {
            skipped += 1;
            continue;
        }
        if items.len() >= limit {
            return Ok(page_after_lookahead(items, offset, limit));
        }
        items.push(item);
    }

    Ok((items, false, None))
}

fn next_inbox_offset(offset: usize, limit: usize) -> Option<usize> {
    let next_offset = offset.saturating_add(limit);
    (next_offset < wardian_core::control::MAX_INBOX_OFFSET).then_some(next_offset)
}

fn page_after_lookahead(
    items: Vec<serde_json::Value>,
    offset: usize,
    limit: usize,
) -> (Vec<serde_json::Value>, bool, Option<usize>) {
    (items, true, next_inbox_offset(offset, limit))
}

pub(crate) async fn inbox_list_control(
    app: &AppHandle,
    request: ControlRequest,
) -> Result<String, crate::control::ControlError> {
    let ControlRequest::InboxList {
        offset,
        types,
        sources,
        unread,
        limit,
    } = request
    else {
        return Err(crate::control::ControlError::bad_request(
            "invalid Inbox control request",
        ));
    };
    if offset > wardian_core::control::MAX_INBOX_OFFSET {
        return Err(crate::control::ControlError::bad_request(format!(
            "Inbox offset must not exceed {}",
            wardian_core::control::MAX_INBOX_OFFSET
        )));
    }
    if limit == 0 || limit > wardian_core::control::MAX_INBOX_PAGE_LIMIT {
        return Err(crate::control::ControlError::bad_request(
            "Inbox limit must be between 1 and 200",
        ));
    }
    if offset.saturating_add(limit) > wardian_core::control::MAX_INBOX_OFFSET {
        return Err(crate::control::ControlError::bad_request(format!(
            "Inbox offset plus limit must not exceed {}",
            wardian_core::control::MAX_INBOX_OFFSET
        )));
    }
    let state = app.state::<AppState>();
    let (items, truncated, next_offset) =
        remote_inbox_list_page(state.inner(), offset, &types, &sources, unread, limit)
            .await
            .map_err(crate::control::ControlError::request_failed)?;
    serde_json::to_string(&InboxListResponse::new(items, truncated, next_offset))
        .map_err(crate::control::ControlError::request_failed)
}

fn inbox_source_name(source: InboxSource) -> &'static str {
    match source {
        InboxSource::Notifications => "interaction_store",
        InboxSource::AutomationApprovals | InboxSource::AutomationTerminals => "live_runtime",
        InboxSource::LegacyQueue => "provider_runtime",
    }
}

fn source_may_match(source: InboxSource, types: &[String], sources: &[String]) -> bool {
    (sources.is_empty()
        || matches!(source, InboxSource::LegacyQueue)
        || sources
            .iter()
            .any(|value| value == inbox_source_name(source)))
        && (types.is_empty()
            || match source {
                InboxSource::Notifications => types
                    .iter()
                    .any(|value| matches!(value.as_str(), "agent_update" | "approval_request")),
                InboxSource::AutomationApprovals => {
                    types.iter().any(|value| value == "approval_request")
                }
                InboxSource::AutomationTerminals => types.iter().any(|value| {
                    matches!(
                        value.as_str(),
                        "automation_completed"
                            | "automation_failed"
                            | "workflow_completed"
                            | "workflow_failed"
                    )
                }),
                InboxSource::LegacyQueue => true,
            })
}

async fn remote_inbox_source_page(
    state: &AppState,
    source: InboxSource,
    offset: usize,
    page_limit: usize,
    context: &InboxProjectionContext<'_>,
) -> Result<InboxSourcePage, String> {
    if !source_may_match(source, context.types, context.sources) {
        return Ok(InboxSourcePage {
            items: Vec::new(),
            truncated: false,
        });
    }

    if matches!(source, InboxSource::LegacyQueue) {
        let persisted = wardian_core::queue::load_recent_items_matching(
            page_limit,
            offset,
            context.cutoff,
            |item| {
                item.get("inbox_notification_id").is_none()
                    && item.get("workflow_approval").is_none()
                    && item.get("automation_approval").is_none()
                    && item.get("dismissed").and_then(serde_json::Value::as_bool) != Some(true)
                    && inbox_item_matches(item, context.types, context.sources, context.unread)
            },
        );
        return Ok(InboxSourcePage {
            items: persisted.items,
            truncated: persisted.truncated,
        });
    }

    let page_end = offset.saturating_add(page_limit);
    let mut raw_offset = 0usize;
    let mut matching = Vec::new();
    let mut truncated = false;
    loop {
        let page = remote_inbox_source_page_raw(
            state,
            source,
            raw_offset,
            context.cutoff,
            context.read_notification_ids,
        )
        .await?;
        matching.extend(page.items.into_iter().filter(|item| {
            let persisted_automation_duplicate = matches!(source, InboxSource::AutomationTerminals)
                && !context.sources.iter().any(|value| value == "live_runtime")
                && automation_identity(item)
                    .is_some_and(|key| context.persisted_automation_runs.contains(&key));
            !persisted_automation_duplicate
                && inbox_item_matches(item, context.types, context.sources, context.unread)
        }));
        if matching.len() > page_end {
            truncated = true;
            break;
        }
        if !page.truncated {
            break;
        }
        raw_offset = raw_offset.saturating_add(MAX_INBOX_SOURCE_ITEMS);
    }
    matching.sort_by(|left, right| {
        item_timestamp(left)
            .cmp(&item_timestamp(right))
            .then_with(|| item_id(left).cmp(item_id(right)))
            .reverse()
    });
    Ok(InboxSourcePage {
        items: matching.into_iter().skip(offset).take(page_limit).collect(),
        truncated,
    })
}

async fn remote_inbox_source_page_raw(
    state: &AppState,
    source: InboxSource,
    offset: usize,
    cutoff: i64,
    read_notification_ids: &std::collections::HashSet<String>,
) -> Result<InboxSourcePage, String> {
    match source {
        InboxSource::Notifications => {
            let notification_page =
                crate::commands::inbox::list_inbox_notifications_for_state_with_offset_read_only(
                    state, offset,
                )
                .await;
            let (notifications, truncated) = match notification_page {
                Ok(result) => (result.notifications, result.truncated),
                Err(_) => (Vec::new(), false),
            };
            Ok(InboxSourcePage {
                items: notifications
                    .into_iter()
                    .map(|notification| {
                        let is_approval =
                            matches!(&notification.kind, InboxNotificationKind::Approval);
                        serde_json::json!({
                            "id": format!("notification:{}", notification.id),
                            "type": if is_approval { "approval_request" } else { "agent_update" },
                            "timestamp": queue_timestamp(&notification.created_at),
                            "read": if is_approval { notification.status != InteractionStatus::AwaitingReply } else { read_notification_ids.contains(notification.id.as_str()) },
                            "agent_session_id": notification.sender_session_id,
                            "evidence_source": "interaction_store",
                            "inbox_notification_id": notification.id,
                            "notification_status": notification.status,
                            "summary": notification.body,
                            "notification_title": notification.title,
                            "proposed_action": notification.proposed_action,
                            "risk": notification.risk,
                            "approval_choices": notification.choices,
                            "approval_decision": notification.decision.map(|decision| decision.choice),
                            "expires_at": notification.expires_at,
                        })
                    })
                    .collect(),
                truncated,
            })
        }
        InboxSource::AutomationApprovals => {
            let (approvals, truncated) =
                crate::commands::inbox::list_automation_inbox_approvals_page(offset).await?;
            Ok(InboxSourcePage {
                items: approvals
                    .into_iter()
                    .map(|approval| {
                        serde_json::json!({
                            "id": format!("automation-approval:{}:{}:{}", approval.blueprint_id, approval.run_id, approval.node),
                            "type": "approval_request",
                            "timestamp": approval.created_at.as_deref().map(queue_timestamp).unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
                            "read": false,
                            "evidence_source": "live_runtime",
                            "automation_id": approval.blueprint_id,
                            "automation_run_id": approval.run_id,
                            "automation_name": approval.title,
                            "notification_title": approval.title,
                            "summary": approval.prompt,
                            "proposed_action": "Continue this automation beyond its approval gate",
                            "risk": "The automation will execute the next authored steps after approval.",
                            "approval_choices": ["Approve", "Reject"],
                            "automation_approval": { "blueprint_id": approval.blueprint_id, "blueprint_path": approval.blueprint_path, "run_id": approval.run_id, "node": approval.node },
                        })
                    })
                    .collect(),
                truncated,
            })
        }
        InboxSource::AutomationTerminals => {
            let (terminals, truncated) =
                crate::commands::inbox::list_automation_inbox_terminal_runs_page(offset).await?;
            Ok(InboxSourcePage {
                items: terminals
                    .into_iter()
                    .map(|run| {
                        serde_json::json!({
                            "id": format!("automation-completion:{}:{}", run.automation_id, run.run_instance_id),
                            "type": "automation_completed",
                            "timestamp": run.updated_at.as_deref().map(queue_timestamp).unwrap_or_default(),
                            "read": false,
                            "evidence_source": "live_runtime",
                            "automation_id": run.automation_id,
                            "automation_run_id": run.run_instance_id,
                            "automation_name": run.automation_name,
                            "status": run.status,
                            "error": run.error,
                            "summary": run.summary,
                        })
                    })
                    .collect(),
                truncated,
            })
        }
        InboxSource::LegacyQueue => {
            let persisted =
                wardian_core::queue::load_recent_items(MAX_INBOX_SOURCE_ITEMS, offset, cutoff);
            Ok(InboxSourcePage {
                items: persisted
                    .items
                    .into_iter()
                    .filter(|item| {
                        item.get("inbox_notification_id").is_none()
                            && item.get("workflow_approval").is_none()
                            && item.get("automation_approval").is_none()
                            && item.get("dismissed").and_then(serde_json::Value::as_bool)
                                != Some(true)
                    })
                    .collect(),
                truncated: persisted.truncated,
            })
        }
    }
}

fn inbox_item_matches(
    item: &serde_json::Value,
    types: &[String],
    sources: &[String],
    unread: bool,
) -> bool {
    let type_matches = types.is_empty()
        || types.iter().any(|kind| {
            item.get("type").and_then(serde_json::Value::as_str) == Some(kind.as_str())
                || ((kind == "automation_failed" || kind == "workflow_failed")
                    && item
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|item_type| {
                            item_type == "automation_completed" || item_type == "workflow_completed"
                        })
                    && item.get("status").and_then(serde_json::Value::as_str) == Some("failed"))
        });
    let item_source = item
        .get("evidence_source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("provider_runtime");
    let source_matches = sources.is_empty() || sources.iter().any(|source| source == item_source);
    type_matches
        && source_matches
        && (!unread || item.get("read").and_then(serde_json::Value::as_bool) != Some(true))
}

fn item_timestamp(item: &serde_json::Value) -> i64 {
    item.get("timestamp")
        .and_then(serde_json::Value::as_i64)
        .or_else(|| {
            item.get("timestamp")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| i64::try_from(value).ok())
        })
        .unwrap_or_default()
}

fn item_id(item: &serde_json::Value) -> &str {
    item.get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn persisted_queue_items() -> Vec<serde_json::Value> {
    crate::utils::queue::load_items()
}

fn save_persisted_queue_items(items: &[serde_json::Value]) -> Result<(), String> {
    crate::utils::queue::save_items(items)
}

fn is_legacy_queue_item(item: &serde_json::Value) -> bool {
    item.get("inbox_notification_id").is_none() && item.get("automation_approval").is_none()
}

fn is_clearable_legacy_completion(item: &serde_json::Value) -> bool {
    is_legacy_queue_item(item)
        && item.get("dismissed").and_then(serde_json::Value::as_bool) != Some(true)
        && !provider_choice_acknowledgement_unresolved(item)
        && matches!(
            item.get("type").and_then(serde_json::Value::as_str),
            Some("agent_completed" | "automation_completed")
        )
}

fn is_pending_approval(item: &serde_json::Value) -> bool {
    item.get("automation_approval").is_some()
        || (item.get("type").and_then(serde_json::Value::as_str) == Some("approval_request")
            && item
                .get("notification_status")
                .and_then(serde_json::Value::as_str)
                == Some("awaiting_reply"))
}

fn provider_choice_acknowledgement_unresolved(item: &serde_json::Value) -> bool {
    item.get("provider_choice_pending").is_some()
        || (item.get("provider_choice_sent").is_some()
            && item.get("read").and_then(serde_json::Value::as_bool) != Some(true))
}

fn notification_read_acknowledgement(notification_id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("notification-read:{notification_id}"),
        "type": "agent_update",
        "timestamp": chrono::Utc::now().timestamp_millis(),
        "read": true,
        "inbox_notification_id": notification_id,
    })
}

fn current_queue_item<'a>(
    items: &'a [serde_json::Value],
    item_id: &str,
) -> Result<&'a serde_json::Value, String> {
    items
        .iter()
        .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(item_id))
        .ok_or_else(|| "inbox_item_not_found".to_string())
}

fn automation_identity(item: &serde_json::Value) -> Option<(String, String)> {
    if !matches!(
        item.get("type").and_then(serde_json::Value::as_str),
        Some("automation_completed" | "workflow_completed")
    ) {
        return None;
    }
    Some((
        item.get("automation_id")
            .or_else(|| item.get("workflow_id"))
            .and_then(serde_json::Value::as_str)?
            .to_string(),
        item.get("automation_run_id")
            .or_else(|| item.get("workflow_run_id"))
            .and_then(serde_json::Value::as_str)?
            .to_string(),
    ))
}

fn automation_dismissal_marker(item: &serde_json::Value) -> Option<serde_json::Value> {
    let (automation_id, run_id) = automation_identity(item)?;
    let legacy = item.get("automation_id").is_none();
    Some(if legacy {
        serde_json::json!({
            "id": format!("workflow-dismissed:{automation_id}:{run_id}"),
            "type": "workflow_completed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "read": true,
            "dismissed": true,
            "workflow_id": automation_id,
            "workflow_run_id": run_id,
        })
    } else {
        serde_json::json!({
            "id": format!("automation-dismissed:{automation_id}:{run_id}"),
            "type": "automation_completed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "read": true,
            "dismissed": true,
            "automation_id": automation_id,
            "automation_run_id": run_id,
        })
    })
}

fn validate_remote_mark_read(item: &serde_json::Value) -> Result<(), String> {
    if is_pending_approval(item) {
        return Err("pending_approval_cannot_be_marked_read".to_string());
    }
    if item.get("provider_choice_pending").is_some() {
        return Err("provider_choice_delivery_uncertain_cannot_be_marked_read".to_string());
    }
    Ok(())
}

/// Applies a mobile Inbox mutation to the same persisted projection used by
/// the desktop Inbox. The caller must authenticate and rate-limit the request.
pub async fn apply_remote_inbox_action(
    state: &AppState,
    app: &AppHandle,
    request: RemoteInboxActionRequest,
) -> Result<(), String> {
    let _queue_guard = state.queue_io_lock.lock().await;
    let projected_items = remote_queue_items_for_mutation(state).await;
    match request.action.as_str() {
        "mark_read" => {
            let item_id = request
                .item_id
                .as_deref()
                .ok_or_else(|| "item_id_required".to_string())?;
            let item = current_queue_item(&projected_items, item_id)?;
            validate_remote_mark_read(item)?;
            let mut persisted = persisted_queue_items();
            if let Some(notification_id) = item
                .get("inbox_notification_id")
                .and_then(serde_json::Value::as_str)
            {
                if !persisted.iter().any(|candidate| {
                    candidate
                        .get("inbox_notification_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(notification_id)
                        && candidate.get("read").and_then(serde_json::Value::as_bool) == Some(true)
                }) {
                    persisted.push(notification_read_acknowledgement(notification_id));
                }
            } else if let Some(candidate) = persisted.iter_mut().find(|candidate| {
                candidate.get("id").and_then(serde_json::Value::as_str) == Some(item_id)
            }) {
                candidate["read"] = serde_json::Value::Bool(true);
            } else if automation_identity(item).is_some() {
                let mut persisted_item = item.clone();
                persisted_item["read"] = serde_json::Value::Bool(true);
                persisted.push(persisted_item);
            } else {
                return Err("inbox_item_not_persisted".to_string());
            }
            save_persisted_queue_items(&persisted)?;
        }
        "mark_all_read" => {
            let mut persisted = persisted_queue_items();
            for item in persisted.iter_mut().filter(|item| {
                is_legacy_queue_item(item) && !provider_choice_acknowledgement_unresolved(item)
            }) {
                item["read"] = serde_json::Value::Bool(true);
            }
            let known_acknowledgements = persisted
                .iter()
                .filter(|item| item.get("read").and_then(serde_json::Value::as_bool) == Some(true))
                .filter_map(|item| {
                    item.get("inbox_notification_id")
                        .and_then(serde_json::Value::as_str)
                })
                .map(str::to_string)
                .collect::<std::collections::HashSet<_>>();
            for item in projected_items.iter().filter(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("agent_update")
            }) {
                if let Some(notification_id) = item
                    .get("inbox_notification_id")
                    .and_then(serde_json::Value::as_str)
                {
                    if !known_acknowledgements.contains(notification_id) {
                        persisted.push(notification_read_acknowledgement(notification_id));
                    }
                }
            }
            let known_automation_runs = persisted
                .iter()
                .filter_map(automation_identity)
                .collect::<std::collections::HashSet<_>>();
            for item in projected_items.iter().filter(|item| {
                matches!(
                    item.get("type").and_then(serde_json::Value::as_str),
                    Some("automation_completed" | "workflow_completed")
                )
            }) {
                let Some(identity) = automation_identity(item) else {
                    continue;
                };
                if known_automation_runs.contains(&identity) {
                    continue;
                }
                let mut persisted_item = item.clone();
                persisted_item["read"] = serde_json::Value::Bool(true);
                persisted.push(persisted_item);
            }
            save_persisted_queue_items(&persisted)?;
        }
        "clear_read" => {
            let persisted = persisted_queue_items();
            let mut automation_dismissals = Vec::new();
            let next = persisted
                .into_iter()
                .filter_map(|item| {
                    let clear = is_clearable_legacy_completion(&item)
                        && item.get("read").and_then(serde_json::Value::as_bool) == Some(true);
                    if clear {
                        if let Some(marker) = automation_dismissal_marker(&item) {
                            automation_dismissals.push(marker);
                        }
                        None
                    } else {
                        Some(item)
                    }
                })
                .collect::<Vec<_>>();
            let mut next = next;
            next.extend(automation_dismissals);
            save_persisted_queue_items(&next)?;
        }
        "dismiss" => {
            let item_id = request
                .item_id
                .as_deref()
                .ok_or_else(|| "item_id_required".to_string())?;
            let item = current_queue_item(&projected_items, item_id)?;
            if item.get("automation_approval").is_some()
                || item.get("inbox_notification_id").is_some()
                || provider_choice_acknowledgement_unresolved(item)
            {
                return Err("inbox_item_not_dismissible".to_string());
            }
            let persisted = persisted_queue_items();
            let next = persisted
                .into_iter()
                .filter(|candidate| {
                    candidate.get("id").and_then(serde_json::Value::as_str) != Some(item_id)
                })
                .chain(automation_dismissal_marker(item))
                .collect::<Vec<_>>();
            save_persisted_queue_items(&next)?;
        }
        "resolve_approval" => {
            let item_id = request
                .item_id
                .as_deref()
                .ok_or_else(|| "item_id_required".to_string())?;
            let choice = request
                .choice
                .as_deref()
                .ok_or_else(|| "choice_required".to_string())?;
            let item = current_queue_item(&projected_items, item_id)?;
            let choices = item
                .get("approval_choices")
                .and_then(serde_json::Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !choices.contains(&choice) {
                return Err("invalid_choice".to_string());
            }
            if let Some(notification_id) = item
                .get("inbox_notification_id")
                .and_then(serde_json::Value::as_str)
            {
                state
                    .interactions
                    .resolve_notification(notification_id, choice)
                    .await
                    .map_err(|error| error.to_string())?;
            } else if let Some(automation_approval) = item.get("automation_approval") {
                crate::commands::automation::approve_automation_for_surface(
                    state,
                    app.clone(),
                    automation_approval
                        .get("blueprint_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "automation_blueprint_id_missing".to_string())?
                        .to_string(),
                    automation_approval
                        .get("run_id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "automation_run_id_missing".to_string())?
                        .to_string(),
                    automation_approval
                        .get("blueprint_path")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "automation_blueprint_path_missing".to_string())?
                        .to_string(),
                    automation_approval
                        .get("node")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "automation_node_missing".to_string())?
                        .to_string(),
                    choice == "Approve",
                    "user".to_string(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            } else {
                return Err("inbox_item_not_approval".to_string());
            }
        }
        _ => return Err("unsupported_inbox_action".to_string()),
    }
    state.invalidate_remote_inbox_runtime();
    let _ = app.emit("inbox-updated", ());
    Ok(())
}

fn queue_timestamp(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.timestamp_millis())
        .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
}

pub async fn remote_agent_chat_transcript(
    state: &AppState,
    session_id: &str,
) -> Result<Vec<AgentChatEvent>, String> {
    crate::commands::chat::load_agent_chat_transcript_for_state(state, session_id.to_string()).await
}

pub async fn remote_agent_chat_page(
    state: &AppState,
    session_id: &str,
    before: Option<usize>,
    limit: usize,
) -> Result<RemoteAgentChatPage, String> {
    let events = remote_agent_chat_transcript(state, session_id).await?;
    Ok(page_remote_agent_chat_events(events, before, limit))
}

pub fn page_remote_agent_chat_events(
    events: Vec<AgentChatEvent>,
    before: Option<usize>,
    limit: usize,
) -> RemoteAgentChatPage {
    let end = before.unwrap_or(events.len()).min(events.len());
    let start = end.saturating_sub(limit.max(1));

    RemoteAgentChatPage {
        events: events[start..end].to_vec(),
        has_older: start > 0,
        next_before: (start > 0).then_some(start),
    }
}

pub async fn remote_agent_terminal_snapshot(
    state: &AppState,
    session_id: &str,
    since: Option<&str>,
    tail_bytes: Option<usize>,
) -> Result<RemoteTerminalSnapshot, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .map(|agent| agent.watch_state.clone())
            .ok_or_else(|| "agent_not_found".to_string())?
    };
    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch_state_unavailable".to_string())?
        .snapshot_since(since, tail_bytes)
        .map_err(|error| error.code().to_string())?;

    Ok(RemoteTerminalSnapshot {
        cursor: snapshot.output.cursor,
        text: snapshot.output.text,
        truncated: snapshot.output.truncated,
        omitted_bytes: snapshot.output.omitted_bytes,
    })
}

pub async fn remote_agent_terminal_raw_output(
    state: &AppState,
    session_id: &str,
    tail_bytes: Option<usize>,
) -> Result<String, String> {
    let watch_state = {
        let agents = state.agents.lock().await;
        agents
            .get(session_id)
            .map(|agent| agent.watch_state.clone())
            .ok_or_else(|| "agent_not_found".to_string())?
    };
    let snapshot = watch_state
        .lock()
        .map_err(|_| "watch_state_unavailable".to_string())?
        .raw_snapshot_since(None, tail_bytes)
        .map_err(|error| error.code().to_string())?;

    Ok(snapshot.text)
}

pub fn validate_remote_agent_action(request: &RemoteAgentActionRequest) -> Result<(), String> {
    if request.action == "send_prompt" {
        if request
            .prompt
            .as_ref()
            .is_none_or(|prompt| prompt.trim().is_empty())
        {
            Err("prompt_required".to_string())
        } else if matches!(
            request.input_mode.unwrap_or_default(),
            MessageInputMode::Message | MessageInputMode::Command
        ) {
            Ok(())
        } else {
            Err("unsupported_remote_input_mode".to_string())
        }
    } else {
        match request.action.as_str() {
            "pause" | "resume" | "clear" | "kill" => Ok(()),
            _ => Err("unsupported_remote_agent_action".to_string()),
        }
    }
}

pub async fn run_remote_agent_action(
    app: &AppHandle,
    request: RemoteAgentActionRequest,
) -> Result<(), String> {
    validate_remote_agent_action(&request)?;
    match request.action.as_str() {
        "send_prompt" => {
            let state = app.state::<AppState>();
            let prompt = request.prompt.unwrap_or_default();
            let _queue_guard = if request.inbox_item_id.is_some() {
                Some(state.queue_io_lock.lock().await)
            } else {
                None
            };
            send_remote_prompt_with_idempotency(
                app,
                &state,
                &request.target,
                &prompt,
                request.input_mode.unwrap_or_default(),
                request.inbox_item_id.as_deref(),
            )
            .await
        }
        "pause" => {
            let state = app.state::<AppState>();
            crate::commands::agent::pause_agent(request.target, state, app.clone()).await
        }
        "resume" => {
            let state = app.state::<AppState>();
            crate::commands::agent::resume_agent(request.target, state, app.clone()).await
        }
        "clear" => {
            let state = app.state::<AppState>();
            crate::commands::agent::clear_agent_session(request.target, None, state, app.clone())
                .await
        }
        "kill" => {
            let state = app.state::<AppState>();
            crate::commands::agent::kill_agent(request.target, state, app.clone()).await
        }
        _ => Err("unsupported_remote_agent_action".to_string()),
    }
}

async fn send_remote_prompt_with_idempotency(
    app: &AppHandle,
    state: &AppState,
    target: &str,
    prompt: &str,
    input_mode: MessageInputMode,
    inbox_item_id: Option<&str>,
) -> Result<(), String> {
    if let Some(item_id) = inbox_item_id {
        let mut persisted = crate::utils::queue::load_items();
        let index = persisted
            .iter()
            .position(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(item_id))
            .ok_or_else(|| "inbox_item_not_found".to_string())?;
        let item = &persisted[index];
        if item
            .get("agent_session_id")
            .and_then(serde_json::Value::as_str)
            != Some(target)
        {
            return Err("inbox_item_agent_mismatch".to_string());
        }
        if item.get("type").and_then(serde_json::Value::as_str) != Some("action_needed") {
            return Err("inbox_item_not_provider_action".to_string());
        }
        if let Some(pending_choice) = item
            .get("provider_choice_pending")
            .and_then(serde_json::Value::as_str)
        {
            return if pending_choice == prompt {
                Err("provider_choice_delivery_uncertain".to_string())
            } else {
                Err("provider_choice_already_sent".to_string())
            };
        }
        if let Some(sent_choice) = item
            .get("provider_choice_sent")
            .and_then(serde_json::Value::as_str)
        {
            return if sent_choice == prompt {
                Ok(())
            } else {
                Err("provider_choice_already_sent".to_string())
            };
        }
        // Record the choice before touching the provider. A restart between this
        // write and the native dispatch must recover as explicitly uncertain,
        // never as an absent choice that can be replayed blindly.
        persisted[index]["provider_choice_pending"] = serde_json::Value::String(prompt.to_string());
        crate::utils::queue::save_items(&persisted)?;
        let result = crate::delivery::submit_live_surface_prompt(
            Some(app),
            state,
            crate::delivery::LiveSurfacePromptRequest {
                session_id: target.to_string(),
                prompt: prompt.to_string(),
                interaction_id: None,
                input_mode,
                queue_policy: wardian_core::control::QueuePolicy::LiveOnly,
                approval_action: None,
                origin: None,
                runtime_state: "live_pty_available",
                mark_prompt_started: true,
                require_provider_turn_receipt: true,
                payload_sent_detail: None,
                delivery_message_id: None,
            },
        )
        .await;
        return match result {
            Ok(_) => {
                persisted[index]["provider_choice_sent"] =
                    serde_json::Value::String(prompt.to_string());
                persisted[index]
                    .as_object_mut()
                    .expect("queue item object")
                    .remove("provider_choice_pending");
                crate::utils::queue::save_items(&persisted)?;
                Ok(())
            }
            Err(error) => {
                if error.retry_safe {
                    if let Some(item) = persisted.get_mut(index) {
                        item.as_object_mut()
                            .expect("queue item object")
                            .remove("provider_choice_pending");
                    }
                    let _ = crate::utils::queue::save_items(&persisted);
                }
                Err(error.to_string())
            }
        };
    }

    crate::control::deliver_prompt_to_agent(Some(app), state, target, prompt, input_mode)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::test_support::{HeldMutex, TestWardianHome};
    use super::*;
    use crate::state::{ActiveAgent, AgentWatchState, AppState};
    use std::sync::{Arc, Mutex};
    use wardian_core::control::WatchTranscriptMessage;
    use wardian_core::models::AgentConfig;

    #[test]
    fn remote_automation_monitor_pages_are_fixed_and_explicit() {
        let items = (0..=REMOTE_AUTOMATION_MONITOR_PAGE_SIZE).collect::<Vec<_>>();
        let (first, truncated, next_offset) = page_remote_items(items, 0);

        assert_eq!(first.len(), REMOTE_AUTOMATION_MONITOR_PAGE_SIZE);
        assert!(truncated);
        assert_eq!(next_offset, Some(REMOTE_AUTOMATION_MONITOR_PAGE_SIZE));

        let (beyond, truncated, next_offset) = page_remote_items(vec![1, 2], 10);
        assert!(beyond.is_empty());
        assert!(!truncated);
        assert_eq!(next_offset, None);
    }

    #[test]
    fn remote_automation_monitor_rejects_an_unreadable_top_level_run_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("runs-as-file");
        std::fs::write(&root, "not a directory").expect("write root fixture");

        let error = remote_automation_monitor_snapshot_from(&root, Vec::new(), 0, 0, 0)
            .expect_err("top-level run path should fail");
        assert_eq!(error, "automation_runs_unavailable");
    }

    #[test]
    fn remote_automation_monitor_skips_a_corrupt_run_item() {
        let temp = tempfile::tempdir().expect("temp dir");
        let run = temp.path().join("blueprint").join("run-corrupt");
        std::fs::create_dir_all(&run).expect("create corrupt run");
        std::fs::write(run.join("state.json"), "not json").expect("write corrupt state");

        let snapshot = remote_automation_monitor_snapshot_from(temp.path(), Vec::new(), 0, 0, 0)
            .expect("corrupt run should be skipped");
        assert!(snapshot.active_runs.is_empty());
        assert!(snapshot.recent_runs.is_empty());
    }

    #[test]
    fn remote_failure_projection_never_emits_raw_provider_or_path_details() {
        let raw = "provider failed at C:\\Users\\private\\workspace and /Users/private/workspace\u{1b}[31m\u{1b}]8;;file:///secret\u{7}";
        let projected = project_remote_failure(Some(raw.to_string()), REMOTE_RUN_FAILURE_SUMMARY)
            .expect("failure summary");

        assert_eq!(projected, REMOTE_RUN_FAILURE_SUMMARY);
        assert!(!projected.contains("private"));
        assert!(!projected.contains("provider"));
        assert!(!projected.contains('\u{1b}'));
    }

    #[test]
    fn remote_schedule_projection_omits_sensitive_invocation_state() {
        let mut assignments = wardian_core::models::AutomationAssignments::new();
        assignments.insert(
            "reviewer".to_string(),
            wardian_core::models::AutomationRoleAssignment::TemporaryProvider {
                provider: "codex".to_string(),
                workspace: Some("<absolute-workspace-path>".to_string()),
                model: Some("secret-model".to_string()),
                effort: None,
            },
        );
        let schedule = wardian_core::models::AutomationSchedule {
            id: "schedule-1".to_string(),
            blueprint_id: "release".to_string(),
            name: "Release\u{1b}[31m".to_string(),
            provider: Some("codex".to_string()),
            workspace: Some("<absolute-workspace-path>".to_string()),
            input: serde_json::json!({ "token": "do-not-return" }),
            bindings: HashMap::from([("writer".to_string(), "agent-secret".to_string())]),
            assignments,
            schedule: wardian_core::models::ScheduleDefinition {
                schedule_type: "daily".to_string(),
                active: true,
                ..Default::default()
            },
            next_run_epoch_ms: Some(42),
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: Some("failed".to_string()),
            last_run_error: Some(
                "failed at C:\\Users\\private and /Users/private\u{1b}]8;;file:///secret\u{7}"
                    .to_string(),
            ),
            last_run_epoch_ms: Some(10),
        };

        let projected = project_remote_schedule(schedule);
        let json = serde_json::to_value(projected).expect("remote schedule json");
        let text = json.to_string();

        assert_eq!(json["automation_name"], "Release");
        assert!(json.get("input").is_none());
        assert!(json.get("workspace").is_none());
        assert!(json.get("bindings").is_none());
        assert!(!text.contains("do-not-return"));
        assert!(!text.contains("absolute-workspace-path"));
        assert!(!text.contains("agent-secret"));
        assert!(!text.contains("secret-model"));
        assert!(!text.contains("Users"));
        assert!(!text.contains("file:///"));
        assert_eq!(json["last_run_error"], REMOTE_SCHEDULE_FAILURE_SUMMARY);
        assert_eq!(
            json["target_labels"],
            serde_json::json!(["reviewer · Temporary codex", "writer · Assigned"])
        );
    }

    #[test]
    fn remote_schedule_order_puts_upcoming_before_deterministic_paused_rows() {
        let schedule = |id: &str, paused: bool, next: Option<u64>, last: Option<u64>| {
            RemoteAutomationMonitorSchedule {
                id: id.to_string(),
                blueprint_id: id.to_string(),
                automation_name: id.to_string(),
                schedule: wardian_core::models::ScheduleDefinition::default(),
                next_run_epoch_ms: next,
                is_paused: paused,
                last_run_status: None,
                last_run_error: None,
                last_run_epoch_ms: last,
                target_labels: Vec::new(),
            }
        };
        let mut schedules = vec![
            schedule("paused-old", true, None, Some(1)),
            schedule("next-later", false, Some(20), None),
            schedule("paused-new", true, None, Some(2)),
            schedule("next-soon", false, Some(10), None),
        ];
        schedules.sort_by(compare_remote_schedules);

        assert_eq!(
            schedules
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec!["next-soon", "next-later", "paused-new", "paused-old"]
        );
    }

    fn test_agent(
        session_id: &str,
        session_name: &str,
        agent_class: &str,
        status: &str,
    ) -> ActiveAgent {
        ActiveAgent {
            config: Arc::new(Mutex::new(AgentConfig {
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                agent_class: agent_class.to_string(),
                provider: "mock".to_string(),
                folder: "<absolute-workspace-path>".to_string(),
                ..Default::default()
            })),
            child_process: None,
            background_processes: Vec::new(),
            memory_capability: None,
            runtime_generation: None,
            process_id: Some(1234),
            query_count: Arc::new(Mutex::new(0)),
            init_timestamp: Arc::new(Mutex::new(None)),
            last_query_timestamp: Arc::new(Mutex::new(None)),
            current_status: Arc::new(Mutex::new(status.to_string())),
            last_status_at: Arc::new(Mutex::new(None)),
            watch_state: Arc::new(Mutex::new(AgentWatchState::new(
                session_id.to_string(),
                16,
                8192,
            ))),
            terminal_title: Arc::new(Mutex::new(String::new())),
            last_output_at: Arc::new(Mutex::new(None)),
            log_path: Arc::new(Mutex::new(None)),
            log_last_modified: Arc::new(Mutex::new(None)),
            #[cfg(windows)]
            job_object: None,
        }
    }

    async fn insert_agent(state: &AppState, agent: ActiveAgent) {
        let session_id = agent.config.lock().expect("config").session_id.clone();
        state.agents.lock().await.insert(session_id, agent);
    }

    #[tokio::test]
    async fn remote_agent_roster_maps_agent_summary_fields() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Idle");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_transcript(WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "ready from transcript".to_string(),
                provider: "mock".to_string(),
                turn_id: Some("turn-1".to_string()),
                source: Some("model".to_string()),
            });
        }
        insert_agent(&state, agent).await;

        let roster = remote_agent_roster(&state).await;

        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].session_id, "agent-1");
        assert_eq!(roster[0].session_name, "CoderOne");
        assert_eq!(roster[0].agent_class, "Coder");
        assert_eq!(roster[0].provider, "mock");
        assert_eq!(roster[0].workspace, "<absolute-workspace-path>");
        assert_eq!(roster[0].status, "Idle");
        assert_eq!(roster[0].latest_text, None);
    }

    #[tokio::test]
    async fn remote_agent_roster_preserves_desktop_agent_order() {
        let state = AppState::new();
        insert_agent(&state, test_agent("agent-1", "Alpha", "Coder", "Idle")).await;
        insert_agent(
            &state,
            test_agent("agent-2", "Beta", "Reviewer", "Processing"),
        )
        .await;
        state
            .agent_order
            .lock()
            .await
            .extend(["agent-2".to_string(), "agent-1".to_string()]);

        let roster = remote_agent_roster(&state).await;

        assert_eq!(
            roster
                .iter()
                .map(|agent| agent.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["agent-2", "agent-1"]
        );
    }

    #[tokio::test]
    async fn remote_agent_roster_omits_latest_text_by_default() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(format!("\u{1b}[31m{}\u{1b}[0m", "x".repeat(5000)).as_bytes());
        }
        insert_agent(&state, agent).await;

        let roster = remote_agent_roster(&state).await;

        assert_eq!(roster[0].latest_text, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_agent_roster_returns_last_complete_snapshot_when_agent_state_is_busy() {
        let state = Arc::new(AppState::new());
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        let config = agent.config.clone();
        insert_agent(&state, agent).await;
        let initial_roster = remote_agent_roster(&state).await;
        let config_guard = HeldMutex::new(config.clone());

        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("busy roster read should not wait for the config lock");

        drop(config_guard);
        assert_eq!(roster, initial_roster);

        let agents_guard = state.agents.lock().await;
        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("busy roster read should not wait for the global agent lock");
        drop(agents_guard);
        assert_eq!(roster, initial_roster);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_agent_roster_keeps_live_status_when_config_is_busy() {
        let state = Arc::new(AppState::new());
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        let config = agent.config.clone();
        let status = agent.current_status.clone();
        insert_agent(&state, agent).await;
        let _ = remote_agent_roster(&state).await;
        *status.lock().expect("status") = "Idle".to_string();
        let config_guard = HeldMutex::new(config.clone());

        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("busy config read should not wait for the config lock");

        drop(config_guard);
        assert_eq!(roster[0].status, "Idle");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_agent_roster_uses_cached_status_when_global_state_is_busy() {
        let state = Arc::new(AppState::new());
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Restoring");
        let status = agent.current_status.clone();
        insert_agent(&state, agent).await;
        let initial_roster = remote_agent_roster(&state).await;
        *status.lock().expect("status") = "Idle".to_string();
        let refreshed_roster = remote_agent_roster(&state).await;

        let agents_guard = state.agents.lock().await;
        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("cached roster read should not wait for the global agent lock");
        drop(agents_guard);

        assert_eq!(initial_roster[0].status, "Restoring");
        assert_eq!(refreshed_roster[0].status, "Idle");
        assert_eq!(roster[0].status, "Idle");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_agent_roster_keeps_other_agents_live_when_one_status_is_busy() {
        let state = Arc::new(AppState::new());
        let busy = test_agent("agent-1", "Busy", "Coder", "Processing");
        let busy_status = busy.current_status.clone();
        let live = test_agent("agent-2", "Live", "Coder", "Idle");
        let live_status = live.current_status.clone();
        insert_agent(&state, busy).await;
        insert_agent(&state, live).await;
        let initial_roster = remote_agent_roster(&state).await;
        let busy_guard = HeldMutex::new(busy_status.clone());
        *live_status.lock().expect("live status") = "Action Needed".to_string();

        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("partial roster read should not wait for one status lock");

        drop(busy_guard);
        assert_eq!(
            roster
                .iter()
                .find(|agent| agent.session_id == "agent-1")
                .expect("busy agent")
                .status,
            initial_roster
                .iter()
                .find(|agent| agent.session_id == "agent-1")
                .expect("initial busy agent")
                .status
        );
        assert_eq!(
            roster
                .iter()
                .find(|agent| agent.session_id == "agent-2")
                .expect("live agent")
                .status,
            "Action Needed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_agent_roster_uses_persisted_config_without_waiting_for_live_state() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        let settings = temp.path().join("settings");
        std::fs::create_dir_all(&settings).expect("settings dir");
        let persisted_config = AgentConfig {
            session_id: "agent-1".to_string(),
            session_name: "CoderOne".to_string(),
            agent_class: "Coder".to_string(),
            provider: "mock".to_string(),
            folder: "<absolute-workspace-path>".to_string(),
            ..Default::default()
        };
        std::fs::write(
            settings.join("state.json"),
            serde_json::to_string(&vec![persisted_config]).expect("serialize config"),
        )
        .expect("persist config");

        let previous_home = std::env::var_os("WARDIAN_HOME");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let state = Arc::new(AppState::new());
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        let config = agent.config.clone();
        insert_agent(&state, agent).await;
        let config_guard = HeldMutex::new(config.clone());

        let roster = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            remote_agent_roster(&state),
        )
        .await
        .expect("persisted roster read should not wait for the config lock");

        drop(config_guard);
        match previous_home {
            Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
            None => unsafe { std::env::remove_var("WARDIAN_HOME") },
        }

        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].session_id, "agent-1");
        assert_eq!(roster[0].status, "Processing");
    }

    #[tokio::test]
    async fn remote_watchlist_state_reads_persisted_state_and_prefs() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        let watchlists_dir = temp.path().join("watchlists");
        std::fs::create_dir_all(&watchlists_dir).expect("watchlists dir");
        std::fs::write(
            watchlists_dir.join("index.json"),
            serde_json::json!({
                "version": 2,
                "teams": [{ "id": "team-1", "name": "Core", "agentIds": ["agent-2", "agent-1"] }],
                "watchlists": [{ "id": "main", "name": "Main", "entries": [{ "type": "team", "teamId": "team-1" }] }]
            })
            .to_string(),
        )
        .expect("watchlist json");
        std::fs::write(
            watchlists_dir.join("prefs.json"),
            serde_json::json!({
                "columns": [],
                "sort": null,
                "preserve_team_grouping_when_sorted": false,
                "collapsed_team_ids": ["team-1"]
            })
            .to_string(),
        )
        .expect("prefs json");

        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let response = remote_watchlist_state().expect("watchlist response");
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(response.watchlists[0]["id"], "main");
        assert_eq!(response.teams[0]["agentIds"][0], "agent-2");
        assert_eq!(
            response.prefs.as_ref().expect("prefs")["collapsed_team_ids"][0],
            "team-1"
        );
    }

    #[tokio::test]
    async fn remote_watchlist_state_uses_empty_defaults_for_missing_or_bad_files() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        std::fs::create_dir_all(temp.path().join("watchlists")).expect("watchlists dir");
        std::fs::write(temp.path().join("watchlists/index.json"), "{").expect("bad index");
        std::fs::write(temp.path().join("watchlists/prefs.json"), "{").expect("bad prefs");

        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let response = remote_watchlist_state().expect("watchlist response");
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(response.watchlists, serde_json::json!([]));
        assert_eq!(response.teams, serde_json::json!([]));
        assert!(response.prefs.is_none());
    }

    #[tokio::test]
    async fn remote_queue_items_reads_the_desktop_inbox_file() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        let queue_dir = temp.path().join("queue");
        std::fs::create_dir_all(&queue_dir).expect("queue dir");
        std::fs::write(
            queue_dir.join("items.json"),
            serde_json::json!([{
                "id": "desktop-inbox-1",
                "type": "approval_request",
                "timestamp": chrono::Utc::now().timestamp_millis(),
            }])
            .to_string(),
        )
        .expect("queue json");

        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let items = remote_queue_items(&AppState::new()).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(items[0]["id"], "desktop-inbox-1");
    }

    #[tokio::test]
    async fn remote_queue_items_keeps_persisted_workflow_completions() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        crate::utils::queue::save_items(&[serde_json::json!({
            "id": "workflow-completion:release:run-1",
            "type": "workflow_completed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "workflow_id": "release",
            "workflow_run_id": "run-1",
            "status": "completed",
        })])
        .expect("workflow completion");

        let items = remote_queue_items(&AppState::new()).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(items[0]["id"], "workflow-completion:release:run-1");
    }

    #[tokio::test]
    async fn remote_queue_items_merges_cached_runtime_projection() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        crate::utils::queue::save_items(&[serde_json::json!({
            "id": "desktop-item",
            "type": "agent_completed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
        })])
        .expect("desktop item");
        let state = AppState::new();
        state.set_remote_inbox_runtime_items(
            0,
            vec![serde_json::json!({
                "id": "automation-item",
                "type": "automation_completed",
                "timestamp": chrono::Utc::now().timestamp_millis() + 1,
            })],
        );

        let items = remote_queue_items(&state).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(
            items
                .iter()
                .map(|item| item["id"].as_str().expect("item id"))
                .collect::<Vec<_>>(),
            vec!["automation-item", "desktop-item"]
        );
    }

    #[test]
    fn remote_runtime_refresh_is_single_flight_and_interval_bounded() {
        let state = AppState::new();
        let generation = state
            .try_start_remote_inbox_runtime_refresh()
            .expect("first refresh should start");
        assert!(state.try_start_remote_inbox_runtime_refresh().is_none());
        state.set_remote_inbox_runtime_items(generation, Vec::new());
        assert!(state.try_start_remote_inbox_runtime_refresh().is_none());
    }

    #[test]
    fn remote_runtime_refresh_invalidation_rejects_stale_results() {
        let state = AppState::new();
        let generation = state
            .try_start_remote_inbox_runtime_refresh()
            .expect("refresh should start");
        state
            .set_remote_inbox_runtime_items(generation, vec![serde_json::json!({ "id": "stale" })]);
        assert!(state.remote_inbox_runtime_items().is_some());

        state.invalidate_remote_inbox_runtime();
        assert!(state.remote_inbox_runtime_items().is_none());
        state.set_remote_inbox_runtime_items(
            generation,
            vec![serde_json::json!({ "id": "rejected" })],
        );
        assert!(state.remote_inbox_runtime_items().is_none());
    }

    #[test]
    fn inbox_page_reports_more_items_at_the_cursor_cap() {
        let (items, truncated, next_offset) = page_after_lookahead(
            vec![serde_json::json!({ "id": "lookahead" })],
            wardian_core::control::MAX_INBOX_OFFSET - 200,
            200,
        );

        assert_eq!(items[0]["id"], "lookahead");
        assert!(truncated);
        assert!(next_offset.is_none());
    }

    #[tokio::test]
    async fn pending_provider_choice_survives_reload_as_uncertain() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        crate::utils::queue::save_items(&[serde_json::json!({
            "id": "action-1",
            "type": "action_needed",
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "read": false,
            "agent_session_id": "agent-1",
            "summary": "Proceed?\n1. Yes",
            "provider_choice_pending": "1",
        })])
        .expect("pending queue");

        let items = remote_queue_items(&AppState::new()).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        assert_eq!(items[0]["provider_choice_pending"], "1");
        assert!(provider_choice_acknowledgement_unresolved(&items[0]));
    }

    #[tokio::test]
    async fn concurrent_queue_mutations_preserve_both_updates_and_valid_json() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        crate::utils::queue::save_items(&[
            serde_json::json!({ "id": "first", "read": false }),
            serde_json::json!({ "id": "second", "read": false }),
        ])
        .expect("initial queue");

        let state = Arc::new(AppState::new());
        let start = Arc::new(tokio::sync::Barrier::new(2));
        let first_state = Arc::clone(&state);
        let first_start = Arc::clone(&start);
        let first = tokio::spawn(async move {
            first_start.wait().await;
            let _queue_guard = first_state.queue_io_lock.lock().await;
            let mut items = persisted_queue_items();
            tokio::task::yield_now().await;
            items[0]["read"] = serde_json::Value::Bool(true);
            save_persisted_queue_items(&items).expect("first queue update");
        });
        let second_state = Arc::clone(&state);
        let second_start = Arc::clone(&start);
        let second = tokio::spawn(async move {
            second_start.wait().await;
            let _queue_guard = second_state.queue_io_lock.lock().await;
            let mut items = persisted_queue_items();
            tokio::task::yield_now().await;
            items[1]["read"] = serde_json::Value::Bool(true);
            save_persisted_queue_items(&items).expect("second queue update");
        });
        first.await.expect("first task");
        second.await.expect("second task");

        let items = persisted_queue_items();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert_eq!(items[0]["read"], true);
        assert_eq!(items[1]["read"], true);
    }

    #[tokio::test]
    async fn desktop_snapshot_merge_preserves_remote_triage() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let initial = vec![
            serde_json::json!({ "id": "first", "read": false, "provider_choice_sent": "1" }),
            serde_json::json!({ "id": "second", "read": false }),
        ];
        crate::utils::queue::save_items(&initial).expect("initial queue");
        let state = AppState::new();
        *state.queue_loaded_snapshot.lock().await = Some(initial.clone());

        {
            let _queue_guard = state.queue_io_lock.lock().await;
            let mut remote_items = crate::utils::queue::load_items();
            remote_items[0]["read"] = serde_json::Value::Bool(true);
            crate::utils::queue::save_items(&remote_items).expect("remote queue update");
        }

        {
            let _queue_guard = state.queue_io_lock.lock().await;
            let latest = crate::utils::queue::load_items();
            let base = state.queue_loaded_snapshot.lock().await.clone();
            let desktop_snapshot = vec![
                serde_json::json!({ "id": "first", "read": false }),
                serde_json::json!({ "id": "second", "read": true }),
            ];
            let merged = crate::utils::queue::merge_desktop_snapshot(
                base.as_deref(),
                &desktop_snapshot,
                &latest,
            );
            crate::utils::queue::save_items(&merged).expect("merged queue update");
        }

        let items = crate::utils::queue::load_items();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert_eq!(items[0]["read"], true);
        assert_eq!(items[1]["read"], true);
        assert_eq!(items[0]["provider_choice_sent"], "1");
    }

    #[tokio::test]
    async fn desktop_save_without_load_baseline_preserves_remote_triage() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let initial = vec![serde_json::json!({ "id": "first", "read": false })];
        crate::utils::queue::save_items(&initial).expect("initial queue");
        let state = AppState::new();

        {
            let _queue_guard = state.queue_io_lock.lock().await;
            let mut remote_items = crate::utils::queue::load_items();
            remote_items[0]["read"] = serde_json::Value::Bool(true);
            crate::utils::queue::save_items(&remote_items).expect("remote queue update");
        }

        let latest = crate::utils::queue::load_items();
        let desktop_snapshot = vec![serde_json::json!({ "id": "first", "read": false })];
        let merged = crate::utils::queue::merge_desktop_snapshot(None, &desktop_snapshot, &latest);
        crate::utils::queue::save_items(&merged).expect("desktop queue update");

        let items = crate::utils::queue::load_items();
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert_eq!(items[0]["read"], true);
    }

    #[tokio::test]
    async fn baseline_less_desktop_save_does_not_resurrect_remote_dismissal() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        let initial = vec![serde_json::json!({ "id": "dismissed", "read": false })];
        crate::utils::queue::save_items(&initial).expect("initial queue");
        let state = AppState::new();

        {
            let _queue_guard = state.queue_io_lock.lock().await;
            crate::utils::queue::save_items(&[]).expect("remote dismissal");
        }

        let latest = crate::utils::queue::load_items();
        let desktop_snapshot = initial;
        let merged = crate::utils::queue::merge_desktop_snapshot(None, &desktop_snapshot, &latest);
        crate::utils::queue::save_items(&merged).expect("desktop queue update");

        assert!(crate::utils::queue::load_items().is_empty());
        unsafe { std::env::remove_var("WARDIAN_HOME") };
    }

    #[test]
    fn pending_approval_guard_covers_automation_and_manual_approvals() {
        assert!(is_pending_approval(&serde_json::json!({
            "automation_approval": { "run_id": "run-1" }
        })));
        assert!(is_pending_approval(&serde_json::json!({
            "type": "approval_request",
            "notification_status": "awaiting_reply"
        })));
        assert!(!is_pending_approval(&serde_json::json!({
            "type": "approval_request",
            "notification_status": "completed"
        })));
    }

    #[test]
    fn remote_mark_read_guard_rejects_pending_provider_choice() {
        let error = validate_remote_mark_read(&serde_json::json!({
            "type": "action_needed",
            "read": false,
            "provider_choice_pending": "1"
        }))
        .expect_err("pending provider choice must remain unread");
        assert_eq!(
            error,
            "provider_choice_delivery_uncertain_cannot_be_marked_read"
        );

        assert!(validate_remote_mark_read(&serde_json::json!({
            "type": "action_needed",
            "read": false,
            "provider_choice_sent": "1"
        }))
        .is_ok());
    }

    #[test]
    fn clear_read_only_targets_legacy_completion_items() {
        assert!(is_clearable_legacy_completion(&serde_json::json!({
            "type": "agent_completed"
        })));
        assert!(is_clearable_legacy_completion(&serde_json::json!({
            "type": "automation_completed"
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "action_needed"
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "agent_update"
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "agent_completed",
            "inbox_notification_id": "notice-1"
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "automation_completed",
            "read": true,
            "provider_choice_pending": "1"
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "automation_completed",
            "read": true,
            "dismissed": true
        })));
        assert!(!is_clearable_legacy_completion(&serde_json::json!({
            "type": "agent_completed",
            "read": false,
            "provider_choice_sent": "1"
        })));
    }

    #[test]
    fn automation_dismissal_marker_preserves_run_identity() {
        let marker = automation_dismissal_marker(&serde_json::json!({
            "type": "workflow_completed",
            "workflow_id": "release",
            "workflow_run_id": "run-1",
        }))
        .expect("workflow item should produce a dismissal marker");

        assert_eq!(marker["id"], "workflow-dismissed:release:run-1");
        assert_eq!(marker["dismissed"], true);
        assert_eq!(
            automation_identity(&marker),
            Some(("release".to_string(), "run-1".to_string(),))
        );
    }

    #[tokio::test]
    async fn remote_queue_items_projects_live_inbox_notifications() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize state db");
        let state = AppState::new();
        let notification = state
            .interactions
            .create_notification_durable(
                "agent-1".to_string(),
                wardian_core::control::InboxNotificationPayload {
                    kind: InboxNotificationKind::Approval,
                    title: "Approve deployment".to_string(),
                    body: "Deploy the reviewed change.".to_string(),
                    proposed_action: Some("Deploy".to_string()),
                    risk: Some("Changes production".to_string()),
                    choices: vec!["Approve".to_string(), "Reject".to_string()],
                    expires_at: None,
                },
            )
            .await
            .expect("create notification");

        let items = remote_queue_items(&state).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };

        let item = items
            .iter()
            .find(|item| item["inbox_notification_id"] == notification.id)
            .expect("live inbox notification");
        assert_eq!(item["type"], "approval_request");
        assert_eq!(item["evidence_source"], "interaction_store");
        assert_eq!(item["notification_title"], "Approve deployment");
        assert_eq!(
            item["approval_choices"],
            serde_json::json!(["Approve", "Reject"])
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_mutation_lookup_waits_for_authoritative_notifications() {
        let _guard = crate::utils::wardian_test_env_lock_async().await;
        let temp = tempfile::tempdir().expect("temp home");
        unsafe { std::env::set_var("WARDIAN_HOME", temp.path()) };
        wardian_core::db::init_db_at_path(&temp.path().join("state.db"))
            .expect("initialize state db");
        let state = Arc::new(AppState::new());
        let first = state
            .interactions
            .create_notification_durable(
                "agent-1".to_string(),
                wardian_core::control::InboxNotificationPayload {
                    kind: InboxNotificationKind::Update,
                    title: "First update".to_string(),
                    body: "The first update is ready.".to_string(),
                    proposed_action: None,
                    risk: None,
                    choices: Vec::new(),
                    expires_at: None,
                },
            )
            .await
            .expect("create first notification");

        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let blocker_state = Arc::clone(&state);
        let blocker_entered = Arc::clone(&entered);
        let blocker_release = Arc::clone(&release);
        let blocker = tokio::spawn(async move {
            let _records = blocker_state.interactions.records.lock().await;
            blocker_entered.notify_one();
            blocker_release.notified().await;
        });
        entered.notified().await;

        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(50),
            remote_queue_items_for_mutation(&state),
        )
        .await
        .is_err());
        release.notify_one();
        blocker.await.expect("release first records lock");
        let items = remote_queue_items_for_mutation(&state).await;
        assert!(items
            .iter()
            .any(|item| { item["id"] == format!("notification:{}", first.id) }));

        let second = state
            .interactions
            .create_notification_durable(
                "agent-1".to_string(),
                wardian_core::control::InboxNotificationPayload {
                    kind: InboxNotificationKind::Update,
                    title: "Second update".to_string(),
                    body: "The second update is ready.".to_string(),
                    proposed_action: None,
                    risk: None,
                    choices: Vec::new(),
                    expires_at: None,
                },
            )
            .await
            .expect("create second notification");
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let blocker_state = Arc::clone(&state);
        let blocker_entered = Arc::clone(&entered);
        let blocker_release = Arc::clone(&release);
        let blocker = tokio::spawn(async move {
            let _records = blocker_state.interactions.records.lock().await;
            blocker_entered.notify_one();
            blocker_release.notified().await;
        });
        entered.notified().await;

        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(50),
            remote_queue_items_for_mutation(&state),
        )
        .await
        .is_err());
        release.notify_one();
        blocker.await.expect("release second records lock");
        let items = remote_queue_items_for_mutation(&state).await;
        unsafe { std::env::remove_var("WARDIAN_HOME") };
        assert!(items
            .iter()
            .any(|item| { item["id"] == format!("notification:{}", first.id) }));
        assert!(items
            .iter()
            .any(|item| { item["id"] == format!("notification:{}", second.id) }));
    }

    #[tokio::test]
    async fn remote_agent_chat_transcript_returns_normalized_messages() {
        let _home = TestWardianHome::new_async().await;
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Idle");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_transcript(WatchTranscriptMessage {
                role: "assistant".to_string(),
                text: "Use the shared chat transcript model.".to_string(),
                provider: "mock".to_string(),
                turn_id: Some("turn-1".to_string()),
                source: Some("model".to_string()),
            });
        }
        insert_agent(&state, agent).await;

        let transcript = remote_agent_chat_transcript(&state, "agent-1")
            .await
            .expect("remote chat transcript");

        assert!(transcript.iter().any(|event| {
            event.kind == wardian_core::models::chat::AgentChatEventKind::Message
                && event.role == Some(wardian_core::models::chat::AgentChatRole::Assistant)
                && event.text.as_deref() == Some("Use the shared chat transcript model.")
        }));
    }

    #[test]
    fn unresolved_provider_choice_cannot_be_dismissed() {
        assert!(provider_choice_acknowledgement_unresolved(
            &serde_json::json!({
                "provider_choice_pending": "1",
                "read": false
            })
        ));
        assert!(provider_choice_acknowledgement_unresolved(
            &serde_json::json!({
                "provider_choice_sent": "1",
                "read": false
            })
        ));
        assert!(!provider_choice_acknowledgement_unresolved(
            &serde_json::json!({
                "provider_choice_sent": "1",
                "read": true
            })
        ));
    }

    #[test]
    fn remote_agent_chat_pages_keep_the_newest_events_and_a_stable_older_cursor() {
        let events: Vec<AgentChatEvent> = (1..=85)
            .map(|sequence| AgentChatEvent {
                id: format!("event-{sequence}"),
                session_id: "agent-1".to_string(),
                provider: "mock".to_string(),
                kind: wardian_core::models::chat::AgentChatEventKind::Message,
                role: Some(wardian_core::models::chat::AgentChatRole::Assistant),
                text: Some(format!("Message {sequence}")),
                title: None,
                status: None,
                turn_id: None,
                source: None,
                command: None,
                exit_code: None,
                path: None,
                language: None,
                created_at: None,
                sequence: Some(sequence),
                metadata: serde_json::json!({}),
            })
            .collect();

        let latest = page_remote_agent_chat_events(events.clone(), None, 40);

        assert_eq!(latest.events.len(), 40);
        assert_eq!(
            latest.events.first().map(|event| event.id.as_str()),
            Some("event-46")
        );
        assert_eq!(
            latest.events.last().map(|event| event.id.as_str()),
            Some("event-85")
        );
        assert!(latest.has_older);
        assert_eq!(latest.next_before, Some(45));

        let older = page_remote_agent_chat_events(events.clone(), latest.next_before, 40);
        assert_eq!(older.events.len(), 40);
        assert_eq!(
            older.events.first().map(|event| event.id.as_str()),
            Some("event-6")
        );
        assert_eq!(
            older.events.last().map(|event| event.id.as_str()),
            Some("event-45")
        );
        assert!(older.has_older);
        assert_eq!(older.next_before, Some(5));

        let first_page = page_remote_agent_chat_events(events, older.next_before, 40);
        assert_eq!(first_page.events.len(), 5);
        assert_eq!(
            first_page.events.first().map(|event| event.id.as_str()),
            Some("event-1")
        );
        assert!(!first_page.has_older);
        assert_eq!(first_page.next_before, None);
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_returns_sanitized_output_without_draining() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"\x1b[31mred terminal\x1b[0m\nsecond line");
        }
        insert_agent(&state, agent).await;

        let first = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(4096))
            .await
            .expect("first terminal snapshot");
        let second = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(4096))
            .await
            .expect("second terminal snapshot");

        assert_eq!(first.text, "red terminal\nsecond line");
        assert_eq!(second.text, first.text);
        assert_eq!(second.cursor, first.cursor);
        assert!(!first.truncated);
        assert_eq!(first.omitted_bytes, 0);
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_respects_tail_bytes() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"alpha beta gamma");
        }
        insert_agent(&state, agent).await;

        let snapshot = remote_agent_terminal_snapshot(&state, "agent-1", None, Some(5))
            .await
            .expect("bounded terminal snapshot");

        assert_eq!(snapshot.text, "gamma");
        assert!(snapshot.truncated);
        assert!(snapshot.omitted_bytes > 0);
    }

    #[tokio::test]
    async fn remote_agent_terminal_raw_output_preserves_escape_sequences_without_draining() {
        let state = AppState::new();
        let agent = test_agent("agent-1", "CoderOne", "Coder", "Processing");
        {
            let mut watch = agent.watch_state.lock().expect("watch state");
            watch.push_output(b"\x1b[31mred terminal\x1b[0m\nsecond line");
        }
        insert_agent(&state, agent).await;

        let first = remote_agent_terminal_raw_output(&state, "agent-1", Some(4096))
            .await
            .expect("first raw terminal output");
        let second = remote_agent_terminal_raw_output(&state, "agent-1", Some(4096))
            .await
            .expect("second raw terminal output");

        assert_eq!(first, "\x1b[31mred terminal\x1b[0m\nsecond line");
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn remote_agent_terminal_snapshot_rejects_unknown_agent() {
        let state = AppState::new();

        assert_eq!(
            remote_agent_terminal_snapshot(&state, "missing-agent", None, Some(4096))
                .await
                .unwrap_err(),
            "agent_not_found"
        );
    }

    #[test]
    fn run_remote_agent_action_rejects_unknown_actions_before_dispatch() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "open_shell".to_string(),
            target: "agent-1".to_string(),
            prompt: None,
            input_mode: None,
            inbox_item_id: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_agent_action"
        );
    }

    #[test]
    fn remote_send_prompt_accepts_command_mode() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "send_prompt".to_string(),
            target: "agent-1".to_string(),
            prompt: Some("/status".to_string()),
            input_mode: Some(wardian_core::control::MessageInputMode::Command),
            inbox_item_id: None,
        };

        validate_remote_agent_action(&request).expect("command mode should be accepted");
    }

    #[test]
    fn remote_send_prompt_rejects_approval_action_mode() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "send_prompt".to_string(),
            target: "agent-1".to_string(),
            prompt: Some("1".to_string()),
            input_mode: Some(wardian_core::control::MessageInputMode::ApprovalAction),
            inbox_item_id: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_input_mode"
        );
    }

    #[test]
    fn remote_agent_action_rejects_clone() {
        let request = crate::remote::models::RemoteAgentActionRequest {
            action: "clone".to_string(),
            target: "agent-1".to_string(),
            prompt: None,
            input_mode: None,
            inbox_item_id: None,
        };

        assert_eq!(
            validate_remote_agent_action(&request).unwrap_err(),
            "unsupported_remote_agent_action"
        );
    }
}
