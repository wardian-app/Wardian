//! No-follow ownership records and the agent-local nonblocking preparation lock.
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[must_use = "hold through recovery, migration, projection and MCP registration"]
pub(crate) struct HomePreparationGuard(File);

impl Drop for HomePreparationGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub(super) fn lock(path: &Path) -> Result<HomePreparationGuard, String> {
    let mut options = options();
    options.write(true).create_new(true);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            options.create_new(false).open(path).map_err(self::error)?
        }
        Err(error) => return Err(self::error(error)),
    };
    regular(&file)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|error| format!("Codex home preparation busy or lock unavailable: {error}"))?;
    let guard = HomePreparationGuard(file);
    options.create_new(false);
    let current = options.open(path).map_err(self::error)?;
    regular(&current)?;
    if identity(&current)?.0 != identity(&guard.0)?.0 {
        return Err("Codex preparation lock changed during acquisition".into());
    }
    Ok(guard)
}

pub(super) fn error(error: std::io::Error) -> String {
    format!("Codex home filesystem operation failed: {error}")
}

pub(super) fn exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(self::error(error)),
    }
}

pub(super) fn is_link(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(crate::utils::fs::is_directory_link(&metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(self::error(error)),
    }
}

pub(super) fn plain_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_dir() || crate::utils::fs::is_directory_link(&metadata) {
        return Err(format!(
            "Expected an unlinked owned directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000 | 0x0200_0000);
    }
    options
}

fn regular(file: &File) -> Result<(), String> {
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_file()
        || crate::utils::fs::is_directory_link(&metadata)
        || identity(file)?.1 != 1
    {
        return Err(
            "Codex ownership/lock record must be regular, unlinked and singly linked".into(),
        );
    }
    Ok(())
}

pub(super) fn directory_identity(path: &Path) -> Result<(u64, u64), String> {
    plain_directory(path)?;
    let file = options().open(path).map_err(error)?;
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_dir() || crate::utils::fs::is_directory_link(&metadata) {
        return Err("Codex physical directory changed while opening it".into());
    }
    Ok(identity(&file)?.0)
}

pub(super) fn read_record<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    if !exists(path)? {
        return Ok(None);
    }
    let file = options().open(path).map_err(error)?;
    regular(&file)?;
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() > 65_536 {
        return Err("Oversized Codex ownership record".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "Codex ownership record is not UTF-8")?;
    serde_json::from_str(text)
        .map(Some)
        .map_err(|error| format!("Malformed Codex ownership record: {error}"))
}

pub(super) fn publish_new<T: Serialize>(path: &Path, record: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 || exists(path)? {
        return Err(
            "Codex ownership record already exists or exceeds its size limit; retained".into(),
        );
    }
    let parent = path.parent().ok_or("Ownership record has no parent")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
    temp.write_all(&bytes).map_err(error)?;
    temp.as_file().sync_all().map_err(error)?;
    temp.persist_noclobber(path)
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(error)?;
    Ok(())
}

#[cfg(unix)]
fn identity(file: &File) -> Result<((u64, u64), u64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(error)?;
    Ok(((metadata.dev(), metadata.ino()), metadata.nlink()))
}

#[cfg(windows)]
fn identity(file: &File) -> Result<((u64, u64), u64), String> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: live owned handle and the existing winapi ABI structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) } == 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    // SAFETY: successful call initialized the complete structure.
    let info = unsafe { info.assume_init() };
    Ok((
        (
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ),
        u64::from(info.nNumberOfLinks),
    ))
}
