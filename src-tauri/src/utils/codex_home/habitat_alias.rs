//! Windows-only short working-directory aliases for projected provider habitats.
//!
//! The alias points at the complete habitat so instruction and skill ancestry
//! remains intact. Its ownership record is separate from Codex home migration:
//! it records one exact junction and is removed only after the provider process
//! has been joined by agent removal.

use super::*;
#[cfg(windows)]
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub(crate) const HABITAT_ALIAS_RECORD: &str = ".wardian-habitat-alias.json";

#[cfg(windows)]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct HabitatAliasRecord {
    version: u32,
    token: String,
    agent_id: String,
    wardian_home: PathBuf,
    habitat: PathBuf,
    target: PathBuf,
    habitat_identity: (u64, u64),
    slot_identity: (u64, u64),
}

#[cfg(windows)]
// A raw UTF-16 cwd of 258 units still launches through CreateProcessW in the
// supported host, while 259 units fails with ERROR_DIRECTORY (267). Leave the
// separator and terminating NUL headroom to the OS by selecting the alias at
// the observed 259-unit boundary.
const WINDOWS_MAX_PATH_UNITS: usize = 258;

#[cfg(windows)]
fn needs_habitat_cwd_alias(logical_cwd: &Path) -> bool {
    windows_path_units(logical_cwd) > WINDOWS_MAX_PATH_UNITS
}

#[cfg(windows)]
pub(crate) fn prepare_habitat_cwd_alias(
    agent_id: &str,
    habitat_root: Option<&Path>,
    workspace_cwd: &Path,
    logical_cwd: &Path,
) -> Result<Option<PathBuf>, String> {
    let Some(habitat_root) = habitat_root else {
        return Ok(None);
    };
    if !needs_habitat_cwd_alias(logical_cwd) {
        return Ok(None);
    }

    let relative =
        relative_habitat_cwd(habitat_root, workspace_cwd, logical_cwd).ok_or_else(|| {
            format!(
                "Long provider cwd {} is outside its projected habitat",
                logical_cwd.display()
            )
        })?;
    let home = crate::utils::fs::get_wardian_home()
        .ok_or("Could not find Wardian home for the long habitat cwd alias")?;
    let target = prepare_alias_for_home(&home, agent_id, habitat_root)?;
    let launch_cwd = target.join(relative);
    if windows_path_units(&launch_cwd) > WINDOWS_MAX_PATH_UNITS {
        return Err(format!(
            "Secure habitat alias remains too long for provider cwd: {}",
            launch_cwd.display()
        ));
    }
    if canonical(&launch_cwd)? != canonical(logical_cwd)? {
        return Err(format!(
            "Habitat alias cwd does not resolve to the logical provider cwd: {}",
            logical_cwd.display()
        ));
    }
    Ok(Some(launch_cwd))
}

#[cfg(windows)]
pub(crate) fn is_owned_habitat_workspace_alias(
    agent_id: &str,
    habitat_root: &Path,
    saved_cwd: &Path,
) -> bool {
    crate::utils::fs::get_wardian_home().is_some_and(|home| {
        is_owned_habitat_workspace_alias_in(&home, agent_id, habitat_root, saved_cwd)
    })
}

#[cfg(windows)]
fn is_owned_habitat_workspace_alias_in(
    wardian_home: &Path,
    agent_id: &str,
    habitat_root: &Path,
    saved_cwd: &Path,
) -> bool {
    (|| -> Option<()> {
        super::validate_agent_id(agent_id).ok()?;
        let home = canonical(wardian_home).ok()?;
        let habitat = canonical(habitat_root).ok()?;
        if expected_habitat(&home, agent_id).ok()? != habitat {
            return None;
        }
        let record_path = home
            .join("agents")
            .join(agent_id)
            .join(HABITAT_ALIAS_RECORD);
        let record = storage::read_record::<HabitatAliasRecord>(&record_path).ok()??;
        let identity = storage::directory_identity(&habitat).ok()?;
        validate_record(&home, agent_id, &habitat, identity, &record).ok()?;
        if saved_cwd != record.target.join("workspace") {
            return None;
        }
        if canonical(saved_cwd).ok()? != canonical(&habitat.join("workspace")).ok()? {
            return None;
        }
        Some(())
    })()
    .is_some()
}

#[cfg(not(windows))]
pub(crate) fn is_owned_habitat_workspace_alias(
    _agent_id: &str,
    _habitat_root: &Path,
    _saved_cwd: &Path,
) -> bool {
    false
}

#[cfg(not(windows))]
pub(crate) fn prepare_habitat_cwd_alias(
    _agent_id: &str,
    _habitat_root: Option<&Path>,
    _workspace_cwd: &Path,
    _logical_cwd: &Path,
) -> Result<Option<PathBuf>, String> {
    Ok(None)
}

#[cfg(windows)]
fn windows_path_units(path: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str().encode_wide().count()
}

fn relative_habitat_cwd(
    habitat_root: &Path,
    workspace_cwd: &Path,
    logical_cwd: &Path,
) -> Option<PathBuf> {
    if paths_equal(habitat_root, logical_cwd) {
        return Some(PathBuf::new());
    }

    let habitat_workspace = habitat_root.join("workspace");
    if paths_equal(&habitat_workspace, logical_cwd) || paths_equal(workspace_cwd, logical_cwd) {
        return Some(PathBuf::from("workspace"));
    }

    let relative = logical_cwd.strip_prefix(habitat_root).ok()?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(relative.to_path_buf())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (canonical(left).ok(), canonical(right).ok()) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[cfg(windows)]
fn prepare_alias_for_home(
    home: &Path,
    agent_id: &str,
    habitat_root: &Path,
) -> Result<PathBuf, String> {
    super::validate_agent_id(agent_id)?;
    let home = canonical(home)?;
    let _preparation = super::acquire_preparation(&home, agent_id)?;
    let habitat = canonical(habitat_root)?;
    if expected_habitat(&home, agent_id)? != habitat {
        return Err("Habitat alias source is outside the owning agent habitat".into());
    }
    let habitat_identity = storage::directory_identity(&habitat)?;
    let record_path = home
        .join("agents")
        .join(agent_id)
        .join(HABITAT_ALIAS_RECORD);

    let record = match storage::read_record::<HabitatAliasRecord>(&record_path)? {
        Some(record) => {
            validate_record(&home, agent_id, &habitat, habitat_identity, &record)?;
            record
        }
        None => {
            let target = reserve_alias_target(&home)?;
            let slot = target
                .parent()
                .ok_or("Habitat alias slot has no parent")?
                .to_path_buf();
            let slot_identity = storage::directory_identity(&slot)?;
            let record = HabitatAliasRecord {
                version: 1,
                token: uuid::Uuid::new_v4().to_string(),
                agent_id: agent_id.to_owned(),
                wardian_home: home.clone(),
                habitat: habitat.clone(),
                target,
                habitat_identity,
                slot_identity,
            };
            // Publish intent before the junction. A crash after this point is
            // recoverable by the same owner; a foreign occupant is retained.
            storage::publish_new(&record_path, &record)?;
            record
        }
    };

    ensure_alias_link(&record, &habitat)?;
    Ok(record.target)
}

#[cfg(windows)]
fn expected_habitat(home: &Path, agent_id: &str) -> Result<PathBuf, String> {
    canonical(&home.join("agents").join(agent_id).join("habitat"))
}

#[cfg(windows)]
fn reserve_alias_target(home: &Path) -> Result<PathBuf, String> {
    let roots = alias_root_candidates(home)?;
    let mut failures = Vec::new();
    for root in roots {
        match super::reserve(&root) {
            Ok(target) => return Ok(target),
            Err(error) => failures.push(error),
        }
    }
    Err(format!(
        "No secure short habitat alias root is available; configure a shorter private root and retry: {}",
        failures.join("; ")
    ))
}

#[cfg(windows)]
fn alias_root_candidates(home: &Path) -> Result<Vec<PathBuf>, String> {
    #[cfg(test)]
    if let Some(roots) = super::TEST_ROOTS.with(|roots| roots.borrow().clone()) {
        return Ok(roots);
    }

    super::platform::root_candidates(home)
}

#[cfg(windows)]
fn validate_record(
    home: &Path,
    agent_id: &str,
    habitat: &Path,
    habitat_identity: (u64, u64),
    record: &HabitatAliasRecord,
) -> Result<(), String> {
    let slot = record
        .target
        .parent()
        .ok_or("Habitat alias target has no slot")?;
    let root = slot.parent().ok_or("Habitat alias slot has no root")?;
    let slot_name = slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Habitat alias slot has no valid name")?;
    if record.version != 1
        || uuid::Uuid::parse_str(&record.token)
            .map(|id| id.get_version_num() != 4)
            .unwrap_or(true)
        || record.agent_id != agent_id
        || record.wardian_home != home
        || record.habitat != habitat
        || record.target != slot.join("h")
        || slot_name.len() != 8
        || !slot_name.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record.habitat_identity != habitat_identity
    {
        return Err("Foreign or malformed habitat alias record; retained for inspection".into());
    }
    super::platform::validate_private_root(root)?;
    super::platform::validate_private_root(slot)?;
    if canonical(slot)? != slot || !super::socket_fits(&record.target) {
        return Err("Habitat alias slot changed or is too long".into());
    }
    if storage::directory_identity(slot)? != record.slot_identity {
        return Err("Habitat alias slot identity changed; cleanup refused".into());
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_alias_link(record: &HabitatAliasRecord, habitat: &Path) -> Result<(), String> {
    let target = &record.target;
    let slot_record = target
        .parent()
        .ok_or("Habitat alias target has no slot")?
        .join(HABITAT_ALIAS_RECORD);
    // Claim or authenticate the slot before publishing its junction. A
    // foreign record therefore leaves the slot exactly as it was found.
    let slot_claimed = match storage::read_record::<HabitatAliasRecord>(&slot_record)? {
        Some(existing) if existing != *record => {
            return Err(
                "Habitat alias slot ownership record changed; retained for inspection".into(),
            )
        }
        Some(_) => true,
        None => false,
    };

    let target_exists = storage::exists(target)?;
    if target_exists && (!storage::is_link(target)? || canonical(target)? != habitat) {
        return Err("Habitat alias target is foreign or was retargeted".into());
    }
    if !slot_claimed {
        storage::publish_new(&slot_record, record)?;
    }

    if target_exists {
        if !storage::is_link(target)? || canonical(target)? != habitat {
            return Err("Habitat alias target is foreign or was retargeted".into());
        }
    } else {
        crate::utils::fs::create_directory_link(habitat, target)?;
        if !storage::is_link(target)? || canonical(target)? != habitat {
            return Err("Habitat alias junction did not resolve to its owned habitat".into());
        }
    }
    Ok(())
}

/// Remove one exact owned habitat junction after the caller has joined every
/// process for the agent. Never recurse through the junction into the habitat.
pub(crate) fn cleanup_habitat_alias(home: &Path, agent_id: &str) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (home, agent_id);
        return Ok(());
    }

    #[cfg(windows)]
    {
        super::validate_agent_id(agent_id)?;
        let home = canonical(home)?;
        let record_path = home
            .join("agents")
            .join(agent_id)
            .join(HABITAT_ALIAS_RECORD);
        let Some(record) = storage::read_record::<HabitatAliasRecord>(&record_path)? else {
            return Ok(());
        };
        let habitat = expected_habitat(&home, agent_id)?;
        if record.habitat != habitat {
            return Err(
                "Habitat alias source is outside the owning agent habitat; cleanup refused".into(),
            );
        }
        validate_record(
            &home,
            agent_id,
            &habitat,
            storage::directory_identity(&habitat)?,
            &record,
        )?;
        let slot = record
            .target
            .parent()
            .ok_or("Habitat alias target has no slot")?;
        validate_slot_contents(slot, &record)?;

        if storage::exists(&record.target)? {
            if !storage::is_link(&record.target)? || canonical(&record.target)? != habitat {
                return Err("Habitat alias target changed; cleanup refused".into());
            }
            unlink_exact(&record.target)?;
        }
        let slot_record = slot.join(HABITAT_ALIAS_RECORD);
        if storage::exists(&slot_record)? {
            std::fs::remove_file(&slot_record).map_err(storage::error)?;
        }
        std::fs::remove_dir(slot).map_err(storage::error)?;
        std::fs::remove_file(record_path).map_err(storage::error)
    }
}

#[cfg(windows)]
fn validate_slot_contents(slot: &Path, record: &HabitatAliasRecord) -> Result<(), String> {
    if storage::directory_identity(slot)? != record.slot_identity {
        return Err("Habitat alias slot identity changed; cleanup refused".into());
    }
    for entry in std::fs::read_dir(slot).map_err(storage::error)? {
        let name = entry.map_err(storage::error)?.file_name();
        if name != std::ffi::OsStr::new("h") && name != std::ffi::OsStr::new(HABITAT_ALIAS_RECORD) {
            return Err("Unknown habitat alias slot state retained; cleanup refused".into());
        }
    }
    let slot_record = slot.join(HABITAT_ALIAS_RECORD);
    if let Some(existing) = storage::read_record::<HabitatAliasRecord>(&slot_record)? {
        if existing != *record {
            return Err("Habitat alias slot ownership record changed; cleanup refused".into());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn unlink_exact(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage::error)?;
    if !crate::utils::fs::is_directory_link(&metadata) {
        return Err("Habitat alias occupant is not a directory link".into());
    }
    std::fs::remove_dir(path)
        .or_else(|_| std::fs::remove_file(path))
        .map_err(storage::error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_habitat_cwd_preserves_the_projected_descendant() {
        let habitat = Path::new(r"C:\long\agents\agent\habitat");
        let workspace = Path::new(r"D:\project");
        assert_eq!(
            relative_habitat_cwd(habitat, workspace, &habitat.join("workspace")),
            Some(PathBuf::from("workspace"))
        );
        assert_eq!(
            relative_habitat_cwd(habitat, workspace, habitat),
            Some(PathBuf::new())
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn non_windows_never_allocates_a_habitat_alias() {
        assert_eq!(
            prepare_habitat_cwd_alias(
                "agent",
                Some(Path::new("/home/agent/habitat")),
                Path::new("/project"),
                Path::new("/home/agent/habitat/workspace"),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    #[cfg(windows)]
    fn short_windows_cwd_never_allocates_a_habitat_alias() {
        assert_eq!(
            prepare_habitat_cwd_alias(
                "agent",
                Some(Path::new(r"C:\h")),
                Path::new(r"C:\project"),
                Path::new(r"C:\h\workspace"),
            )
            .unwrap(),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn observed_259_unit_cwd_selects_the_habitat_alias() {
        fn path_with_units(target: usize) -> PathBuf {
            let prefix = PathBuf::from(r"C:\seed");
            let filler = "x".repeat(target - windows_path_units(&prefix) - 1);
            prefix.join(filler)
        }

        let at_natural_boundary = path_with_units(258);
        let at_create_process_failure = path_with_units(259);
        assert_eq!(windows_path_units(&at_natural_boundary), 258);
        assert_eq!(windows_path_units(&at_create_process_failure), 259);
        assert!(!needs_habitat_cwd_alias(&at_natural_boundary));
        assert!(needs_habitat_cwd_alias(&at_create_process_failure));
    }

    #[cfg(windows)]
    mod windows {
        use super::*;
        use portable_pty::win::conpty_load_diagnostics;
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::{Read, Write};
        use std::os::windows::ffi::OsStrExt;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        struct Fixture {
            _temp: tempfile::TempDir,
            home: PathBuf,
            habitat: PathBuf,
            workspace: PathBuf,
            root: PathBuf,
        }

        impl Fixture {
            fn new() -> Self {
                let mut builder = tempfile::Builder::new();
                builder.prefix("ch");
                let temp = builder.tempdir().expect("temp dir");
                let base = temp.path().canonicalize().expect("canonical temp dir");
                let mut home = base.join("wardian-home");
                while units(&home) < 235 {
                    home.push("long-home-segment-abcdefghijkl");
                }
                let habitat = home.join("agents").join("agent").join("habitat");
                let workspace = base.join("real-workspace");
                std::fs::create_dir_all(&habitat).expect("create habitat");
                std::fs::create_dir_all(&workspace).expect("create workspace");
                std::fs::write(habitat.join("AGENTS.md"), "habitat ancestry").expect("agents");
                std::fs::write(workspace.join("marker.txt"), "workspace target")
                    .expect("workspace marker");
                crate::utils::fs::create_directory_link(&workspace, &habitat.join("workspace"))
                    .expect("workspace link");
                Self {
                    _temp: temp,
                    home,
                    habitat,
                    workspace,
                    root: base.join("c"),
                }
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir(&self.root);
            }
        }

        fn units(path: &Path) -> usize {
            path.as_os_str().encode_wide().count()
        }

        fn with_fixture<T>(fixture: &Fixture, operation: impl FnOnce() -> T) -> T {
            let previous = super::super::TEST_ROOTS
                .with(|roots| roots.replace(Some(vec![fixture.root.clone()])));
            let result = operation();
            super::super::TEST_ROOTS.with(|roots| roots.replace(previous));
            result
        }

        #[test]
        fn saved_pi_alias_must_belong_to_the_same_habitat() {
            let fixture = Fixture::new();
            let alias = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("prepare owned alias")
            });
            let saved = alias.join("workspace");
            assert!(is_owned_habitat_workspace_alias_in(
                &fixture.home,
                "agent",
                &fixture.habitat,
                &saved
            ));
            assert!(!is_owned_habitat_workspace_alias_in(
                &fixture.home,
                "another-agent",
                &fixture.habitat,
                &saved
            ));
            assert!(!is_owned_habitat_workspace_alias_in(
                &fixture.home,
                "agent",
                &fixture.habitat,
                &fixture.workspace
            ));
        }

        #[derive(Debug)]
        struct InertPtyResult {
            spawn_succeeded: bool,
            elapsed: Duration,
            exit_code: Option<u32>,
            success: Option<bool>,
            killed: bool,
            output: Vec<u8>,
            error: Option<String>,
        }

        fn finish_reader(
            receiver: mpsc::Receiver<Result<Vec<u8>, String>>,
            thread: std::thread::JoinHandle<()>,
        ) -> Result<Vec<u8>, String> {
            match receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(result) => {
                    thread
                        .join()
                        .map_err(|_| "PTY reader thread panicked".to_owned())?;
                    result
                }
                Err(error) => {
                    drop(thread);
                    Err(format!("PTY reader did not drain before deadline: {error}"))
                }
            }
        }

        fn run_inert_pty(cwd: &Path) -> InertPtyResult {
            let started = Instant::now();
            eprintln!(
                "inert phase=open cwd_utf16={} cwd={cwd:?} conpty={:?}",
                units(cwd),
                conpty_load_diagnostics()
            );
            let pty = match native_pty_system().openpty(PtySize::default()) {
                Ok(pty) => pty,
                Err(error) => {
                    return InertPtyResult {
                        spawn_succeeded: false,
                        elapsed: started.elapsed(),
                        exit_code: None,
                        success: None,
                        killed: false,
                        output: Vec::new(),
                        error: Some(format!("openpty failed: {error:#}")),
                    };
                }
            };
            let mut reader = match pty.master.try_clone_reader() {
                Ok(reader) => reader,
                Err(error) => {
                    return InertPtyResult {
                        spawn_succeeded: false,
                        elapsed: started.elapsed(),
                        exit_code: None,
                        success: None,
                        killed: false,
                        output: Vec::new(),
                        error: Some(format!("clone reader failed: {error:#}")),
                    };
                }
            };
            let mut writer = match pty.master.take_writer() {
                Ok(writer) => writer,
                Err(error) => {
                    return InertPtyResult {
                        spawn_succeeded: false,
                        elapsed: started.elapsed(),
                        exit_code: None,
                        success: None,
                        killed: false,
                        output: Vec::new(),
                        error: Some(format!("take writer failed: {error:#}")),
                    };
                }
            };
            let (sender, receiver) = mpsc::channel();
            let reader_thread = std::thread::spawn(move || {
                let mut output = Vec::new();
                let mut buffer = [0u8; 1024];
                let mut query_replied = false;
                let mut error = None;
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            output.extend_from_slice(&buffer[..count]);
                            if !query_replied
                                && output.windows(4).any(|window| window == b"\x1b[6n")
                            {
                                query_replied = true;
                                eprintln!(
                                    "inert phase=reader-query observed=ESC[6n response=ESC[1;1R"
                                );
                                if let Err(value) =
                                    writer.write_all(b"\x1b[1;1R").and_then(|_| writer.flush())
                                {
                                    error = Some(format!("write DSR response failed: {value}"));
                                    break;
                                }
                            }
                        }
                        Err(value) => {
                            error = Some(format!("read PTY output failed: {value}"));
                            break;
                        }
                    }
                }
                let result = error.map_or(Ok(output), Err);
                let _ = sender.send(result);
            });
            eprintln!("inert phase=spawn cwd={cwd:?}");
            let mut command = CommandBuilder::new("cmd.exe");
            command.args(["/c", "exit", "0"]);
            command.cwd(cwd);
            let mut child = match pty.slave.spawn_command(command) {
                Ok(child) => child,
                Err(error) => {
                    drop(pty);
                    let output = finish_reader(receiver, reader_thread).unwrap_or_default();
                    return InertPtyResult {
                        spawn_succeeded: false,
                        elapsed: started.elapsed(),
                        exit_code: None,
                        success: None,
                        killed: false,
                        output,
                        error: Some(format!("spawn failed: {error:#}")),
                    };
                }
            };
            eprintln!(
                "inert phase=spawned pid={:?} cwd={cwd:?}",
                child.process_id()
            );

            let deadline = Instant::now() + Duration::from_secs(10);
            let mut status = None;
            let mut error = None;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(value)) => {
                        status = Some(value);
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                    Err(value) => {
                        error = Some(format!("try_wait failed: {value}"));
                        break;
                    }
                }
            }

            let mut killed = false;
            if status.is_none() {
                killed = true;
                eprintln!(
                    "inert phase=child-wait timeout; killing owned pid={:?}",
                    child.process_id()
                );
                if let Err(value) = child.kill() {
                    error = Some(format!("kill after child-wait timeout failed: {value}"));
                }
                let kill_deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < kill_deadline {
                    match child.try_wait() {
                        Ok(Some(value)) => {
                            status = Some(value);
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                        Err(value) => {
                            error = Some(format!("try_wait after kill failed: {value}"));
                            break;
                        }
                    }
                }
            }
            eprintln!("inert phase=child-wait status={status:?} killed={killed}");
            let (exit_code, success) = status
                .as_ref()
                .map(|value| (Some(value.exit_code()), Some(value.success())))
                .unwrap_or((None, None));
            drop(child);
            drop(pty);
            eprintln!("inert phase=pty-drop");
            let output = match finish_reader(receiver, reader_thread) {
                Ok(output) => output,
                Err(value) => {
                    error = Some(
                        error.map_or(value.clone(), |previous| format!("{previous}; {value}")),
                    );
                    Vec::new()
                }
            };
            eprintln!(
                "inert phase=reader-join bytes={} output={:?} elapsed_ms={} error={error:?}",
                output.len(),
                String::from_utf8_lossy(&output),
                started.elapsed().as_millis()
            );
            InertPtyResult {
                spawn_succeeded: true,
                elapsed: started.elapsed(),
                exit_code,
                success,
                killed,
                output,
                error,
            }
        }

        #[test]
        fn owned_alias_reuses_ancestry_and_removes_only_its_junction() {
            let fixture = Fixture::new();
            let logical = fixture.habitat.join("workspace");
            let alias = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("prepare alias")
            });
            let alias_cwd = alias.join("workspace");
            assert!(units(&logical) > WINDOWS_MAX_PATH_UNITS);
            assert!(units(&alias_cwd) < WINDOWS_MAX_PATH_UNITS);
            assert_eq!(
                std::fs::read_to_string(alias_cwd.parent().unwrap().join("AGENTS.md"))
                    .expect("read through whole-habitat alias"),
                "habitat ancestry"
            );
            assert_eq!(
                std::fs::read_to_string(alias_cwd.join("marker.txt")).expect("read workspace"),
                "workspace target"
            );
            let reused = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("reuse alias")
            });
            assert_eq!(reused, alias);
            cleanup_habitat_alias(&fixture.home, "agent").expect("cleanup alias");
            assert!(!alias.exists());
            assert!(fixture.habitat.join("AGENTS.md").exists());
            assert!(fixture.workspace.join("marker.txt").exists());
        }

        #[test]
        fn foreign_slot_record_blocks_publication_without_creating_a_junction() {
            let fixture = Fixture::new();
            let alias = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("prepare alias")
            });
            let record_path = fixture
                .home
                .join("agents")
                .join("agent")
                .join(HABITAT_ALIAS_RECORD);
            let record = super::super::storage::read_record::<HabitatAliasRecord>(&record_path)
                .expect("read agent alias record")
                .expect("agent alias record");
            let slot = record.target.parent().expect("alias slot").to_path_buf();
            let slot_record = slot.join(HABITAT_ALIAS_RECORD);
            unlink_exact(&record.target).expect("remove owned junction for setup");
            std::fs::remove_file(&slot_record).expect("remove owned slot record for setup");

            let mut foreign = record.clone();
            foreign.agent_id = "foreign-agent".to_owned();
            foreign.token = uuid::Uuid::new_v4().to_string();
            super::super::storage::publish_new(&slot_record, &foreign)
                .expect("publish foreign slot record");
            assert!(!super::super::storage::exists(&alias).expect("check target before retry"));

            let error = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect_err("foreign slot ownership must reject publication")
            });
            assert!(error.contains("slot ownership record changed"), "{error}");
            assert!(!super::super::storage::exists(&alias).expect("check target after retry"));
            assert_eq!(
                super::super::storage::read_record::<HabitatAliasRecord>(&slot_record)
                    .expect("read retained foreign record"),
                Some(foreign)
            );
        }

        #[test]
        fn cleanup_rejects_a_source_record_moved_outside_the_agent_habitat() {
            let fixture = Fixture::new();
            let alias = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("prepare alias")
            });
            let record_path = fixture
                .home
                .join("agents")
                .join("agent")
                .join(HABITAT_ALIAS_RECORD);
            let record = super::super::storage::read_record::<HabitatAliasRecord>(&record_path)
                .expect("read agent alias record")
                .expect("agent alias record");
            let foreign_habitat = fixture.root.join("foreign-habitat");
            std::fs::create_dir_all(&foreign_habitat).expect("create foreign habitat");
            let foreign_habitat = canonical(&foreign_habitat).expect("canonical foreign habitat");
            let mut moved = record.clone();
            moved.habitat = foreign_habitat.clone();
            moved.habitat_identity = super::super::storage::directory_identity(&foreign_habitat)
                .expect("foreign habitat identity");
            let bytes = serde_json::to_vec_pretty(&moved).expect("serialize moved record");
            std::fs::write(&record_path, bytes).expect("write moved source record");

            let error = cleanup_habitat_alias(&fixture.home, "agent")
                .expect_err("cleanup must reject a moved source record");
            assert!(
                error.contains("outside the owning agent habitat"),
                "{error}"
            );
            assert!(super::super::storage::exists(&alias).expect("check retained junction"));
            assert_eq!(
                canonical(&alias).expect("resolve retained junction"),
                canonical(&fixture.habitat).expect("resolve retained habitat")
            );

            let bytes = serde_json::to_vec_pretty(&record).expect("restore source record");
            std::fs::write(&record_path, bytes).expect("restore source record");
            cleanup_habitat_alias(&fixture.home, "agent").expect("cleanup restored alias");
            assert!(!super::super::storage::exists(&alias).expect("check cleaned junction"));
            assert!(fixture.workspace.join("marker.txt").exists());
        }

        #[test]
        fn inert_pty_uses_the_short_alias_and_preserves_exit_success() {
            let fixture = Fixture::new();
            let alias = with_fixture(&fixture, || {
                prepare_alias_for_home(&fixture.home, "agent", &fixture.habitat)
                    .expect("prepare alias")
            });
            let plain = run_inert_pty(&fixture.workspace);
            let aliased = run_inert_pty(&alias.join("workspace"));
            cleanup_habitat_alias(&fixture.home, "agent").expect("cleanup alias");
            eprintln!(
                "inert plain result: elapsed_ms={} output_bytes={} error={:?} details={plain:?}",
                plain.elapsed.as_millis(),
                plain.output.len(),
                plain.error
            );
            eprintln!(
                "inert alias result: elapsed_ms={} output_bytes={} error={:?} details={aliased:?}",
                aliased.elapsed.as_millis(),
                aliased.output.len(),
                aliased.error
            );
            assert!(plain.spawn_succeeded, "plain inert PTY failed: {plain:?}");
            assert_eq!(
                plain.exit_code,
                Some(0),
                "plain inert PTY failed: {plain:?}"
            );
            assert_eq!(
                plain.success,
                Some(true),
                "plain inert PTY failed: {plain:?}"
            );
            assert!(!plain.killed, "plain inert PTY timed out: {plain:?}");
            assert!(
                aliased.spawn_succeeded,
                "aliased inert PTY failed: {aliased:?}"
            );
            assert_eq!(
                aliased.exit_code,
                Some(0),
                "aliased inert PTY failed: {aliased:?}"
            );
            assert_eq!(
                aliased.success,
                Some(true),
                "aliased inert PTY failed: {aliased:?}"
            );
            assert!(!aliased.killed, "aliased inert PTY timed out: {aliased:?}");
        }
    }
}
