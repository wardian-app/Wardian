//! Durable protection for a workspace used by an unpublished provider candidate.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    agent_id: String,
    workspace: String,
}

pub(super) struct WorkspaceReservation {
    path: PathBuf,
    armed: bool,
}

fn linked(metadata: &fs::Metadata) -> bool {
    let is_link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        is_link || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        is_link
    }
}

fn root(home: &Path, create: bool) -> Result<PathBuf, String> {
    let mut path = home.to_path_buf();
    for part in ["runtime", "workspace-handovers"] {
        path.push(part);
        if create && !path.exists() {
            fs::create_dir(&path)
                .map_err(|e| format!("Cannot create workspace reservation directory: {e}"))?;
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && !linked(&meta) => (),
            Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            _ => {
                return Err("Workspace reservation directory is linked, missing, or invalid".into())
            }
        }
    }
    Ok(path)
}

impl WorkspaceReservation {
    /// The caller holds shared deletion exclusion until the reservation is committed.
    pub(super) fn create(home: &Path, agent_id: &str, workspace: &Path) -> Result<Self, String> {
        let workspace = workspace
            .canonicalize()
            .map_err(|e| format!("Cannot resolve handover workspace: {e}"))?;
        let path = root(home, true)?.join(format!("{}.json", uuid::Uuid::new_v4()));
        let record = Record {
            schema: 1,
            agent_id: agent_id.into(),
            workspace: workspace.to_string_lossy().into_owned(),
        };
        let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Cannot persist workspace reservation: {e}"))?;
        Ok(Self { path, armed: false })
    }

    /// Arm before shutdown or spawn: cancellation cannot establish writer exit.
    pub(super) fn arm(&mut self) {
        self.armed = true;
    }

    /// Old writer exit is confirmed and no replacement launch has begun.
    pub(super) fn disarm_after_confirmed_stop(&mut self) {
        self.armed = false;
    }

    /// Only a successful roster commit makes ordinary assignment protection sufficient.
    pub(super) fn committed(mut self) {
        if fs::remove_file(&self.path).is_ok() {
            self.armed = false;
        }
    }
}

impl Drop for WorkspaceReservation {
    fn drop(&mut self) {
        if !self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Fail closed before Git deletion, including reservations retained across restart.
pub(super) fn reject_reserved_workspace(home: &Path, workspace: &Path) -> Result<(), String> {
    let root = root(home, false)?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Cannot inspect workspace reservations: {e}")),
    };
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("Cannot resolve deletion workspace: {e}"))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.is_file() || linked(&meta) {
            return Err(
                "Invalid workspace reservation; inspection required before deletion".into(),
            );
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|_| "Malformed workspace reservation; inspection required before deletion")?;
        let target = Path::new(&record.workspace);
        if record.schema != 1 || record.agent_id.is_empty() || !target.is_absolute() {
            return Err(
                "Invalid workspace reservation; inspection required before deletion".into(),
            );
        }
        let target = target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf());
        if target == workspace {
            return Err("Cannot delete a workspace reserved by an unconfirmed provider handover; inspect the provider candidate and retained runtime/workspace-handovers record first".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preflight_abort_and_committed_handover_release_reservation() {
        let home = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        drop(WorkspaceReservation::create(home.path(), "owner", target.path()).unwrap());
        reject_reserved_workspace(home.path(), target.path()).unwrap();
        let mut reservation =
            WorkspaceReservation::create(home.path(), "owner", target.path()).unwrap();
        reservation.arm();
        reservation.committed();
        reject_reserved_workspace(home.path(), target.path()).unwrap();
    }
    #[tokio::test]
    async fn cancelled_spawn_keeps_restart_visible_target_protection() {
        let home = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut reservation =
            WorkspaceReservation::create(home.path(), "owner", target.path()).unwrap();
        reservation.arm();
        let task = tokio::spawn(async move {
            let _reservation = reservation;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        assert!(reject_reserved_workspace(home.path(), target.path())
            .unwrap_err()
            .contains("unconfirmed"));
    }
    #[test]
    fn spawn_failure_and_malformed_records_reject_deletion() {
        let home = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut reservation =
            WorkspaceReservation::create(home.path(), "owner", target.path()).unwrap();
        reservation.arm();
        drop(reservation);
        assert!(reject_reserved_workspace(home.path(), target.path()).is_err());
        let other = tempfile::tempdir().unwrap();
        reject_reserved_workspace(home.path(), other.path()).unwrap();
        fs::write(root(home.path(), false).unwrap().join("invalid.json"), b"{").unwrap();
        assert!(reject_reserved_workspace(home.path(), other.path())
            .unwrap_err()
            .contains("Malformed"));
    }
}
