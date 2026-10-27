//! Native Claude turn-ended observations. These are never success verdicts.
//!
//! The session-scoped mod captures the provider event into a durable outbox.
//! Wardian admits receipts only from the current launch, then persists an
//! Inbox notice before acknowledging. Only admitted receipts may replay after
//! restart; an unobserved or stale producer file cannot become a completion.

use crate::manager::turn_completion::{self, PersistedCompletion};
use crate::state::AppState;
use crate::utils::logging::log_debug;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use wardian_core::models::AgentConfig;

const MOD_SOURCE: &str = include_str!("claude_turn_ended/register.js");
const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    schema: u8,
    wardian_session_id: String,
    launch_id: String,
    accepted_sessions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u8,
    wardian_session_id: String,
    launch_id: String,
    provider_session_id: String,
    turn_id: String,
    agent_id: Option<String>,
    prompt_origin: String,
    prompt_text: String,
    start_text: String,
    answer: String,
    is_aborted: bool,
    reason: Option<String>,
    started_at_ms: i64,
    captured_at_ms: i64,
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl Receipt {
    fn valid_for(&self, launch: &Launch) -> bool {
        self.schema == 1
            && launch.schema == 1
            && self.wardian_session_id == launch.wardian_session_id
            && self.launch_id == launch.launch_id
            && launch.accepted_sessions.contains(&self.provider_session_id)
            && safe_id(&self.provider_session_id)
            && safe_id(&self.turn_id)
            && self.agent_id.is_none()
            && matches!(self.prompt_origin.as_str(), "composer" | "bridge")
            && !self.prompt_text.trim().is_empty()
            && self.prompt_text == self.start_text
            && self.started_at_ms > 0
            && self.captured_at_ms >= self.started_at_ms
    }

    fn item(&self) -> serde_json::Value {
        let outcome = if self.is_aborted {
            "aborted"
        } else {
            "unknown"
        };
        // `reason=answer` can also mean stop_hook_prevented. Keep the raw
        // provider reason as evidence; do not promote it to success or error.
        let detail = if self.is_aborted {
            "Turn interrupted"
        } else {
            "Outcome unverified"
        };
        let summary = if self.answer.trim().is_empty() {
            detail.to_string()
        } else {
            format!(
                "{detail}\n{}",
                self.answer.chars().take(500).collect::<String>()
            )
        };
        serde_json::json!({
            "id": format!("agent-turn-ended:{}:{}:{}:{}", self.wardian_session_id,
                self.launch_id, self.provider_session_id, self.turn_id),
            "type": "agent_turn_ended",
            "timestamp": self.captured_at_ms,
            "timestamp_source": "claude_mod_turn_complete",
            "read": false,
            "agent_session_id": self.wardian_session_id,
            "agent_name": "",
            "evidence_id": format!("{}:{}:{}", self.launch_id, self.provider_session_id, self.turn_id),
            "evidence_source": "provider_runtime",
            "provider_session_id": self.provider_session_id,
            "provider_turn_id": self.turn_id,
            "turn_outcome": outcome,
            "provider_reason": self.reason,
            "summary": summary,
            "response_text": self.answer,
        })
    }
}

fn root(session_id: &str) -> Option<PathBuf> {
    safe_id(session_id)
        .then(crate::utils::fs::get_wardian_home)?
        .map(|home| {
            home.join("agents")
                .join(session_id)
                .join("claude")
                .join("turn-ended")
        })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Publishes a complete file atomically. Incomplete producer writes are never
/// accepted as evidence and incomplete admission writes never become replayable.
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let temp = path.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

/// Owns only this launch's passive plugin and receipt directory. Preparing it
/// changes no provider settings, authentication, workspace files, or global plugins.
pub(crate) struct TurnEndedLaunch {
    dir: PathBuf,
    launch: Launch,
}

impl TurnEndedLaunch {
    pub(crate) fn prepare(config: &AgentConfig) -> Result<Self, String> {
        let launch = Launch {
            schema: 1,
            wardian_session_id: config.session_id.clone(),
            launch_id: uuid::Uuid::new_v4().simple().to_string(),
            accepted_sessions: super::claude::claude_accepted_sessions(config),
        };
        let dir = root(&config.session_id)
            .ok_or("Claude turn receipt home unavailable")?
            .join(&launch.launch_id);
        let plugin = dir.join("plugin");
        std::fs::create_dir_all(plugin.join(".claude-plugin"))
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(plugin.join("hooks")).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(dir.join("receipts")).map_err(|error| error.to_string())?;
        write_json(&dir.join("launch.json"), &launch)?;
        write_json(
            &dir.join("coverage.json"),
            &serde_json::json!({
                "available": false, "reason": "awaiting_provider_mod_handshake",
            }),
        )?;
        write_json(
            &plugin.join(".claude-plugin/plugin.json"),
            &serde_json::json!({
                "name": "wardian-turn-ended", "version": "1.0.0",
                "description": "Passive native turn-ended Inbox observations",
                "author": { "name": "Wardian" },
            }),
        )?;
        write_json(
            &plugin.join("hooks/hooks.json"),
            &serde_json::json!({
                "modules": ["./register.js"],
            }),
        )?;
        let binding = serde_json::json!({
            "wardian_session_id": launch.wardian_session_id,
            "launch_id": launch.launch_id,
            "directory": dir.join("receipts").to_string_lossy(),
        });
        std::fs::write(
            plugin.join("hooks/register.js"),
            format!("const wardianBridge = {binding};\n{MOD_SOURCE}"),
        )
        .map_err(|error| error.to_string())?;
        Ok(Self { dir, launch })
    }

    pub(crate) fn plugin_dir(&self) -> PathBuf {
        self.dir.join("plugin")
    }

    pub(crate) fn observe(self, app: AppHandle, generation: u64, status: Arc<Mutex<String>>) {
        tauri::async_runtime::spawn(async move {
            let mut attached = false;
            let mut acknowledged = HashSet::new();
            let mut ready = false;
            loop {
                if status.lock().is_ok_and(|value| value.as_str() == "Off") {
                    break;
                }
                let state = app.state::<AppState>();
                let current = {
                    let agents = state.agents.lock().await;
                    agents
                        .get(&self.launch.wardian_session_id)
                        .is_some_and(|agent| {
                            agent.runtime_generation == Some(generation)
                                && Arc::ptr_eq(&agent.current_status, &status)
                        })
                };
                if attached && !current {
                    break;
                }
                attached |= current;
                if current {
                    if !ready {
                        if let Some(handshake) =
                            read_json::<serde_json::Value>(&self.dir.join("receipts/ready.json"))
                        {
                            ready = handshake["launch_id"] == self.launch.launch_id
                                && handshake["wardian_session_id"]
                                    == self.launch.wardian_session_id
                                && handshake["provider_session_id"].as_str().is_some_and(|id| {
                                    self.launch
                                        .accepted_sessions
                                        .iter()
                                        .any(|accepted| accepted == id)
                                });
                            if ready {
                                let _ = write_json(
                                    &self.dir.join("coverage.json"),
                                    &serde_json::json!({ "available": true, "source": "native_mod_handshake" }),
                                );
                            }
                        }
                    }
                    if ready {
                        for receipt in receipts_filtered(
                            &self.dir,
                            &self.launch,
                            "notice-",
                            Some(&acknowledged),
                        ) {
                            if acknowledged.contains(&receipt.turn_id) {
                                continue;
                            }
                            // Admission and runtime validation share the lifecycle
                            // lock. Startup replays this durable admission only.
                            let lifecycle = state
                                .lock_agent_lifecycle(&self.launch.wardian_session_id)
                                .await;
                            let still_current = state
                                .agents
                                .lock()
                                .await
                                .get(&self.launch.wardian_session_id)
                                .is_some_and(|agent| {
                                    agent.runtime_generation == Some(generation)
                                        && Arc::ptr_eq(&agent.current_status, &status)
                                });
                            if !still_current {
                                drop(lifecycle);
                                continue;
                            }
                            let admitted =
                                self.dir.join(format!("admitted-{}.json", receipt.turn_id));
                            let admitted_ok = write_json(&admitted, &receipt).is_ok();
                            drop(lifecycle);
                            if admitted_ok
                                && persist(&app, &state, &self.dir, &receipt, Some(generation))
                                    .await
                            {
                                acknowledged.insert(receipt.turn_id.clone());
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
    }
}

fn receipts(dir: &Path, launch: &Launch, prefix: &str) -> Vec<Receipt> {
    receipts_filtered(dir, launch, prefix, None)
}

fn receipts_filtered(
    dir: &Path,
    launch: &Launch,
    prefix: &str,
    excluded: Option<&HashSet<String>>,
) -> Vec<Receipt> {
    let source = if prefix == "notice-" {
        dir.join("receipts")
    } else {
        dir.to_path_buf()
    };
    std::fs::read_dir(source)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) || !name.ends_with(".json") {
                return None;
            }
            let turn_id = &name[prefix.len()..name.len() - ".json".len()];
            if excluded.is_some_and(|ids| ids.contains(turn_id)) {
                return None;
            }
            let receipt = read_json::<Receipt>(&entry.path())?;
            (receipt.valid_for(launch) && name == format!("{prefix}{}.json", receipt.turn_id))
                .then_some(receipt)
        })
        .collect()
}

async fn persist<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    dir: &Path,
    receipt: &Receipt,
    generation: Option<u64>,
) -> bool {
    let PersistedCompletion::Committed {
        agent_name,
        item_id,
    } = turn_completion::persist_turn_notice(
        state,
        &receipt.wardian_session_id,
        "claude",
        generation,
        receipt.item(),
    )
    .await
    else {
        return false;
    };
    // UI projection never runs completion watches, agent-turn-completed, change
    // snapshots, or successful automation triggers for these notices.
    turn_completion::emit_active_completion(
        app,
        state,
        &receipt.wardian_session_id,
        &agent_name,
        &item_id,
    )
    .await;
    let ack = dir
        .join("receipts")
        .join(format!("notice-{}.ack", receipt.turn_id));
    if let Err(error) = write_json(&ack, &serde_json::json!({ "id": item_id })) {
        log_debug(&format!(
            "[Wardian] Claude turn-ended acknowledgement pending: {error}"
        ));
        return false;
    }
    true
}

/// Replays backend-admitted notices, including saved-Off sessions. Raw producer
/// files from retired launches have no admission and are deliberately ignored.
pub(crate) fn replay<R: Runtime>(app: &AppHandle<R>, config: &AgentConfig) {
    let app = app.clone();
    let config = config.clone();
    // Queue persistence may retry. Startup publication must not wait for it
    // while the normal desktop queue/agent restoration is still settling.
    tauri::async_runtime::spawn(async move {
        replay_pending(&app, &config).await;
    });
}

async fn replay_pending<R: Runtime>(app: &AppHandle<R>, config: &AgentConfig) {
    let Some(root) = root(&config.session_id) else {
        return;
    };
    let state = app.state::<AppState>();
    let accepted = super::claude::claude_accepted_sessions(config);
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if !entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            continue;
        }
        let dir = entry.path();
        let Some(launch) = read_json::<Launch>(&dir.join("launch.json")) else {
            continue;
        };
        if launch.wardian_session_id != config.session_id || launch.schema != 1 {
            continue;
        }
        for receipt in receipts(&dir, &launch, "admitted-") {
            if !accepted.contains(&receipt.provider_session_id) {
                continue;
            }
            let ack = dir
                .join("receipts")
                .join(format!("notice-{}.ack", receipt.turn_id));
            if read_json::<serde_json::Value>(&ack)
                .is_some_and(|ack| ack["id"] == receipt.item()["id"])
            {
                continue;
            }
            if persist(app, &state, &dir, &receipt, None).await {
                let _ = app.emit("inbox-updated", ());
            }
        }
    }
}

#[cfg(test)]
#[path = "claude_turn_ended/tests.rs"]
mod tests;
