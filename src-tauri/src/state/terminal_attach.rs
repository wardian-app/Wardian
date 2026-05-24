use base64::Engine;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tokio::sync::broadcast;

const TERMINAL_ATTACH_SCROLLBACK_LINES: usize = 1_000;
const TERMINAL_ATTACH_UPDATE_BUFFER: usize = 64;
const MIN_ATTACH_COLS: u16 = 20;
const MIN_ATTACH_ROWS: u16 = 8;
const MAX_ATTACH_COLS: u16 = 240;
const MAX_ATTACH_ROWS: u16 = 80;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TerminalScreenSnapshot {
    pub attachment_id: Option<String>,
    pub owner_attachment_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub state_base64: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalAttachEvent {
    Snapshot {
        attachment_id: Option<String>,
        owner_attachment_id: Option<String>,
        cols: u16,
        rows: u16,
        state_base64: String,
    },
    Update {
        attachment_id: Option<String>,
        owner_attachment_id: Option<String>,
        state_base64: String,
    },
    Ownership {
        owner_attachment_id: Option<String>,
        cols: u16,
        rows: u16,
    },
}

pub struct TerminalAttachSubscription {
    pub attachment_id: String,
    pub snapshot: TerminalScreenSnapshot,
    pub receiver: broadcast::Receiver<TerminalAttachEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalAttachment {
    remote_session_id: String,
    device_id: String,
}

struct TerminalAttachRuntime {
    parser: vt100::Parser,
    attachments: HashMap<String, TerminalAttachment>,
    owner_attachment_id: Option<String>,
    generation: u64,
    sender: broadcast::Sender<TerminalAttachEvent>,
}

#[derive(Default)]
pub struct TerminalAttachState {
    runtimes: Mutex<HashMap<String, TerminalAttachRuntime>>,
}

impl TerminalAttachState {
    pub fn attach(
        &self,
        session_id: &str,
        attachment_id: &str,
        remote_session_id: &str,
        device_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalAttachSubscription, String> {
        let attachment_id = sanitize_id(attachment_id, "attachment_id")?;
        let remote_session_id = sanitize_id(remote_session_id, "remote_session_id")?;
        let device_id = sanitize_id(device_id, "device_id")?;
        let (cols, rows) = normalize_geometry(cols, rows);
        let mut runtimes = self
            .runtimes
            .lock()
            .map_err(|_| "terminal_attach_state_unavailable".to_string())?;
        let runtime = runtimes.entry(session_id.to_string()).or_insert_with(|| {
            let (sender, _) = broadcast::channel(TERMINAL_ATTACH_UPDATE_BUFFER);
            TerminalAttachRuntime {
                parser: vt100::Parser::new(rows, cols, TERMINAL_ATTACH_SCROLLBACK_LINES),
                attachments: HashMap::new(),
                owner_attachment_id: None,
                generation: 0,
                sender,
            }
        });
        runtime.parser.screen_mut().set_size(rows, cols);
        runtime.attachments.insert(
            attachment_id.clone(),
            TerminalAttachment {
                remote_session_id,
                device_id,
            },
        );
        runtime.owner_attachment_id = Some(attachment_id.clone());
        runtime.generation = runtime.generation.saturating_add(1);
        let snapshot = runtime.snapshot(Some(attachment_id.clone()));
        let receiver = runtime.sender.subscribe();
        let _ = runtime.sender.send(TerminalAttachEvent::Ownership {
            owner_attachment_id: runtime.owner_attachment_id.clone(),
            cols,
            rows,
        });
        Ok(TerminalAttachSubscription {
            attachment_id,
            snapshot,
            receiver,
        })
    }

    pub fn detach(&self, session_id: &str, attachment_id: &str) -> Option<u64> {
        let mut runtimes = self.runtimes.lock().ok()?;
        let runtime = runtimes.get_mut(session_id)?;
        runtime.attachments.remove(attachment_id)?;
        if runtime.owner_attachment_id.as_deref() == Some(attachment_id) {
            runtime.owner_attachment_id = most_recent_attachment(&runtime.attachments);
        }
        runtime.generation = runtime.generation.saturating_add(1);
        let generation = runtime.generation;
        let (rows, cols) = runtime.parser.screen().size();
        let _ = runtime.sender.send(TerminalAttachEvent::Ownership {
            owner_attachment_id: runtime.owner_attachment_id.clone(),
            cols,
            rows,
        });
        Some(generation)
    }

    pub fn dispose_if_idle_generation(&self, session_id: &str, generation: u64) -> bool {
        let mut runtimes = match self.runtimes.lock() {
            Ok(runtimes) => runtimes,
            Err(_) => return false,
        };
        let should_remove = runtimes.get(session_id).is_some_and(|runtime| {
            runtime.attachments.is_empty() && runtime.generation == generation
        });
        if should_remove {
            runtimes.remove(session_id);
        }
        should_remove
    }

    pub fn process_output(&self, session_id: &str, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let mut runtimes = match self.runtimes.lock() {
            Ok(runtimes) => runtimes,
            Err(_) => return false,
        };
        let Some(runtime) = runtimes.get_mut(session_id) else {
            return false;
        };
        let previous = runtime.parser.screen().clone();
        runtime.parser.process(bytes);
        if !runtime.attachments.is_empty() {
            let diff = runtime.parser.screen().state_diff(&previous);
            if !diff.is_empty() {
                let _ = runtime.sender.send(TerminalAttachEvent::Update {
                    attachment_id: None,
                    owner_attachment_id: runtime.owner_attachment_id.clone(),
                    state_base64: encode_base64(&diff),
                });
            }
        }
        true
    }

    pub fn resize_owner(
        &self,
        session_id: &str,
        attachment_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalScreenSnapshot, String> {
        let (cols, rows) = normalize_geometry(cols, rows);
        let mut runtimes = self
            .runtimes
            .lock()
            .map_err(|_| "terminal_attach_state_unavailable".to_string())?;
        let runtime = runtimes
            .get_mut(session_id)
            .ok_or_else(|| "terminal_attach_not_found".to_string())?;
        if runtime.owner_attachment_id.as_deref() != Some(attachment_id) {
            return Err("terminal_attach_not_owner".to_string());
        }
        runtime.parser.screen_mut().set_size(rows, cols);
        let snapshot = runtime.snapshot(Some(attachment_id.to_string()));
        let _ = runtime.sender.send(TerminalAttachEvent::Snapshot {
            attachment_id: Some(attachment_id.to_string()),
            owner_attachment_id: runtime.owner_attachment_id.clone(),
            cols,
            rows,
            state_base64: snapshot.state_base64.clone(),
        });
        Ok(snapshot)
    }

    pub fn is_owner(&self, session_id: &str, attachment_id: &str) -> bool {
        self.runtimes
            .lock()
            .ok()
            .and_then(|runtimes| {
                runtimes
                    .get(session_id)
                    .and_then(|runtime| runtime.owner_attachment_id.clone())
            })
            .as_deref()
            == Some(attachment_id)
    }

    pub fn snapshot(&self, session_id: &str) -> Option<TerminalScreenSnapshot> {
        self.runtimes
            .lock()
            .ok()
            .and_then(|runtimes| runtimes.get(session_id).map(|runtime| runtime.snapshot(None)))
    }
}

impl TerminalAttachRuntime {
    fn snapshot(&self, attachment_id: Option<String>) -> TerminalScreenSnapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        TerminalScreenSnapshot {
            attachment_id,
            owner_attachment_id: self.owner_attachment_id.clone(),
            cols,
            rows,
            state_base64: encode_base64(&screen.state_formatted()),
            text: screen.contents(),
        }
    }
}

fn normalize_geometry(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.clamp(MIN_ATTACH_COLS, MAX_ATTACH_COLS),
        rows.clamp(MIN_ATTACH_ROWS, MAX_ATTACH_ROWS),
    )
}

fn sanitize_id(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!("{field}_invalid"));
    }
    Ok(value.to_string())
}

fn most_recent_attachment(attachments: &HashMap<String, TerminalAttachment>) -> Option<String> {
    let keys: HashSet<&str> = attachments.keys().map(String::as_str).collect();
    keys.into_iter().max().map(str::to_string)
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_attach_becomes_owner_and_renders_processed_output() {
        let state = TerminalAttachState::default();
        let attachment = state
            .attach("agent-1", "attach-1", "remote-session-1", "device-1", 40, 10)
            .expect("attach");
        assert_eq!(
            attachment.snapshot.owner_attachment_id.as_deref(),
            Some("attach-1")
        );

        state.process_output("agent-1", b"\x1b[31mRED\x1b[0m");
        let snapshot = state.snapshot("agent-1").expect("snapshot after output");
        assert!(snapshot.state_base64.len() > 8);
        assert!(snapshot.text.contains("RED"));
    }

    #[test]
    fn stale_attachment_cannot_write_after_new_owner_attaches() {
        let state = TerminalAttachState::default();
        state
            .attach("agent-1", "attach-1", "remote-session-1", "device-1", 80, 24)
            .expect("first attach");
        state
            .attach("agent-1", "attach-2", "remote-session-1", "device-1", 100, 30)
            .expect("second attach");

        assert!(!state.is_owner("agent-1", "attach-1"));
        assert!(state.is_owner("agent-1", "attach-2"));
    }

    #[test]
    fn idle_runtime_is_disposed_only_after_matching_idle_generation() {
        let state = TerminalAttachState::default();
        state
            .attach("agent-1", "attach-1", "remote-session-1", "device-1", 80, 24)
            .expect("attach");
        let generation = state.detach("agent-1", "attach-1").expect("detach");

        assert!(state.snapshot("agent-1").is_some());
        assert!(!state.dispose_if_idle_generation("agent-1", generation + 1));
        assert!(state.dispose_if_idle_generation("agent-1", generation));
        assert!(state.snapshot("agent-1").is_none());
    }

    #[test]
    fn process_output_is_cheap_when_no_runtime_exists() {
        let state = TerminalAttachState::default();
        assert!(!state.process_output("missing-agent", b"ignored"));
        state
            .attach("agent-1", "attach-1", "remote-session-1", "device-1", 80, 24)
            .expect("attach");
        assert!(state.process_output("agent-1", b"live"));
    }
}
