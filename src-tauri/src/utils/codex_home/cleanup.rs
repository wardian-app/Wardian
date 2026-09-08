//! Deletion of a completed authenticated compact mapping, never migration recovery.
use super::*;

pub(super) const DELETING: &str = ".wardian-codex-home-cleanup.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Deleting {
    version: u32,
    intent: Intent,
    ready: Ready,
    slot_identity: (u64, u64),
    link_destination: PathBuf,
}

/// All native/TUI writers must be joined and acquire_preparation held. A plain
/// home is left for agent-directory deletion. A compact home requires both
/// matching records and the exact ready target; pending migration is an error.
/// Keep the agent directory on ANY error. On success the caller may remove it,
/// including the retained intent/cleanup receipt, under the same stable lock.
pub(crate) fn cleanup_managed_home(home: &Path, agent: &str) -> Result<(), String> {
    let (home, source, record) = layout(home, agent)?;
    let marker = record.with_file_name(DELETING);
    let deleting = match storage::read_record::<Deleting>(&marker)? {
        Some(deleting) => deleting,
        None => {
            let Some(intent) = load_intent(&home, agent, &source, &record)? else {
                resolve_managed_home(&home, agent)?; // Reject arbitrary links.
                return Ok(());
            };
            resolve_managed_home(&home, agent)?;
            let ready =
                migration::ready(&intent)?.ok_or("Compact home is not ready for cleanup")?;
            if storage::exists(&intent.staging())? {
                return Err("Unexpected compact staging state; cleanup refused".into());
            }
            if ready.copied {
                if storage::directory_identity(&intent.backup())? != intent.source_identity {
                    return Err("Compact backup identity changed; cleanup refused".into());
                }
            } else if storage::exists(&intent.backup())? {
                return Err("Unexpected compact backup; cleanup refused".into());
            }
            slot_contents(&intent)?;
            let deleting = Deleting {
                version: 1,
                slot_identity: storage::directory_identity(intent.slot())?,
                link_destination: std::fs::read_link(&source).map_err(storage::error)?,
                intent,
                ready,
            };
            // A deletion receipt makes partial cleanup retryable without ever
            // treating an incomplete relocation as permission to delete state.
            storage::publish_new(&marker, &deleting)?;
            deleting
        }
    };
    if deleting.version != 1
        || deleting.ready.version != 1
        || deleting.ready.token != deleting.intent.token
        || storage::read_record::<Intent>(&record)?.as_ref() != Some(&deleting.intent)
    {
        return Err("Foreign compact cleanup receipt; state retained".into());
    }
    let intent = &deleting.intent;
    validate_intent(&home, agent, &source, intent, false)?;
    if !storage::exists(intent.slot())? {
        if storage::exists(&source)? || storage::exists(&intent.backup())? {
            return Err("Compact slot missing while source/backup remains; cleanup refused".into());
        }
        return Ok(()); // Prior cleanup completed; caller still owns agent deletion.
    }
    if storage::directory_identity(intent.slot())? != deleting.slot_identity {
        return Err("Compact slot identity changed; cleanup refused".into());
    }
    slot_contents(intent)?;
    let slot_record = storage::read_record::<Intent>(&intent.slot().join(RECORD))?;
    let ready = storage::read_record::<Ready>(&intent.slot().join(READY))?;
    if slot_record.as_ref().is_some_and(|value| value != intent)
        || ready.as_ref().is_some_and(|value| value != &deleting.ready)
    {
        return Err("Compact cleanup records changed; state retained".into());
    }
    // Validate every root before the first deletion, including resumed cleanup.
    if storage::exists(&intent.target)? {
        if slot_record.is_none() || ready.is_none() {
            return Err(
                "Compact target remains without both ownership records; cleanup refused".into(),
            );
        }
        migration::validate_target(intent, &deleting.ready, &intent.target)?;
    }
    if storage::exists(&intent.backup())?
        && (!deleting.ready.copied
            || storage::directory_identity(&intent.backup())? != intent.source_identity)
    {
        return Err("Compact backup identity changed; state retained".into());
    }
    if storage::exists(&source)? {
        if !storage::is_link(&source)?
            || std::fs::read_link(&source).map_err(storage::error)? != deleting.link_destination
        {
            return Err("Compact habitat link changed; cleanup refused".into());
        }
        unlink(&source)?;
    }
    if storage::exists(&intent.target)? {
        remove_tree(&intent.target)?;
    }
    if storage::exists(&intent.backup())? {
        if !deleting.ready.copied
            || storage::directory_identity(&intent.backup())? != intent.source_identity
        {
            return Err("Compact backup identity changed; state retained".into());
        }
        remove_tree(&intent.backup())?;
    }
    for name in [RECORD, READY] {
        let path = intent.slot().join(name);
        if storage::exists(&path)? {
            std::fs::remove_file(path).map_err(storage::error)?;
        }
    }
    std::fs::remove_dir(intent.slot()).map_err(storage::error)
}

fn slot_contents(intent: &Intent) -> Result<(), String> {
    for entry in std::fs::read_dir(intent.slot()).map_err(storage::error)? {
        let name = entry.map_err(storage::error)?.file_name();
        if ![RECORD, READY, "h"]
            .iter()
            .any(|allowed| name == std::ffi::OsStr::new(allowed))
        {
            return Err("Unknown compact-slot state retained; cleanup refused".into());
        }
    }
    Ok(())
}

fn unlink(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage::error)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x10 != 0 {
            return std::fs::remove_dir(path).map_err(storage::error);
        }
    }
    let _ = metadata;
    std::fs::remove_file(path).map_err(storage::error)
}

fn remove_tree(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage::error)?;
    if crate::utils::fs::is_directory_link(&metadata) {
        return unlink(path); // Includes Windows reparse directories; never recurse.
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(storage::error)? {
            remove_tree(&entry.map_err(storage::error)?.path())?;
        }
        std::fs::remove_dir(path).map_err(storage::error)
    } else {
        std::fs::remove_file(path).map_err(storage::error)
    }
}
