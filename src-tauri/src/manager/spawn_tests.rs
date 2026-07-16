use super::spawn::capture_init_timestamp;
use std::sync::{Arc, Mutex};
use wardian_core::models::AgentEvent;

#[test]
fn init_event_only_captures_timestamp() {
    let timestamp = Arc::new(Mutex::new(None));
    capture_init_timestamp(
        &AgentEvent::Init {
            session_id: "00000000-0000-4000-8000-0000000000aa".into(),
            timestamp: Some("2026-07-16T12:00:00Z".into()),
        },
        &timestamp,
    );
    assert_eq!(
        timestamp.lock().unwrap().as_deref(),
        Some("2026-07-16T12:00:00Z")
    );
}
