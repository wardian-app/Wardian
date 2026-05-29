use crate::event::Event;
use crate::state::RunState;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

const EVENTS: &str = "events.jsonl";
const CHECKPOINT: &str = "state.json";

/// Append one event as a JSON line to `<root>/events.jsonl`.
pub fn append_event(root: &Path, ev: &Event) -> crate::Result<()> {
    std::fs::create_dir_all(root)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(EVENTS))?;
    writeln!(f, "{}", serde_json::to_string(ev)?)?;
    Ok(())
}

/// Read all events in order from `<root>/events.jsonl` (empty if absent).
pub fn read_events(root: &Path) -> crate::Result<Vec<Event>> {
    let path = root.join(EVENTS);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

/// Write the checkpoint snapshot to `<root>/state.json`.
pub fn write_checkpoint(root: &Path, state: &RunState) -> crate::Result<()> {
    std::fs::create_dir_all(root)?;
    std::fs::write(root.join(CHECKPOINT), serde_json::to_string_pretty(state)?)?;
    Ok(())
}

/// Read the checkpoint, or `None` if absent.
pub fn read_checkpoint(root: &Path) -> crate::Result<Option<RunState>> {
    let path = root.join(CHECKPOINT);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(path)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind};

    #[test]
    fn append_then_read_events() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        append_event(
            root,
            &Event::at(
                0,
                "t0".into(),
                EventKind::RunStarted {
                    blueprint_id: "wf".into(),
                    schema: 2,
                    trigger: serde_json::json!({}),
                },
            ),
        )
        .unwrap();
        append_event(
            root,
            &Event::at(
                1,
                "t1".into(),
                EventKind::NodeStarted { node: "a".into() },
            ),
        )
        .unwrap();
        let events = read_events(root).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].seq, 1);
    }

    #[test]
    fn checkpoint_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = crate::state::RunState::new("r", "wf");
        s.next_seq = 9;
        write_checkpoint(dir.path(), &s).unwrap();
        let back = read_checkpoint(dir.path()).unwrap().unwrap();
        assert_eq!(back.next_seq, 9);
    }
}
