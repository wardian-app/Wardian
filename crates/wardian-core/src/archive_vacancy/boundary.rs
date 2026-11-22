//! Handle-relative, read-only traversal. Every component is opened without links.
//!
//! Retaining directory handles avoids the check-then-open symlink/junction race.
//! Read-only child opens use openat/O_NOFOLLOW on supported Linux/macOS targets and
//! NtCreateFile/FILE_OPEN_REPARSE_POINT on Windows. Directory enumeration also
//! uses handles, never an ambient pathname. File owns and closes returned handles.
//!
//! Unix constants, symbols and directory layouts come from libc's target bindings.
//! Unsupported ABIs fail closed. Parent archive locks still own cohort consistency;
//! these handles do not claim exclusive access against external filesystem writers.
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    path::{Component, Path},
};

pub(super) struct Root(File);

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "links_forbidden")
}

fn regular(file: File, directory: bool) -> io::Result<File> {
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(invalid());
    }
    Ok(file)
}

impl Root {
    pub(super) fn new(path: &Path) -> io::Result<Self> {
        Ok(Self(platform::root(&std::path::absolute(path)?)?))
    }

    fn open(&self, relative: &str, directory: bool) -> io::Result<File> {
        let mut handle = self.0.try_clone()?;
        if relative.is_empty() && directory {
            return Ok(handle);
        }
        if relative.is_empty() || relative.contains(['\\', ':']) {
            return Err(invalid());
        }
        let components: Vec<_> = Path::new(relative).components().collect();
        if components.is_empty()
            || components
                .iter()
                .any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(invalid());
        }
        for (index, part) in components.iter().enumerate() {
            handle = platform::child(
                &handle,
                part.as_os_str(),
                index + 1 < components.len() || directory,
            )?;
        }
        Ok(handle)
    }

    pub(super) fn list(&self, relative: &str) -> io::Result<Vec<String>> {
        platform::list(&self.open(relative, true)?)
    }

    pub(super) fn read(&self, relative: &str, limit: Option<usize>) -> io::Result<Vec<u8>> {
        let mut file = self.open(relative, false)?;
        let before = file.metadata()?;
        if limit.is_some_and(|limit| before.len() > limit as u64) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file_limit"));
        }
        let mut bytes = Vec::new();
        match limit {
            Some(limit) => {
                file.by_ref()
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)?;
            }
            None => {
                file.read_to_end(&mut bytes)?;
            }
        }
        let after = file.metadata()?;
        if before.len() != after.len()
            || before.modified()? != after.modified()?
            || bytes.len() as u64 != after.len()
            || limit.is_some_and(|limit| bytes.len() > limit)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot_changed",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn hash(&self, relative: &str) -> io::Result<(u64, [u8; 32])> {
        let mut file = self.open(relative, false)?;
        let before = file.metadata()?;
        let mut hash = Sha256::new();
        let mut bytes = [0_u8; 64 * 1024];
        let mut length = 0_u64;
        loop {
            let count = file.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
            length = length
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("file_limit"))?;
        }
        let after = file.metadata()?;
        if length != before.len()
            || length != after.len()
            || before.modified()? != after.modified()?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot_changed",
            ));
        }
        Ok((length, hash.finalize().into()))
    }
}

#[cfg(any(
    all(target_os = "linux", target_pointer_width = "64"),
    target_os = "macos"
))]
mod platform {
    use super::*;
    use std::{
        ffi::{CStr, CString, OsStr},
        os::{
            fd::{AsRawFd, FromRawFd, IntoRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        },
    };

    // O_NONBLOCK prevents a non-regular leaf from blocking before metadata rejects it.
    const FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;

    pub(super) fn child(parent: &File, name: &OsStr, directory: bool) -> io::Result<File> {
        let name = CString::new(name.as_bytes()).map_err(|_| invalid())?;
        // Read-only openat is rooted at an already verified directory handle.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                FLAGS | if directory { libc::O_DIRECTORY } else { 0 },
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // Ownership is transferred exactly once from openat.
        regular(unsafe { File::from_raw_fd(fd) }, directory)
    }

    pub(super) fn root(path: &Path) -> io::Result<File> {
        let mut current = regular(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(FLAGS | libc::O_DIRECTORY)
                .open("/")?,
            true,
        )?;
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => current = child(&current, name, true)?,
                _ => return Err(invalid()),
            }
        }
        Ok(current)
    }

    unsafe fn errno() -> *mut libc::c_int {
        #[cfg(target_os = "linux")]
        {
            unsafe { libc::__errno_location() }
        }
        #[cfg(target_os = "macos")]
        {
            unsafe { libc::__error() }
        }
    }

    pub(super) fn list(handle: &File) -> io::Result<Vec<String>> {
        let fd = handle.try_clone()?.into_raw_fd();
        // fdopendir owns fd on success; close it ourselves only on failure.
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            drop(unsafe { File::from_raw_fd(fd) });
            return Err(error);
        }
        struct Close(*mut libc::DIR);
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let close = Close(dir);
        unsafe {
            libc::rewinddir(close.0);
        }
        let mut names = Vec::new();
        loop {
            unsafe {
                *errno() = 0;
            }
            let entry = unsafe { libc::readdir(close.0) };
            if entry.is_null() {
                if unsafe { *errno() } != 0 {
                    return Err(io::Error::last_os_error());
                }
                break;
            }
            // SAFETY: readdir supplies a live NUL-terminated d_name, copied before
            // the next call. libc pairs the target dirent with its native symbol,
            // including the $INODE64 ABI on Intel macOS.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .map_err(|_| invalid())?;
            if name != "." && name != ".." {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{c_void, OsStr, OsString},
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{AsRawHandle, FromRawHandle},
        },
        path::Prefix,
    };
    type Handle = *mut c_void;

    #[repr(C)]
    struct Unicode {
        length: u16,
        maximum: u16,
        buffer: *mut u16,
    }
    #[repr(C)]
    struct Attributes {
        length: u32,
        root: Handle,
        name: *mut Unicode,
        flags: u32,
        security: Handle,
        quality: Handle,
    }
    #[repr(C)]
    struct Status {
        status: usize,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            handle: *mut Handle,
            access: u32,
            attributes: *mut Attributes,
            status: *mut Status,
            allocation: *const i64,
            attributes_flags: u32,
            share: u32,
            disposition: u32,
            options: u32,
            ea: Handle,
            ea_length: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(handle: Handle, class: i32, data: Handle, size: u32)
            -> i32;
    }

    fn native(parent: Handle, name: &OsStr, directory: bool) -> io::Result<File> {
        let mut wide: Vec<u16> = name.encode_wide().collect();
        let length =
            u16::try_from(wide.len().checked_mul(2).ok_or_else(invalid)?).map_err(|_| invalid())?;
        let mut name = Unicode {
            length,
            maximum: length,
            buffer: wide.as_mut_ptr(),
        };
        let mut attributes = Attributes {
            length: std::mem::size_of::<Attributes>() as u32,
            root: parent,
            name: &mut name,
            flags: 0x40,
            security: std::ptr::null_mut(),
            quality: std::ptr::null_mut(),
        };
        let mut status = Status {
            status: 0,
            information: 0,
        };
        let mut handle = std::ptr::null_mut();
        // FILE_OPEN_REPARSE_POINT prevents following both symlinks and junctions.
        // Synchronous read-only opens permit std::fs::File reads and handle listing.
        let code = unsafe {
            NtCreateFile(
                &mut handle,
                0x100081,
                &mut attributes,
                &mut status,
                std::ptr::null(),
                0,
                7,
                1,
                0x200000 | 0x20 | if directory { 1 } else { 0x40 },
                std::ptr::null_mut(),
                0,
            )
        };
        if code < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(code) } as i32,
            ));
        }
        regular(unsafe { File::from_raw_handle(handle) }, directory)
    }

    pub(super) fn child(parent: &File, name: &OsStr, directory: bool) -> io::Result<File> {
        native(parent.as_raw_handle(), name, directory)
    }

    pub(super) fn root(path: &Path) -> io::Result<File> {
        let mut parts = path.components();
        let Some(Component::Prefix(prefix)) = parts.next() else {
            return Err(invalid());
        };
        let start = match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                format!("\\??\\{}:\\", char::from(drive))
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                let mut name = OsString::from("\\??\\UNC\\");
                name.push(server);
                name.push("\\");
                name.push(share);
                name.push("\\");
                return walk(native(std::ptr::null_mut(), &name, true)?, parts);
            }
            _ => return Err(invalid()),
        };
        walk(
            native(std::ptr::null_mut(), OsStr::new(&start), true)?,
            parts,
        )
    }

    fn walk(mut current: File, parts: std::path::Components<'_>) -> io::Result<File> {
        for part in parts {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => current = child(&current, name, true)?,
                _ => return Err(invalid()),
            }
        }
        Ok(current)
    }

    pub(super) fn list(handle: &File) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        let mut buffer = vec![0_u64; 8192]; // Aligned 64 KiB native directory buffer.
        let mut restart = true;
        loop {
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    handle.as_raw_handle(),
                    if restart { 15 } else { 14 },
                    buffer.as_mut_ptr().cast(),
                    65536,
                )
            };
            restart = false;
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(18) {
                    break;
                } // ERROR_NO_MORE_FILES
                return Err(error);
            }
            let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), 65536) };
            let mut offset = 0;
            loop {
                if offset + 68 > bytes.len() {
                    return Err(invalid());
                }
                let word = |at: usize| {
                    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("checked native field"))
                };
                let next = word(offset) as usize;
                let length = word(offset + 60) as usize;
                if !length.is_multiple_of(2) || offset + 68 + length > bytes.len() {
                    return Err(invalid());
                }
                let wide: Vec<_> = bytes[offset + 68..offset + 68 + length]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                let name = OsString::from_wide(&wide)
                    .into_string()
                    .map_err(|_| invalid())?;
                if name != "." && name != ".." {
                    names.push(name);
                }
                if next == 0 {
                    break;
                }
                if next < 68 || next > bytes.len() - offset {
                    return Err(invalid());
                }
                offset += next;
            }
        }
        names.sort();
        Ok(names)
    }
}

#[cfg(not(any(
    windows,
    all(target_os = "linux", target_pointer_width = "64"),
    target_os = "macos"
)))]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "platform_unsupported")
    }
    pub(super) fn root(_: &Path) -> io::Result<File> {
        Err(unsupported())
    }
    pub(super) fn child(_: &File, _: &OsStr, _: bool) -> io::Result<File> {
        Err(unsupported())
    }
    pub(super) fn list(_: &File) -> io::Result<Vec<String>> {
        Err(unsupported())
    }
}
