//! Checked UTF-8 snapshots and adjacent atomic publication for config and journal.
use super::{failure, io_failure, CodexSharedError};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

// Windows readers may deny delete sharing even though the file is writable.
// Bound the wait to 200 ms per mutation; never bypass the snapshot checks.
fn retry_sharing_conflict(error: &std::io::Error, retries: &mut u8) -> bool {
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(5 | 32 | 33)) && *retries < 8 {
        *retries += 1;
        std::thread::sleep(std::time::Duration::from_millis(25));
        return true;
    }
    #[cfg(not(windows))]
    let _ = (error, retries);
    false
}

fn operation_failure(operation: &str, path: &Path, error: std::io::Error) -> CodexSharedError {
    // Do not expose private home paths or arbitrary filenames in launch errors.
    let file = if path.file_name().is_some_and(|name| name == "config.toml") {
        "config.toml"
    } else {
        "launch journal"
    };
    failure(&format!(
        "Codex launch file I/O failed while {operation} {file}: {error}"
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) text: String,
    identity: (u64, u64),
}

pub(super) fn validate_home(home: &Path) -> Result<(), CodexSharedError> {
    if !home.is_absolute() {
        return Err(failure("Codex launch file requires an absolute owned home"));
    }
    let metadata = fs::symlink_metadata(home).map_err(io_failure)?;
    if !metadata.is_dir() || crate::utils::fs::is_directory_link(&metadata) {
        return Err(failure(
            "Codex launch home must be a private unlinked directory",
        ));
    }
    Ok(())
}

pub(super) fn read_snapshot(path: &Path) -> Result<Option<Snapshot>, CodexSharedError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => reject_destination(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure(error)),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // Check the opened object too: do not follow a link substituted after lstat.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let mut file = options
        .open(path)
        .map_err(|error| operation_failure("opening", path, error))?;
    reject_destination(&file.metadata().map_err(io_failure)?)?;
    let (volume, index, links) = file_identity(&file).map_err(io_failure)?;
    if links != 1 {
        return Err(failure("hard-linked Codex launch file is not writable"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_failure)?;
    let text =
        String::from_utf8(bytes).map_err(|_| failure("Codex launch file must be valid UTF-8"))?;
    Ok(Some(Snapshot {
        text,
        identity: (volume, index),
    }))
}

fn reject_destination(metadata: &fs::Metadata) -> Result<(), CodexSharedError> {
    if !metadata.is_file() || crate::utils::fs::is_directory_link(metadata) {
        return Err(failure("Codex launch file must be a regular unlinked file"));
    }
    Ok(())
}

pub(super) fn compare_before_publish(
    path: &Path,
    before: &Option<Snapshot>,
) -> Result<(), CodexSharedError> {
    validate_home(path.parent().expect("config has an owned parent"))?;
    if &read_snapshot(path)? != before {
        return Err(failure("Codex launch file changed during preparation"));
    }
    Ok(())
}

pub(super) fn publish(
    path: &Path,
    before: &Option<Snapshot>,
    bytes: &[u8],
    peer: Option<(&Path, &Option<Snapshot>)>,
) -> Result<(), CodexSharedError> {
    // NamedTempFile uses create_new and an adjacent random name. Like managed
    // MCP registration, persist atomically replaces the destination on Unix and
    // Windows; its RAII guard removes the temporary on a pre-publish failure.
    // Windows needs the existing parent's verbatim path for both siblings. Keep
    // compare-before-publish on the caller's unresolved logical paths below.
    let parent = path.parent().expect("config has an owned parent");
    #[cfg(windows)]
    let parent = parent.canonicalize().map_err(io_failure)?;
    #[cfg(windows)]
    let destination = parent.join(path.file_name().expect("config has a file name"));
    #[cfg(not(windows))]
    let destination = path.to_path_buf();
    let mut temporary = tempfile::Builder::new()
        .prefix(".wardian-launch-")
        .tempfile_in(parent)
        .map_err(io_failure)?;
    temporary.write_all(bytes).map_err(io_failure)?;
    temporary.as_file().sync_all().map_err(io_failure)?;
    let mut retries = 0;
    loop {
        // Recheck after EVERY wait: readers or external editors can change
        // either file during contention. This remains a bounded check/rename
        // race, not an OS compare-and-swap guarantee.
        if let Some((peer_path, snapshot)) = peer {
            compare_before_publish(peer_path, snapshot)?;
        }
        compare_before_publish(path, before)?;
        match temporary.persist(&destination) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if !retry_sharing_conflict(&error.error, &mut retries) {
                    return Err(operation_failure("replacing", path, error.error));
                }
                temporary = error.file;
            }
        }
    }
}

#[cfg(unix)]
fn file_identity(file: &File) -> std::io::Result<(u64, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino(), metadata.nlink()))
}

#[cfg(windows)]
fn file_identity(file: &File) -> std::io::Result<(u64, u64, u64)> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the file owns a valid handle and the typed Windows API initializes
    // the complete output structure when it reports success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful call initialized every field above.
    let information = unsafe { information.assume_init() };
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
        u64::from(information.nNumberOfLinks),
    ))
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File) -> std::io::Result<(u64, u64, u64)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Codex config link checks unavailable",
    ))
}

pub(super) fn home_identity(home: &Path) -> Result<(u64, u64), CodexSharedError> {
    validate_home(home)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0220_0000); // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(home).map_err(io_failure)?;
    let metadata = file.metadata().map_err(io_failure)?;
    if !metadata.is_dir() || crate::utils::fs::is_directory_link(&metadata) {
        return Err(failure("Codex launch home changed while opening"));
    }
    let (volume, index, _) = file_identity(&file).map_err(io_failure)?;
    Ok((volume, index))
}

pub(super) fn remove(path: &Path, before: &Option<Snapshot>) -> Result<(), CodexSharedError> {
    let mut retries = 0;
    loop {
        compare_before_publish(path, before)?;
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if retry_sharing_conflict(&error, &mut retries) => {}
            Err(error) => return Err(operation_failure("removing", path, error)),
        }
    }
}
