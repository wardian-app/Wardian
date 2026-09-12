use crate::automation::Blueprint;
use crate::engine::event::Event;
use crate::engine::state::RunState;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

mod checkpoint;

const EVENTS: &str = "events.jsonl";
const CHECKPOINT: &str = "state.json";
const BLUEPRINT: &str = "blueprint.json";

/// Append one event as a JSON line to `<root>/events.jsonl`.
pub fn append_event(root: &Path, ev: &Event) -> crate::engine::Result<()> {
    std::fs::create_dir_all(root)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(EVENTS))?;
    writeln!(f, "{}", serde_json::to_string(ev)?)?;
    Ok(())
}

/// Read all events in order from `<root>/events.jsonl` (empty if absent).
pub fn read_events(root: &Path) -> crate::engine::Result<Vec<Event>> {
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

/// Write the checkpoint snapshot to `<root>/state.json`. Readers that share
/// deletion may retain their previous snapshot while the next one replaces it.
pub fn write_checkpoint(root: &Path, state: &RunState) -> crate::engine::Result<()> {
    std::fs::create_dir_all(root)?;
    checkpoint::write(&root.join(CHECKPOINT), state)?;
    Ok(())
}

/// Read the checkpoint, or `None` if absent.
pub fn read_checkpoint(root: &Path) -> crate::engine::Result<Option<RunState>> {
    let path = root.join(CHECKPOINT);
    if !path.exists() {
        return Ok(None);
    }
    let mut state: RunState = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    state.normalize_legacy();
    Ok(Some(state))
}

/// Persist the parsed blueprint used to start a run. The snapshot is immutable
/// so restart recovery can use the same graph even if the library copy moves
/// or changes later.
pub fn write_blueprint_snapshot(root: &Path, blueprint: &Blueprint) -> crate::engine::Result<()> {
    std::fs::create_dir_all(root)?;
    let path = root.join(BLUEPRINT);
    if path.exists() {
        let existing: Blueprint = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        if serde_json::to_value(existing)? != serde_json::to_value(blueprint)? {
            return Err(crate::engine::EngineError::InvalidState(
                "automation blueprint snapshot already exists and differs".into(),
            ));
        }
        return Ok(());
    }
    crate::atomic_file::write_json_atomic(&path, blueprint)?;
    Ok(())
}

/// Read the immutable parsed blueprint snapshot for a run, when present.
pub fn read_blueprint_snapshot(root: &Path) -> crate::engine::Result<Option<Blueprint>> {
    let path = root.join(BLUEPRINT);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(path)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event::{Event, EventKind};

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
                    run_id: Some("run-1".into()),
                    blueprint_hash: None,
                    blueprint_id: "wf".into(),
                    schema: 2,
                    trigger: serde_json::json!({}),
                },
            ),
        )
        .unwrap();
        append_event(
            root,
            &Event::at(1, "t1".into(), EventKind::NodeStarted { node: "a".into() }),
        )
        .unwrap();
        let events = read_events(root).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].seq, 1);
    }

    #[test]
    fn checkpoint_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = crate::engine::state::RunState::new("r", "wf");
        s.next_seq = 9;
        write_checkpoint(dir.path(), &s).unwrap();
        let back = read_checkpoint(dir.path()).unwrap().unwrap();
        assert_eq!(back.next_seq, 9);
    }

    #[cfg(windows)]
    #[test]
    fn checkpoint_replacement_preserves_open_reader() {
        use std::io::Read;

        let dir = tempfile::tempdir().unwrap();
        let mut root = dir.path().to_path_buf();
        while root.as_os_str().len() < 270 {
            root.push("long-workflow-checkpoint-path");
        }
        let mut state = RunState::new("run", "workflow");
        state.next_seq = 2;
        write_checkpoint(&root, &state).unwrap();
        // Rust's normal file reader shares deletion. Keep it open across the
        // same replacement that polling clients can overlap with the driver.
        let mut reader = File::open(root.join(CHECKPOINT)).unwrap();
        state.next_seq = 3;
        write_checkpoint(&root, &state).expect("checkpoint replacement while read handle is open");
        assert_eq!(read_checkpoint(&root).unwrap().unwrap().next_seq, 3);
        state.next_seq = 4;
        write_checkpoint(&root, &state).expect("another checkpoint with the old reader still open");
        assert_eq!(read_checkpoint(&root).unwrap().unwrap().next_seq, 4);
        let mut previous = String::new();
        reader.read_to_string(&mut previous).unwrap();
        assert_eq!(
            serde_json::from_str::<RunState>(&previous)
                .unwrap()
                .next_seq,
            2
        );
    }

    #[cfg(windows)]
    #[test]
    fn checkpoint_replacement_does_not_bypass_exclusive_reader() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let mut state = RunState::new("run", "workflow");
        state.next_seq = 2;
        write_checkpoint(dir.path(), &state).unwrap();
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE, without delete.
            .open(dir.path().join(CHECKPOINT))
            .unwrap();
        state.next_seq = 3;
        assert!(write_checkpoint(dir.path(), &state).is_err());
        assert_eq!(read_checkpoint(dir.path()).unwrap().unwrap().next_seq, 2);
        drop(reader);
        write_checkpoint(dir.path(), &state).unwrap();
        assert_eq!(read_checkpoint(dir.path()).unwrap().unwrap().next_seq, 3);
    }

    #[test]
    fn legacy_checkpoint_gets_run_storage_during_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CHECKPOINT),
            serde_json::json!({
                "run_id": "legacy",
                "blueprint_id": "wf",
                "status": "running",
                "nodes": {},
                "registry": {"nodes": {}, "trigger": {"output": {}}},
                "loop_iter": {},
                "delivered": {},
                "skipped_edges": [],
                "next_seq": 2,
                "failure": null
            })
            .to_string(),
        )
        .unwrap();

        let state = read_checkpoint(dir.path()).unwrap().unwrap();

        assert_eq!(state.registry["storage"], serde_json::json!({}));
    }
}
