//! Stable, explicitly scoped offline input. Home resolution is used only to
//! exclude Wardian locations. Files are opened read-only.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Missing,
    Unsafe,
    Invalid,
    Changed,
    Limit,
}

impl From<io::Error> for Fault {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::Missing,
            _ => Self::Invalid,
        }
    }
}

fn native_path(path: &Path) -> bool {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    }
    #[cfg(not(windows))]
    true
}

fn path_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy();
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path).to_lowercase())
    }
    #[cfg(not(windows))]
    path.components().collect()
}

fn known_homes() -> impl Iterator<Item = PathBuf> {
    let configured = std::env::var("WARDIAN_HOME").ok();
    let configured = configured
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let profiles = ["USERPROFILE", "HOME"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain(dirs::home_dir());
    configured
        .into_iter()
        .chain(profiles.map(|profile| profile.join(".wardian")))
        .chain(crate::utils::fs::get_wardian_home())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    let left = path_key(left);
    let right = path_key(right);
    left.starts_with(&right) || right.starts_with(&left)
}

pub(in crate::chat_cache_admin) fn validate(path: &Path) -> Result<PathBuf, String> {
    const SEPARATE_COPY: &str = "verify-archive requires a separate absolute offline copy";
    if !native_path(path) {
        return Err(SEPARATE_COPY.into());
    }
    let known = known_homes()
        .map(std::path::absolute)
        .collect::<io::Result<Vec<_>>>()
        .map_err(|_| "cannot resolve known Wardian home")?;
    if known.iter().any(|home| overlaps(path, home)) {
        return Err(SEPARATE_COPY.into());
    }
    if !checked_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("verify-archive requires an existing non-link copy directory".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "cannot resolve offline copy")?;
    if !native_path(&canonical) {
        return Err(SEPARATE_COPY.into());
    }
    for home in known {
        if overlaps(&canonical, &home) {
            return Err(SEPARATE_COPY.into());
        }
        match home.canonicalize() {
            Ok(physical) if overlaps(&canonical, &physical) => return Err(SEPARATE_COPY.into()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot resolve known Wardian home".into()),
        }
    }
    Ok(canonical)
}

fn reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn checked_metadata(path: &Path) -> Result<Metadata, Fault> {
    let mut ancestors: Vec<_> = path
        .ancestors()
        .skip(1)
        .filter(|part| part.is_absolute())
        .collect();
    while let Some(part) = ancestors.pop() {
        let metadata = fs::symlink_metadata(part)?;
        if !metadata.is_dir() || reparse(&metadata) {
            return Err(Fault::Unsafe);
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if reparse(&metadata) {
        return Err(Fault::Unsafe);
    }
    Ok(metadata)
}

// Verify a single hard link, since a copy linked to another store is not an
// independently owned input. Check descriptor identity as part of the witness.
#[cfg(windows)]
fn identity(file: &File, path: &Path) -> Result<(u64, u64), Fault> {
    use std::os::windows::io::AsRawHandle;
    use winapi::um::fileapi::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    };
    let handle = file.as_raw_handle().cast();
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: The handle belongs to an open File and the output is valid on success.
    if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 {
        return Err(Fault::Invalid);
    }
    let info = unsafe { info.assume_init() };
    if info.nNumberOfLinks != 1 {
        return Err(Fault::Unsafe);
    }
    let mut actual = vec![0u16; 32768];
    // SAFETY: The buffer has the stated capacity and remains live for the call.
    let len =
        unsafe { GetFinalPathNameByHandleW(handle, actual.as_mut_ptr(), actual.len() as u32, 0) }
            as usize;
    if len == 0 || len >= actual.len() {
        return Err(Fault::Invalid);
    }
    use std::os::windows::ffi::OsStringExt;
    let actual = PathBuf::from(std::ffi::OsString::from_wide(&actual[..len]));
    if path_key(&actual) != path_key(path) {
        return Err(Fault::Unsafe);
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(unix)]
fn identity(file: &File, path: &Path) -> Result<(u64, u64), Fault> {
    use std::os::unix::fs::MetadataExt;
    let open = file.metadata()?;
    let current = checked_metadata(path)?;
    if open.nlink() != 1 || (open.dev(), open.ino()) != (current.dev(), current.ino()) {
        return Err(Fault::Unsafe);
    }
    Ok((open.dev(), open.ino()))
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &File, _: &Path) -> Result<(u64, u64), Fault> {
    Err(Fault::Unsafe)
}

#[derive(PartialEq, Eq)]
struct Stamp {
    identity: Option<(u64, u64)>,
    bytes: u64,
    modified: SystemTime,
}

fn open(path: &Path) -> Result<(File, Stamp), Fault> {
    if !checked_metadata(path)?.is_file() {
        return Err(Fault::Unsafe);
    }
    let file = open_descriptor(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || reparse(&metadata) {
        return Err(Fault::Unsafe);
    }
    let stamp = Stamp {
        identity: Some(identity(&file, path)?),
        bytes: metadata.len(),
        modified: metadata.modified()?,
    };
    Ok((file, stamp))
}

#[cfg(unix)]
fn open_descriptor(path: &Path) -> Result<File, Fault> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(Fault::Unsafe);
    }
    let mut components = components.peekable();
    let mut parent = File::open("/")?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(Fault::Unsafe);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| Fault::Unsafe)?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | if components.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: The directory descriptor and NUL-terminated name are live.
        // Without create flags, openat takes no mode argument.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a fresh owned descriptor.
        parent = unsafe { File::from_raw_fd(fd) };
    }
    Ok(parent)
}

#[cfg(windows)]
fn open_descriptor(path: &Path) -> Result<File, Fault> {
    use std::os::windows::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x0020_0000)
        .open(path)?)
}

#[cfg(not(any(unix, windows)))]
fn open_descriptor(_: &Path) -> Result<File, Fault> {
    Err(Fault::Unsafe)
}

fn directory_stamp(path: &Path) -> Result<Stamp, Fault> {
    let metadata = checked_metadata(path)?;
    if !metadata.is_dir() {
        return Err(Fault::Unsafe);
    }
    Ok(Stamp {
        identity: None,
        bytes: metadata.len(),
        modified: metadata.modified()?,
    })
}

pub(super) struct Reader {
    home: PathBuf,
    pub(super) attempts_left: usize,
    pub(super) bytes_left: usize,
    observed: BTreeMap<PathBuf, Option<Stamp>>,
}

impl Reader {
    pub(super) fn new(home: PathBuf, attempts_left: usize, bytes_left: usize) -> Self {
        Self {
            home,
            attempts_left,
            bytes_left,
            observed: BTreeMap::new(),
        }
    }

    fn path(&self, relative: &Path) -> Result<PathBuf, Fault> {
        if relative.as_os_str().is_empty()
            || !relative.components().all(|part| {
                matches!(part, Component::Normal(name)
                if name.to_str().is_some_and(super::component))
            })
        {
            return Err(Fault::Unsafe);
        }
        Ok(self.home.join(relative))
    }

    pub(super) fn attempt(&mut self) -> Result<(), Fault> {
        self.attempts_left = self.attempts_left.checked_sub(1).ok_or(Fault::Limit)?;
        Ok(())
    }

    fn witness(&mut self, path: PathBuf, stamp: Option<Stamp>) -> Result<(), Fault> {
        if let Some(old) = self.observed.get(&path) {
            if old != &stamp {
                return Err(Fault::Changed);
            }
        } else {
            self.observed.insert(path, stamp);
        }
        Ok(())
    }

    pub(super) fn file(&mut self, relative: &Path, limit: usize) -> Result<Vec<u8>, Fault> {
        self.attempt()?;
        let path = self.path(relative)?;
        let (file, stamp) = match open(&path) {
            Ok(value) => value,
            Err(Fault::Missing) => {
                self.witness(path, None)?;
                return Err(Fault::Missing);
            }
            Err(fault) => return Err(fault),
        };
        let len = usize::try_from(stamp.bytes).map_err(|_| Fault::Limit)?;
        let allowed = len.checked_add(1).ok_or(Fault::Limit)?;
        if len > limit || allowed > self.bytes_left {
            return Err(Fault::Limit);
        }
        self.bytes_left -= allowed;
        let mut bytes = Vec::with_capacity(len);
        file.take(allowed as u64).read_to_end(&mut bytes)?;
        if bytes.len() != len || open(&path)?.1 != stamp {
            return Err(Fault::Changed);
        }
        self.witness(path, Some(stamp))?;
        Ok(bytes)
    }

    /// List digest-shaped object names without following links or retaining data.
    pub(super) fn objects(&mut self, relative: &Path) -> Result<Vec<String>, Fault> {
        self.attempt()?;
        let path = self.path(relative)?;
        let stamp = directory_stamp(&path)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&path)? {
            self.attempt()?;
            let entry = entry?;
            if let Some(name) = entry
                .file_name()
                .to_str()
                .filter(|name| super::digest_ref(name))
            {
                names.push(name.to_owned());
            }
        }
        self.witness(path, Some(stamp))?;
        names.sort_unstable();
        Ok(names)
    }

    pub(super) fn stable(&self) -> bool {
        self.observed.iter().all(|(path, expected)| match expected {
            Some(stamp) if stamp.identity.is_some() => {
                open(path).is_ok_and(|(_, actual)| &actual == stamp)
            }
            Some(stamp) => directory_stamp(path).is_ok_and(|actual| &actual == stamp),
            None => matches!(checked_metadata(path), Err(Fault::Missing)),
        })
    }
}
