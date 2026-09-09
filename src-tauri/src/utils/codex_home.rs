//! Persistent compact Codex homes. Only the exclusive owner may migrate state.
//! Generic projection/MCP callers resolve authenticated mappings without recovery.
mod cleanup;
mod copy_metadata;
mod migration;
mod platform;
mod storage;
#[cfg(test)]
mod tests;
mod tree;

pub(crate) use cleanup::cleanup_managed_home;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
pub(crate) use storage::HomePreparationGuard;

const RECORD: &str = ".wardian-codex-home.json";
const READY: &str = ".wardian-codex-home-ready.json";
const OVERLAY: &str = ".wardian-launch-config.json";

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_ROOTS: std::cell::RefCell<Option<Vec<PathBuf>>> = const { std::cell::RefCell::new(None) };
}

/// Acquire outermost around recovery/migration/projection/MCP, including generic
/// habitat refresh. Never nest or retain across provider lifetime. Busy fails
/// immediately. The persistent lock file stays outside agent and target trees.
pub(crate) fn acquire_preparation(
    home: &Path,
    agent_id: &str,
) -> Result<HomePreparationGuard, String> {
    use sha2::{Digest, Sha256};
    validate_agent_id(agent_id)?;
    if !home.is_absolute() {
        return Err("Codex preparation requires an absolute Wardian home".into());
    }
    let home = canonical(home)?;
    storage::plain_directory(&home)?;
    let locks = home.join("locks");
    platform::create_private_root(&locks)?;
    platform::validate_private_root(&locks)?;
    let digest = format!("{:x}", Sha256::digest(agent_id.as_bytes()));
    let guard = storage::lock(&locks.join(format!("codex-home-{digest}.lock")))?;
    for path in [home.join("agents"), home.join("agents").join(agent_id)] {
        if storage::exists(&path)? {
            storage::plain_directory(&path)?;
        }
    }
    Ok(guard)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    token: String,
    agent_id: String,
    wardian_home: PathBuf,
    source: PathBuf,
    target: PathBuf,
    source_identity: (u64, u64),
    snapshot: String,
}

impl Intent {
    fn slot(&self) -> &Path {
        self.target.parent().expect("validated target")
    }
    fn staging(&self) -> PathBuf {
        self.slot().join("c")
    }
    fn backup(&self) -> PathBuf {
        self.source
            .with_file_name(format!(".codex-precompact-{}", self.token))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Ready {
    version: u32,
    token: String,
    target_identity: (u64, u64),
    copied: bool,
}

/// Owner gate required, with previous writers joined. Select the authoritative
/// physical home BEFORE launch-overlay recovery. An authenticated interrupted
/// relocation is completed first; its intent was recorded only after recovery.
/// A new ordinary home is created, but no configuration is projected here.
pub(crate) fn owner_preparation_home(home: &Path, agent_id: &str) -> Result<PathBuf, String> {
    let (home, source, record) = layout(home, agent_id)?;
    if let Some(intent) = load_intent(&home, agent_id, &source, &record)? {
        migration::resume(&intent)?;
        return resolve_managed_home(&home, agent_id);
    }
    if !storage::exists(&source)? {
        std::fs::create_dir(&source).map_err(storage::error)?;
    }
    storage::plain_directory(&source)?;
    canonical(&source)
}

/// Owner gate required, after prior writers joined AND recover_launch_config
/// succeeded on owner_preparation_home's result. No global or temporary home is
/// ever selected. Short homes retain their existing physical location.
pub(crate) fn prepare_compact_home(home: &Path, agent_id: &str) -> Result<PathBuf, String> {
    prepare_with_roots(home, agent_id, || {
        #[cfg(test)]
        {
            TEST_ROOTS
                .with(|roots| roots.borrow().clone())
                .ok_or_else(|| "Unit test must inject compact Codex roots".into())
        }
        #[cfg(not(test))]
        {
            platform::root_candidates(home)
        }
    })
}

fn prepare_with_roots(
    home: &Path,
    agent_id: &str,
    roots: impl FnOnce() -> Result<Vec<PathBuf>, String>,
) -> Result<PathBuf, String> {
    let (home, source, record) = layout(home, agent_id)?;
    if let Some(intent) = load_intent(&home, agent_id, &source, &record)? {
        migration::resume(&intent)?;
        let resolved = resolve_managed_home(&home, agent_id)?;
        no_overlay(&resolved)?;
        return Ok(resolved);
    }
    storage::plain_directory(&source)?;
    no_overlay(&source)?;
    let physical = canonical(&source)?;
    if socket_fits(&physical) {
        return Ok(physical);
    }
    let snapshot = tree::snapshot(&source)?;
    let mut failures = Vec::new();
    for root in roots()? {
        match reserve(&root) {
            Ok(target) => {
                let intent = Intent {
                    version: 1,
                    token: uuid::Uuid::new_v4().to_string(),
                    agent_id: agent_id.to_owned(),
                    wardian_home: home.clone(),
                    source: source.clone(),
                    target,
                    source_identity: storage::directory_identity(&source)?,
                    snapshot,
                };
                // Both immutable records precede rename/copy. A crash between
                // records leaves only an unused owned slot and the original home.
                storage::publish_new(&intent.slot().join(RECORD), &intent)?;
                storage::publish_new(&record, &intent)?;
                migration::resume(&intent)?;
                return resolve_managed_home(&home, agent_id);
            }
            Err(error) => failures.push(error),
        }
    }
    Err(format!("No secure compact Codex home fits the local socket limit; configure a shorter private root and retry: {}", failures.join("; ")))
}

/// Read-only. Plain homes are accepted; linked homes require matching agent and
/// private-slot records plus a readiness receipt for the exact physical target.
/// Incomplete relocation is an owner-startup recovery action, never a refresh.
pub(crate) fn resolve_managed_home(home: &Path, agent_id: &str) -> Result<PathBuf, String> {
    let (home, source, record) = layout(home, agent_id)?;
    match load_intent(&home, agent_id, &source, &record)? {
        None => {
            if storage::exists(&source)? {
                storage::plain_directory(&source)?;
                canonical(&source)
            } else {
                Ok(source) // Canonical, validated habitat parent; no write here.
            }
        }
        Some(intent) => {
            let ready = migration::ready(&intent)?
                .ok_or("Compact Codex home is incomplete; restart through its exclusive owner")?;
            migration::validate_target(&intent, &ready, &intent.target)?;
            if !storage::is_link(&source)? || canonical(&source)? != intent.target {
                return Err(
                    "Compact Codex home link is incomplete or foreign; owner recovery required"
                        .into(),
                );
            }
            Ok(intent.target)
        }
    }
}

fn layout(home: &Path, agent_id: &str) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let (home, agent) = agent_directory(home, agent_id)?;
    let habitat = agent.join("habitat");
    storage::plain_directory(&habitat)?;
    Ok((home, habitat.join(".codex"), agent.join(RECORD)))
}

fn validate_agent_id(agent_id: &str) -> Result<(), String> {
    if agent_id.trim() != agent_id
        || agent_id.contains(['/', '\\'])
        || !matches!(
            Path::new(agent_id)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [Component::Normal(_)]
        )
    {
        return Err("Compact Codex home requires one full agent ID component".into());
    }
    Ok(())
}

fn agent_directory(home: &Path, agent_id: &str) -> Result<(PathBuf, PathBuf), String> {
    validate_agent_id(agent_id)?;
    if !home.is_absolute() {
        return Err("Compact Codex home requires an absolute Wardian home".into());
    }
    let home = canonical(home)?;
    let agents = home.join("agents");
    let agent = agents.join(agent_id);
    storage::plain_directory(&home)?;
    for path in [&agents, &agent] {
        storage::plain_directory(path)?;
    }
    Ok((home, agent))
}

fn load_intent(
    home: &Path,
    agent: &str,
    source: &Path,
    record: &Path,
) -> Result<Option<Intent>, String> {
    if storage::exists(&record.with_file_name(cleanup::DELETING))? {
        return Err("Codex home deletion is in progress; preparation/projection refused".into());
    }
    let Some(intent) = storage::read_record::<Intent>(record)? else {
        return Ok(None);
    };
    validate_intent(home, agent, source, &intent, true)?;
    if storage::read_record::<Intent>(&intent.slot().join(RECORD))?.as_ref() != Some(&intent) {
        return Err(
            "Compact home ownership records disagree; no migration or projection allowed".into(),
        );
    }
    Ok(Some(intent))
}

fn validate_intent(
    home: &Path,
    agent: &str,
    source: &Path,
    intent: &Intent,
    require_slot: bool,
) -> Result<(), String> {
    let slot = intent.target.parent().ok_or("Invalid compact target")?;
    let root = slot.parent().ok_or("Invalid compact root")?;
    let name = slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid compact slot")?;
    if intent.version != 1
        || uuid::Uuid::parse_str(&intent.token)
            .map(|id| id.get_version_num() != 4)
            .unwrap_or(true)
        || intent.agent_id != agent
        || intent.wardian_home != home
        || intent.source != source
        || intent.target.file_name() != Some(std::ffi::OsStr::new("h"))
        || name.len() != 8
        || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
        || intent.snapshot.len() != 64
        || !intent.snapshot.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(
            "Foreign or malformed compact Codex home record; retained for inspection".into(),
        );
    }
    platform::validate_private_root(root)?;
    if require_slot || storage::exists(slot)? {
        platform::validate_private_root(slot)?;
        if canonical(slot)? != slot {
            return Err("Compact home slot changed".into());
        }
    }
    if !socket_fits(&intent.target) {
        return Err("Compact home root changed or exceeds the socket limit".into());
    }
    Ok(())
}

fn reserve(root: &Path) -> Result<PathBuf, String> {
    platform::create_private_root(root)?;
    platform::validate_private_root(root)?;
    let root = canonical(root)?;
    if !socket_fits(&root.join("12345678").join("h")) {
        return Err("Private root is too long for the stock Codex socket".into());
    }
    for _ in 0..16 {
        let name = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
        let slot = root.join(name);
        match platform::create_private_directory(&slot) {
            Ok(()) => {
                platform::validate_private_root(&slot)?;
                return Ok(slot.join("h"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(storage::error(error)),
        }
    }
    Err("Cannot reserve a unique private compact Codex slot".into())
}

fn no_overlay(home: &Path) -> Result<(), String> {
    if storage::exists(&home.join(OVERLAY))? {
        return Err("Recover the launch-config journal on the authoritative physical home before relocation".into());
    }
    Ok(())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path).map_err(storage::error)?;
    #[cfg(windows)]
    let path = PathBuf::from(super::fs::strip_windows_verbatim_prefix(
        path.to_str().ok_or("Codex home is not valid UTF-8")?,
    ));
    Ok(path)
}

fn socket_fits(home: &Path) -> bool {
    let socket = home
        .join("app-server-control")
        .join("app-server-control.sock");
    #[cfg(unix)]
    let capacity = {
        // SAFETY: all-zero sockaddr_un is used only to inspect array capacity.
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_path.len()
    };
    #[cfg(windows)]
    let capacity = 108;
    #[cfg(not(any(unix, windows)))]
    let capacity = 104;
    let bytes = socket.as_os_str().as_encoded_bytes();
    !bytes.contains(&0) && bytes.len() < capacity
}
