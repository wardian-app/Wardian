//! Checkpoint replacement while polling clients retain a read handle.
//! The workflow driver remains the writer; readers may observe either complete
//! snapshot. No retry, timeout extension, or in-place truncation is required.

#[cfg(not(windows))]
pub(super) use crate::atomic_file::write_json_atomic as write;

#[cfg(windows)]
pub(super) fn write<T: serde::Serialize + ?Sized>(
    path: &std::path::Path,
    state: &T,
) -> std::io::Result<()> {
    use crate::atomic_file::{replace_staged_atomic_durable, stage_bytes_atomic, wide_path_null};
    use std::io;

    let mut bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    bytes.push(b'\n');
    // Reuse unique staging and sync the complete new snapshot before replacing.
    let staged = stage_bytes_atomic(path, &bytes)?;
    let destination = wide_path_null(path);
    let replacement = wide_path_null(&staged);
    // MoveFileExW fails with ERROR_ACCESS_DENIED when the existing checkpoint
    // is open even by a reader sharing deletion. ReplaceFileW permits that
    // reader to finish reading its old snapshot without blocking this writer.
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // ReplaceFileW requires an existing destination. Only initial creation
    // uses the ordinary atomic move; sharing/access errors must still fail.
    if error.raw_os_error() == Some(2) && !path.try_exists()? {
        return replace_staged_atomic_durable(&staged, path);
    }
    Err(error)
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn ReplaceFileW(
        replaced_file_name: *const u16,
        replacement_file_name: *const u16,
        backup_file_name: *const u16,
        flags: u32,
        exclude: *mut std::ffi::c_void,
        reserved: *mut std::ffi::c_void,
    ) -> i32;
}
