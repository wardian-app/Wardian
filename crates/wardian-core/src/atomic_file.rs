use serde::Serialize;
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicWriteRole {
    Primary,
    Backup,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicFaultPoint {
    BeforeTempSync(AtomicWriteRole),
    AfterTempSync(AtomicWriteRole),
    BeforeReplace(AtomicWriteRole),
    AfterReplace(AtomicWriteRole),
    #[cfg_attr(windows, allow(dead_code))]
    BeforeParentSync(AtomicWriteRole),
    #[cfg_attr(windows, allow(dead_code))]
    AfterParentSync(AtomicWriteRole),
}

pub(crate) trait AtomicFaultHook {
    fn check(&mut self, point: AtomicFaultPoint) -> io::Result<()>;
}

pub(crate) struct NoAtomicFault;

impl AtomicFaultHook for NoAtomicFault {
    fn check(&mut self, _point: AtomicFaultPoint) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_bytes_atomic_durable(path, &bytes)
}

pub(crate) fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("wardian");
    path.with_file_name(format!(".{file_name}.tmp"))
}

pub(crate) fn stage_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    stage_bytes_atomic_with_hook(path, bytes, AtomicWriteRole::Other, &mut NoAtomicFault)
}

pub(crate) fn stage_bytes_atomic_with_hook(
    path: &Path,
    bytes: &[u8],
    role: AtomicWriteRole,
    hook: &mut impl AtomicFaultHook,
) -> io::Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("wardian");
    let tmp_path = path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp_path)?;
    file.write_all(bytes)?;
    hook.check(AtomicFaultPoint::BeforeTempSync(role))?;
    file.sync_all()?;
    hook.check(AtomicFaultPoint::AfterTempSync(role))?;
    Ok(tmp_path)
}

pub(crate) fn replace_staged_atomic_durable(from: &Path, to: &Path) -> io::Result<()> {
    replace_staged_atomic_durable_with_hook(from, to, AtomicWriteRole::Other, &mut NoAtomicFault)
}

pub(crate) fn replace_staged_atomic_durable_with_hook(
    from: &Path,
    to: &Path,
    role: AtomicWriteRole,
    hook: &mut impl AtomicFaultHook,
) -> io::Result<()> {
    hook.check(AtomicFaultPoint::BeforeReplace(role))?;
    replace_file_without_parent_sync(from, to)?;
    hook.check(AtomicFaultPoint::AfterReplace(role))?;
    #[cfg(not(windows))]
    if let Some(parent) = to.parent() {
        hook.check(AtomicFaultPoint::BeforeParentSync(role))?;
        fs::File::open(parent)?.sync_all()?;
        hook.check(AtomicFaultPoint::AfterParentSync(role))?;
    }
    Ok(())
}

pub(crate) fn write_bytes_atomic_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = stage_bytes_atomic(path, bytes)?;
    replace_staged_atomic_durable(&staged, path)
}

pub(crate) fn write_bytes_atomic_durable_with_hook(
    path: &Path,
    bytes: &[u8],
    role: AtomicWriteRole,
    hook: &mut impl AtomicFaultHook,
) -> io::Result<()> {
    let staged = stage_bytes_atomic_with_hook(path, bytes, role, hook)?;
    replace_staged_atomic_durable_with_hook(&staged, path, role, hook)
}

pub(crate) fn cleanup_atomic_temps(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let legacy_name = format!(".{file_name}.tmp");
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let owned = name
            .to_str()
            .is_some_and(|name| is_owned_atomic_temp_name(name, file_name, &legacy_name));
        if owned && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn is_owned_atomic_temp_name(name: &str, target_name: &str, legacy_name: &str) -> bool {
    if name == legacy_name {
        return true;
    }
    let prefix = format!(".{target_name}.");
    let Some(identifier) = name
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".tmp"))
    else {
        return false;
    };
    identifier.len() == 32 && identifier.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(not(windows))]
pub(crate) fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    replace_staged_atomic_durable(from, to)
}

#[cfg(windows)]
pub(crate) fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    replace_staged_atomic_durable(from, to)
}

#[cfg(not(windows))]
fn replace_file_without_parent_sync(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file_without_parent_sync(from: &Path, to: &Path) -> io::Result<()> {
    let from = wide_null(from.as_os_str());
    let to = wide_null(to.as_os_str());
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    // Windows std::fs::rename does not replace an existing destination.
    let replaced = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
}

#[cfg(test)]
mod tests {
    #[test]
    fn workbench_owned_atomic_temp_names_require_the_exact_legacy_or_uuid_grammar() {
        let target = "workbench.json";
        let legacy = ".workbench.json.tmp";
        assert!(super::is_owned_atomic_temp_name(legacy, target, legacy));
        assert!(super::is_owned_atomic_temp_name(
            ".workbench.json.0123456789abcdef0123456789ABCDEF.tmp",
            target,
            legacy,
        ));
        for name in [
            ".workbench.json.user-copy.tmp",
            ".workbench.json.deadbeef.tmp",
            ".workbench.json.0123456789abcdef0123456789abcdeg.tmp",
            ".workbench.json.0123456789abcdef0123456789abcde.tmp",
        ] {
            assert!(!super::is_owned_atomic_temp_name(name, target, legacy));
        }
    }

    #[test]
    fn workbench_temp_cleanup_preserves_directories_and_symbolic_links() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("workbench.json");
        let directory = temp
            .path()
            .join(".workbench.json.0123456789abcdef0123456789abcdef.tmp");
        std::fs::create_dir(&directory).expect("temp-shaped directory");

        let target = temp.path().join("user-copy.json");
        std::fs::write(&target, b"keep").expect("symlink target");
        let link = temp
            .path()
            .join(".workbench.json.fedcba9876543210fedcba9876543210.tmp");
        #[cfg(unix)]
        let link_created = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let link_created = std::os::windows::fs::symlink_file(&target, &link).is_ok();

        super::cleanup_atomic_temps(&path).expect("cleanup");

        assert!(directory.is_dir());
        assert_eq!(std::fs::read(&target).expect("target remains"), b"keep");
        if link_created {
            assert!(std::fs::symlink_metadata(&link)
                .expect("link remains")
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn write_json_atomic_replaces_existing_json_and_removes_temp_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("state.json");
        std::fs::write(&path, r#"{"old":true}"#).expect("old json");

        super::write_json_atomic(&path, &serde_json::json!({"new": true})).expect("atomic write");

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("replacement json"))
                .expect("valid json");
        assert_eq!(value, serde_json::json!({"new": true}));
        assert!(!temp.path().join(".state.json.tmp").exists());
    }
}
