//! Platform security for durable compact-home allocation roots and private slots.
//! These checks authenticate the final directory, not mutable ancestor paths.
//! Allocation/migration callers retain their exclusive owner gate and records.

use std::path::{Component, Path, PathBuf};

/// Discover ordered durable candidates without creating directories. Native
/// profile lookup deliberately does not use the provider's relocated HOME.
pub(super) fn root_candidates(wardian_home: &Path) -> Result<Vec<PathBuf>, String> {
    ordered_candidates(wardian_home, native::candidates())
}

fn ordered_candidates(
    wardian_home: &Path,
    fallbacks: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    checked_path(wardian_home)?;
    let mut candidates = vec![wardian_home.join("c")];
    candidates.extend(fallbacks);
    let mut unique = Vec::new();
    for candidate in candidates {
        // Optional profile/XDG discovery cannot veto a usable Wardian root or
        // another valid fallback (e.g. a container UID without passwd entry).
        if checked_path(&candidate).is_ok() && !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    Ok(unique)
}

/// Atomically create one private directory, or authenticate an existing one.
/// Also supports slots under an authenticated root. Never creates ancestors,
/// repairs permissions, replaces links, or allocates/migrates a Codex home.
pub(super) fn create_private_root(path: &Path) -> Result<(), String> {
    match create_private_directory(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_private_root(path)
        }
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Exclusively create a private root or slot. Existing paths always return
/// AlreadyExists, never successful adoption. A validation failure retains the
/// empty newly created directory for inspection; callers must not use it.
pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    checked_path(path)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let path: PathBuf = path.components().collect();
    native::create(&path)?;
    validate_private_root(&path).map_err(std::io::Error::other)
}

/// Authenticate a real directory owned by this user with private permissions.
/// An existing insecure directory is an error, even if it could be repaired.
pub(super) fn validate_private_root(path: &Path) -> Result<(), String> {
    checked_path(path)?;
    // Remove a trailing separator/dot before opening: on Unix it can otherwise
    // force traversal through a final symlink despite O_NOFOLLOW.
    let path: PathBuf = path.components().collect();
    native::validate(&path).map_err(|error| format!("{}: {error}", path.display()))
}

fn checked_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err("Private directory must have an absolute, non-root path without parent traversal or NUL".into());
    }
    Ok(())
}

#[cfg(unix)]
mod native {
    use super::*;
    use std::ffi::{CStr, OsString};
    use std::fs::OpenOptions;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    pub(super) fn candidates() -> Vec<PathBuf> {
        let profile = native_profile().ok();
        let mut roots = Vec::new();
        if let Some(profile) = &profile {
            roots.push(profile.join(".wc"));
        }
        if let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            roots.push(home.join(".wc"));
        }
        #[cfg(target_os = "macos")]
        // SAFETY: geteuid has no preconditions.
        roots.push(PathBuf::from(format!("/Users/Shared/.wc-{}", unsafe {
            libc::geteuid()
        })));
        #[cfg(target_os = "linux")]
        for (variable, fallback) in [
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_DATA_HOME", ".local/share"),
        ] {
            let base = std::env::var_os(variable)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| profile.as_ref().map(|profile| profile.join(fallback)));
            if let Some(base) = base {
                roots.push(base.join(".wc"));
            }
        }
        roots
    }

    fn native_profile() -> Result<PathBuf, String> {
        let mut capacity = 4096;
        loop {
            let mut buffer = vec![0u8; capacity];
            let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut result = std::ptr::null_mut();
            // SAFETY: all pointers reference live storage of the specified size.
            let status = unsafe {
                libc::getpwuid_r(
                    libc::geteuid(),
                    record.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    capacity,
                    &mut result,
                )
            };
            if status == libc::ERANGE && capacity < 1024 * 1024 {
                capacity *= 2;
                continue;
            }
            if status != 0 {
                return Err(format!(
                    "Native profile lookup failed: {}",
                    std::io::Error::from_raw_os_error(status)
                ));
            }
            if result.is_null() {
                return Err("Current user has no native profile record".into());
            }
            // SAFETY: successful getpwuid_r initialized the record and buffer.
            let directory = unsafe { record.assume_init().pw_dir };
            if directory.is_null() {
                return Err("Native profile record has no directory".into());
            }
            // SAFETY: pw_dir is a NUL-terminated string in the live buffer.
            let bytes = unsafe { CStr::from_ptr(directory) }.to_bytes().to_vec();
            let profile = PathBuf::from(OsString::from_vec(bytes));
            if !profile.is_absolute() {
                return Err("Native profile is not absolute".into());
            }
            return Ok(profile);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn create(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn create(path: &Path) -> std::io::Result<()> {
        use std::ffi::{c_void, CString};
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn filesec_init() -> *mut c_void;
            fn filesec_free(security: *mut c_void);
            fn filesec_set_property(
                security: *mut c_void,
                property: libc::c_int,
                value: *const c_void,
            ) -> libc::c_int;
            fn acl_init(entries: libc::c_int) -> *mut c_void;
            fn acl_free(acl: *mut c_void) -> libc::c_int;
            fn acl_get_flagset_np(acl: *mut c_void, flags: *mut *mut c_void) -> libc::c_int;
            fn acl_add_flag_np(flags: *mut c_void, flag: libc::c_uint) -> libc::c_int;
            fn mkdirx_np(path: *const libc::c_char, security: *mut c_void) -> libc::c_int;
        }
        let path = CString::new(path.as_os_str().as_bytes())?;
        // mkdirx_np installs mode and a no-inherit, empty ACL together. Plain
        // mkdir(0700) is insufficient when a macOS parent has inheritable ACEs.
        // SAFETY: initialized opaque objects live through mkdir; all allocations
        // are freed on every branch, after capturing errno from the failed API.
        unsafe {
            let security = filesec_init();
            if security.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let acl = acl_init(0);
            if acl.is_null() {
                let error = std::io::Error::last_os_error();
                filesec_free(security);
                return Err(error);
            }
            let mode: libc::mode_t = 0o700;
            let mut flags = std::ptr::null_mut();
            let success = acl_get_flagset_np(acl, &mut flags) == 0
                && acl_add_flag_np(flags, 1 << 17) == 0 // ACL_FLAG_NO_INHERIT
                && filesec_set_property(security, 4, std::ptr::addr_of!(mode).cast()) == 0 // FILESEC_MODE
                && filesec_set_property(security, 5, std::ptr::addr_of!(acl).cast()) == 0 // FILESEC_ACL
                && mkdirx_np(path.as_ptr(), security) == 0;
            let error = std::io::Error::last_os_error();
            acl_free(acl);
            filesec_free(security);
            if success {
                Ok(())
            } else {
                Err(error)
            }
        }
    }

    pub(super) fn validate(path: &Path) -> Result<(), String> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| error.to_string())?;
        let metadata = directory.metadata().map_err(|error| error.to_string())?;
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(
                "Private directory must be owned by the current user with mode 0700".into(),
            );
        }
        #[cfg(target_os = "macos")]
        validate_macos_acl(&directory)?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn validate_macos_acl(directory: &std::fs::File) -> Result<(), String> {
        use std::ffi::c_void;
        use std::os::fd::AsRawFd;
        extern "C" {
            fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut c_void;
            fn acl_valid(acl: *mut c_void) -> libc::c_int;
            fn acl_get_entry(
                acl: *mut c_void,
                entry: libc::c_int,
                out: *mut *mut c_void,
            ) -> libc::c_int;
            fn acl_free(acl: *mut c_void) -> libc::c_int;
        }
        // Darwin extended ACLs can grant access beyond mode bits. Require an
        // empty valid ACL; even deny-only entries are conservatively rejected.
        // SAFETY: the fd remains open; the returned ACL is freed exactly once.
        unsafe {
            let acl = acl_get_fd_np(directory.as_raw_fd(), 0x100);
            if acl.is_null() {
                return Err(format!(
                    "Cannot inspect private directory ACL: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let valid = acl_valid(acl) == 0;
            let mut entry = std::ptr::null_mut();
            let status = if valid {
                acl_get_entry(acl, 0, &mut entry)
            } else {
                0
            };
            let error = std::io::Error::last_os_error();
            acl_free(acl);
            // Darwin returns -1/EINVAL for the first entry of an empty ACL.
            if valid && status == -1 && error.raw_os_error() == Some(libc::EINVAL) {
                Ok(())
            } else {
                Err("Private directory has an extended ACL or its ACL cannot be validated".into())
            }
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::null_mut;
    use winapi::shared::minwindef::{BOOL, DWORD, HLOCAL, LPVOID};
    use winapi::um::fileapi::CreateDirectoryW;
    use winapi::um::minwinbase::SECURITY_ATTRIBUTES;
    use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
    use winapi::um::winnt::{
        TokenUser, ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_ACE_TYPE, ACE_HEADER, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, FILE_ALL_ACCESS, FILE_READ_ATTRIBUTES, HANDLE,
        OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PACL, PSECURITY_DESCRIPTOR, PSID,
        READ_CONTROL, SECURITY_DESCRIPTOR_CONTROL, SECURITY_INFORMATION, SE_DACL_PROTECTED,
        TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
    };

    // winbase.h constants; its winapi feature is not enabled directly.
    const FILE_FLAG_BACKUP_SEMANTICS: DWORD = 0x02000000;
    const FILE_FLAG_OPEN_REPARSE_POINT: DWORD = 0x00200000;

    // Existing winapi features expose the ABI types but not these security
    // modules. Match winapi signatures exactly; no custom ABI structs/features.
    #[link(name = "advapi32")]
    extern "system" {
        fn GetTokenInformation(
            token: HANDLE,
            class: TOKEN_INFORMATION_CLASS,
            info: LPVOID,
            size: DWORD,
            needed: *mut DWORD,
        ) -> BOOL;
        fn GetKernelObjectSecurity(
            handle: HANDLE,
            info: SECURITY_INFORMATION,
            descriptor: PSECURITY_DESCRIPTOR,
            size: DWORD,
            needed: *mut DWORD,
        ) -> BOOL;
        fn GetSecurityDescriptorControl(
            descriptor: PSECURITY_DESCRIPTOR,
            control: *mut SECURITY_DESCRIPTOR_CONTROL,
            revision: *mut DWORD,
        ) -> BOOL;
        fn GetSecurityDescriptorOwner(
            descriptor: PSECURITY_DESCRIPTOR,
            owner: *mut PSID,
            defaulted: *mut BOOL,
        ) -> BOOL;
        fn GetSecurityDescriptorDacl(
            descriptor: PSECURITY_DESCRIPTOR,
            present: *mut BOOL,
            acl: *mut PACL,
            defaulted: *mut BOOL,
        ) -> BOOL;
        fn GetAce(acl: PACL, index: DWORD, ace: *mut LPVOID) -> BOOL;
        fn EqualSid(left: PSID, right: PSID) -> BOOL;
        fn GetLengthSid(sid: PSID) -> DWORD;
        fn ConvertSidToStringSidW(sid: PSID, text: *mut *mut u16) -> BOOL;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: DWORD,
            descriptor: *mut PSECURITY_DESCRIPTOR,
            size: *mut DWORD,
        ) -> BOOL;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: HLOCAL) -> HLOCAL;
    }

    struct LocalAllocation(HLOCAL);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // SAFETY: these allocations come only from LocalAlloc-backed APIs.
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    struct CurrentUser(Vec<usize>);

    impl CurrentUser {
        fn load() -> Result<Self, String> {
            let mut token = null_mut();
            // SAFETY: live output pointer; process pseudo-handle is not closed.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            // SAFETY: OpenProcessToken returned a new owned handle.
            let token = unsafe { OwnedHandle::from_raw_handle(token.cast()) };
            security_buffer(|buffer, size, needed| {
                // SAFETY: security_buffer provides aligned storage and length.
                unsafe {
                    GetTokenInformation(
                        token.as_raw_handle().cast(),
                        TokenUser,
                        buffer,
                        size,
                        needed,
                    )
                }
            })
            .map(Self)
        }

        fn sid(&self) -> PSID {
            // SAFETY: successful TokenUser query populated aligned live storage.
            unsafe { (*(self.0.as_ptr().cast::<TOKEN_USER>())).User.Sid }
        }

        fn text(&self) -> Result<String, String> {
            let mut pointer = null_mut();
            // SAFETY: SID belongs to the live token-information buffer.
            if unsafe { ConvertSidToStringSidW(self.sid(), &mut pointer) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let _allocation = LocalAllocation(pointer.cast());
            // SAFETY: the API returns a NUL-terminated UTF-16 SID string.
            let text = unsafe { wide_os_string(pointer) };
            text.into_string()
                .map_err(|_| "Current user SID is not valid UTF-16".into())
        }
    }

    fn security_buffer(
        mut query: impl FnMut(LPVOID, DWORD, *mut DWORD) -> BOOL,
    ) -> Result<Vec<usize>, String> {
        let mut needed = 0;
        let status = query(null_mut(), 0, &mut needed);
        let error = std::io::Error::last_os_error();
        if status != 0 || error.raw_os_error() != Some(122) || needed == 0 || needed > 65536 {
            return Err(format!("Cannot size Windows security information: {error}"));
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if query(buffer.as_mut_ptr().cast(), needed, &mut needed) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(buffer)
    }

    unsafe fn wide_os_string(pointer: *const u16) -> OsString {
        let mut length = 0;
        // SAFETY: callers supply a live NUL-terminated Windows API string.
        unsafe {
            while *pointer.add(length) != 0 {
                length += 1;
            }
            OsString::from_wide(std::slice::from_raw_parts(pointer, length))
        }
    }

    fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, String> {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        // SAFETY: known-folder GUID and default current-user token are valid.
        unsafe {
            let pointer = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default())
                .map_err(|error| error.to_string())?;
            let path = PathBuf::from(wide_os_string(pointer.0));
            CoTaskMemFree(Some(pointer.0.cast()));
            Ok(path)
        }
    }

    pub(super) fn candidates() -> Vec<PathBuf> {
        use windows::Win32::UI::Shell::{FOLDERID_Profile, FOLDERID_ProgramData};
        let mut roots = Vec::new();
        if let Ok(profile) = known_folder(&FOLDERID_Profile) {
            roots.push(profile.join(".wc"));
        }
        if let (Ok(user), Ok(program_data)) =
            (CurrentUser::load(), known_folder(&FOLDERID_ProgramData))
        {
            // SAFETY: token-information owns the valid SID throughout hashing.
            let sid = unsafe {
                std::slice::from_raw_parts(
                    user.sid().cast::<u8>(),
                    GetLengthSid(user.sid()) as usize,
                )
            };
            let hash = format!("{:x}", Sha256::digest(sid));
            roots.push(program_data.join(format!(".wc-{}", &hash[..12])));
        }
        roots
    }

    pub(super) fn create(path: &Path) -> std::io::Result<()> {
        let user = CurrentUser::load().map_err(std::io::Error::other)?;
        let sid = user.text().map_err(std::io::Error::other)?;
        let descriptor = format!("O:{sid}D:P(A;OICI;FA;;;{sid})");
        create_with_descriptor(path, &descriptor)
    }

    pub(super) fn create_with_descriptor(path: &Path, descriptor: &str) -> std::io::Result<()> {
        let sddl: Vec<u16> = descriptor.encode_utf16().chain(Some(0)).collect();
        // std canonicalization supplies the verbatim spelling needed by raw
        // Win32 calls beyond MAX_PATH. Never canonicalize the final component:
        // it must remain an exclusive mkdir / no-follow validation boundary.
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Private directory has no parent",
            )
        })?;
        let name = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Private directory has no final component",
            )
        })?;
        let path = std::fs::canonicalize(parent)?.join(name);
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = null_mut();
        // SAFETY: both strings are NUL-terminated; output stays live through mkdir.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let _allocation = LocalAllocation(descriptor.cast());
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as DWORD,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            if CreateDirectoryW(path.as_ptr(), &mut attributes) != 0 {
                return Ok(());
            }
            Err(std::io::Error::last_os_error())
        }
    }

    pub(super) fn validate(path: &Path) -> Result<(), String> {
        let user = CurrentUser::load()?;
        let directory = OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .map_err(|error| error.to_string())?;
        let metadata = directory.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir() || crate::utils::fs::is_directory_link(&metadata) {
            return Err("Private directory must be a real directory, not a reparse point".into());
        }
        let mut buffer = security_buffer(|buffer, size, needed| {
            // SAFETY: directory remains open and buffer is aligned live storage.
            unsafe {
                GetKernelObjectSecurity(
                    directory.as_raw_handle().cast(),
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    buffer,
                    size,
                    needed,
                )
            }
        })?;
        let descriptor = buffer.as_mut_ptr().cast();
        let mut control = 0;
        let mut revision = 0;
        let mut owner = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut acl: PACL = null_mut();
        let mut ace = null_mut();
        // SAFETY: queried descriptor is valid and all returned pointers borrow
        // its live aligned buffer. GetAce checks bounds before dereferencing.
        unsafe {
            if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
                || GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
                || owner.is_null()
                || EqualSid(owner, user.sid()) == 0
                || control & SE_DACL_PROTECTED == 0
                || GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
                    == 0
                || present == 0
                || acl.is_null()
                || (*acl).AceCount != 1
                || GetAce(acl, 0, &mut ace) == 0
            {
                return Err("Private directory requires current-user ownership and a protected current-user-only DACL".into());
            }
            let header = &*ace.cast::<ACE_HEADER>();
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE
                || header.AceSize as DWORD != 8 + GetLengthSid(user.sid())
            {
                return Err("Private directory has an unexpected access-control entry".into());
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            if ace.Header.AceFlags != (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
                || ace.Mask != FILE_ALL_ACCESS
                || EqualSid(
                    std::ptr::addr_of!(ace.SidStart).cast_mut().cast(),
                    user.sid(),
                ) == 0
            {
                return Err("Private directory DACL must grant inheritable full control only to the current user".into());
            }
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
mod native {
    use super::*;
    pub(super) fn candidates() -> Vec<PathBuf> {
        Vec::new()
    }
    pub(super) fn create(_: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "Private compact homes are unsupported on this platform",
        ))
    }
    pub(super) fn validate(_: &Path) -> Result<(), String> {
        Err("Private compact homes are unsupported on this platform".into())
    }
}

#[cfg(test)]
#[path = "platform_tests.rs"]
mod tests;
