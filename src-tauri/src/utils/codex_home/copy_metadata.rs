//! Fail closed on state the bounded cross-volume copier cannot preserve.
//! Same-volume rename retains this metadata and does not call this preflight.

#[cfg(unix)]
pub(super) fn validate(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // SAFETY: valid NUL-terminated path; null output with zero size queries only.
    // Both calls inspect the link itself rather than following its referent.
    #[cfg(target_os = "macos")]
    let count =
        unsafe { libc::listxattr(path.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    #[cfg(not(target_os = "macos"))]
    let count = unsafe { libc::llistxattr(path.as_ptr(), std::ptr::null_mut(), 0) };
    if count != 0 {
        return Err("Cross-volume Codex copy cannot preserve/inspect extended attributes; use a same-volume private root; original retained".into());
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn validate(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::fileapi::{
        FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
    };
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    // Opaque links are reproduced as links; never query streams on their target.
    if super::storage::is_link(path)? {
        return Ok(());
    }
    #[repr(C)]
    struct StreamData {
        size: i64,
        name: [u16; 296], // WIN32_FIND_STREAM_DATA: MAX_PATH + 36 WCHARs.
    }
    let canonical = std::fs::canonicalize(path).map_err(super::storage::error)?;
    let name: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data = StreamData {
        size: 0,
        name: [0; 296],
    };
    // SAFETY: terminated path and correctly sized/aligned WIN32_FIND_STREAM_DATA.
    let handle = unsafe {
        FindFirstStreamW(
            name.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut StreamData).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return if std::io::Error::last_os_error().raw_os_error() == Some(38) {
            Ok(()) // ERROR_HANDLE_EOF: no data streams (e.g. an empty directory).
        } else {
            Err(
                "Cannot inspect Codex data streams; use a same-volume root; original retained"
                    .into(),
            )
        };
    }
    let result = loop {
        let length = data
            .name
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(data.name.len());
        if data.name[..length] != [58, 58, 36, 68, 65, 84, 65] {
            // ::$DATA
            break Err("Cross-volume Codex copy cannot preserve named data streams; use a same-volume private root; original retained".into());
        }
        // SAFETY: live enumeration handle and the same initialized output buffer.
        if unsafe { FindNextStreamW(handle, (&mut data as *mut StreamData).cast()) } == 0 {
            break if std::io::Error::last_os_error().raw_os_error() == Some(38) {
                Ok(())
            } else {
                Err("Codex data-stream inspection failed; original retained".into())
            };
        }
    };
    // SAFETY: handle came from FindFirstStreamW and is closed exactly once.
    unsafe { FindClose(handle) };
    result
}
