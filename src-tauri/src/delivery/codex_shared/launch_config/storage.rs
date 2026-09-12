//! Checked UTF-8 snapshots and adjacent atomic publication for config and journal.
use super::{failure, io_failure, CodexSharedError};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

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
    let mut file = options.open(path).map_err(io_failure)?;
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
    let mut temporary = tempfile::Builder::new()
        .prefix(".wardian-launch-")
        .tempfile_in(path.parent().expect("config has an owned parent"))
        .map_err(io_failure)?;
    temporary.write_all(bytes).map_err(io_failure)?;
    temporary.as_file().sync_all().map_err(io_failure)?;
    // Detect observed edits, replacements (even identical bytes), new links or
    // removal. External editors share no lock: the final check/rename race is
    // bounded, not an OS compare-and-swap guarantee.
    if let Some((peer_path, snapshot)) = peer {
        compare_before_publish(peer_path, snapshot)?;
    }
    compare_before_publish(path, before)?;
    temporary
        .persist(path)
        .map_err(|error| io_failure(error.error))?;
    Ok(())
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
    compare_before_publish(path, before)?;
    fs::remove_file(path).map_err(io_failure)
}
