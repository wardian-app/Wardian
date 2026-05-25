use base64::Engine;
use std::collections::{hash_map::Entry, HashMap};
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

pub struct TerminalAttachRequest<'a> {
    pub session_id: &'a str,
    pub attachment_id: &'a str,
    pub remote_session_id: &'a str,
    pub device_id: &'a str,
    pub cols: u16,
    pub rows: u16,
    pub initial_output: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalAttachment {
    remote_session_id: String,
    device_id: String,
    attach_sequence: u64,
}

struct TerminalAttachRuntime {
    parser: vt100::Parser,
    attachments: HashMap<String, TerminalAttachment>,
    owner_attachment_id: Option<String>,
    attach_sequence: u64,
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
        request: TerminalAttachRequest<'_>,
    ) -> Result<TerminalAttachSubscription, String> {
        let attachment_id = sanitize_id(request.attachment_id, "attachment_id")?;
        let remote_session_id = sanitize_id(request.remote_session_id, "remote_session_id")?;
        let device_id = sanitize_id(request.device_id, "device_id")?;
        let (cols, rows) = normalize_geometry(request.cols, request.rows);
        let mut runtimes = self
            .runtimes
            .lock()
            .map_err(|_| "terminal_attach_state_unavailable".to_string())?;
        let mut created_runtime = false;
        let runtime = match runtimes.entry(request.session_id.to_string()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                created_runtime = true;
                let (sender, _) = broadcast::channel(TERMINAL_ATTACH_UPDATE_BUFFER);
                entry.insert(TerminalAttachRuntime {
                    parser: vt100::Parser::new(rows, cols, TERMINAL_ATTACH_SCROLLBACK_LINES),
                    attachments: HashMap::new(),
                    owner_attachment_id: None,
                    attach_sequence: 0,
                    generation: 0,
                    sender,
                })
            }
        };
        runtime.parser.screen_mut().set_size(rows, cols);
        if created_runtime && !request.initial_output.is_empty() {
            runtime.parser.process(request.initial_output);
        }
        runtime.attach_sequence = runtime.attach_sequence.saturating_add(1);
        runtime.attachments.insert(
            attachment_id.clone(),
            TerminalAttachment {
                remote_session_id,
                device_id,
                attach_sequence: runtime.attach_sequence,
            },
        );
        runtime.owner_attachment_id = Some(attachment_id.clone());
        runtime.generation = runtime.generation.saturating_add(1);
        let mut snapshot = runtime.snapshot(Some(attachment_id.clone()));
        if !request.initial_output.is_empty() {
            snapshot.state_base64 = encode_base64(request.initial_output);
        }
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
        runtime.parser.process(bytes);
        if !runtime.attachments.is_empty() {
            let _ = runtime.sender.send(TerminalAttachEvent::Update {
                attachment_id: None,
                owner_attachment_id: runtime.owner_attachment_id.clone(),
                state_base64: encode_base64(bytes),
            });
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
        self.runtimes.lock().ok().and_then(|runtimes| {
            runtimes
                .get(session_id)
                .map(|runtime| runtime.snapshot(None))
        })
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
    attachments
        .iter()
        .max_by_key(|(_, attachment)| attachment.attach_sequence)
        .map(|(attachment_id, _)| attachment_id.clone())
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
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 40,
                rows: 10,
                initial_output: &[],
            })
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
    fn remote_attach_replays_initial_output_for_client_scrollback() {
        let state = TerminalAttachState::default();
        let seed = (1..=40)
            .map(|line| format!("history line {line}\r\n"))
            .collect::<String>();

        let attachment = state
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 80,
                rows: 12,
                initial_output: seed.as_bytes(),
            })
            .expect("attach with history");

        assert_eq!(
            decode_base64(&attachment.snapshot.state_base64),
            seed.as_bytes()
        );
        assert!(attachment.snapshot.text.contains("history line 40"));
    }

    #[test]
    fn stale_attachment_cannot_write_after_new_owner_attaches() {
        let state = TerminalAttachState::default();
        state
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 80,
                rows: 24,
                initial_output: &[],
            })
            .expect("first attach");
        state
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-2",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 100,
                rows: 30,
                initial_output: &[],
            })
            .expect("second attach");

        assert!(!state.is_owner("agent-1", "attach-1"));
        assert!(state.is_owner("agent-1", "attach-2"));
    }

    #[test]
    fn detaching_owner_promotes_most_recent_remaining_attachment() {
        let state = TerminalAttachState::default();
        for attachment_id in ["z-first", "a-second", "m-third"] {
            state
                .attach(TerminalAttachRequest {
                    session_id: "agent-1",
                    attachment_id,
                    remote_session_id: "remote-session-1",
                    device_id: "device-1",
                    cols: 80,
                    rows: 24,
                    initial_output: &[],
                })
                .expect("attach");
        }

        assert!(state.is_owner("agent-1", "m-third"));

        state.detach("agent-1", "m-third").expect("detach owner");

        assert!(state.is_owner("agent-1", "a-second"));
        assert!(!state.is_owner("agent-1", "z-first"));
    }

    #[test]
    fn idle_runtime_is_disposed_only_after_matching_idle_generation() {
        let state = TerminalAttachState::default();
        state
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 80,
                rows: 24,
                initial_output: &[],
            })
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
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 80,
                rows: 24,
                initial_output: &[],
            })
            .expect("attach");
        assert!(state.process_output("agent-1", b"live"));
    }

    #[test]
    fn process_output_streams_live_pty_bytes_to_remote_attachments() {
        let state = TerminalAttachState::default();
        let mut subscription = state
            .attach(TerminalAttachRequest {
                session_id: "agent-1",
                attachment_id: "attach-1",
                remote_session_id: "remote-session-1",
                device_id: "device-1",
                cols: 80,
                rows: 24,
                initial_output: &[],
            })
            .expect("attach");
        let _ = subscription.receiver.try_recv();

        let live_frame = b"\x1b[?2026h\x1b[HWorking...\x1b[K\x1b[?2026l";
        assert!(state.process_output("agent-1", live_frame));

        let event = subscription.receiver.try_recv().expect("live update");
        let TerminalAttachEvent::Update { state_base64, .. } = event else {
            panic!("expected live update");
        };
        assert_eq!(decode_base64(&state_base64), live_frame);
    }

    fn decode_base64(value: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .expect("base64")
    }
}
