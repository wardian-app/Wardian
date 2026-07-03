use super::WinChild;
use crate::cmdbuilder::CommandBuilder;
use crate::win::procthreadattr::ProcThreadAttributeList;
use anyhow::{bail, ensure, Error};
use filedescriptor::{FileDescriptor, OwnedHandle};
use lazy_static::lazy_static;
use shared_library::shared_library;
use std::ffi::OsString;
use std::io::Error as IoError;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::{mem, ptr};
use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::{HRESULT, S_OK};
use winapi::um::handleapi::*;
use winapi::um::processthreadsapi::*;
use winapi::um::winbase::{
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, STARTF_USESHOWWINDOW,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;
use winapi::um::winuser::SW_HIDE;

pub type HPCON = HANDLE;

pub const PSUEDOCONSOLE_INHERIT_CURSOR: DWORD = 0x1;
pub const PSEUDOCONSOLE_RESIZE_QUIRK: DWORD = 0x2;
pub const PSEUDOCONSOLE_WIN32_INPUT_MODE: DWORD = 0x4;
#[allow(dead_code)]
pub const PSEUDOCONSOLE_PASSTHROUGH_MODE: DWORD = 0x8;

fn conpty_child_process_creation_flags() -> DWORD {
    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT
}

shared_library!(ConPtyFuncs,
    pub fn CreatePseudoConsole(
        size: COORD,
        hInput: HANDLE,
        hOutput: HANDLE,
        flags: DWORD,
        hpc: *mut HPCON
    ) -> HRESULT,
    pub fn ResizePseudoConsole(hpc: HPCON, size: COORD) -> HRESULT,
    pub fn ClosePseudoConsole(hpc: HPCON),
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConPtyLoadSource {
    Bundled,
    Bare,
    Kernel32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConPtyLoadDiagnostics {
    pub load_source: ConPtyLoadSource,
    pub bundled_conpty_path: Option<PathBuf>,
    pub bundled_conpty_exists: bool,
    pub bundled_openconsole_path: Option<PathBuf>,
    pub bundled_openconsole_exists: bool,
    pub bundled_load_error: Option<String>,
    pub bare_load_error: Option<String>,
}

struct ConPtyLoad {
    funcs: ConPtyFuncs,
    diagnostics: ConPtyLoadDiagnostics,
}

fn bundled_conpty_path_from_exe_dir(exe_dir: &Path) -> PathBuf {
    exe_dir.join("conpty").join("x64").join("conpty.dll")
}

fn bundled_openconsole_path_from_exe_dir(exe_dir: &Path) -> PathBuf {
    exe_dir
        .join("conpty")
        .join("x64")
        .join("OpenConsole.exe")
}

fn load_conpty() -> ConPtyLoad {
    // If the kernel doesn't export these functions then their system is
    // too old and we cannot run.
    let kernel = ConPtyFuncs::open(Path::new("kernel32.dll")).expect(
        "this system does not support conpty.  Windows 10 October 2018 or newer is required",
    );
    let mut diagnostics = ConPtyLoadDiagnostics {
        load_source: ConPtyLoadSource::Kernel32,
        bundled_conpty_path: None,
        bundled_conpty_exists: false,
        bundled_openconsole_path: None,
        bundled_openconsole_exists: false,
        bundled_load_error: None,
        bare_load_error: None,
    };

    // We prefer to use a sideloaded conpty.dll and openconsole.exe host deployed
    // alongside the application.  We check for this after checking for kernel
    // support so that we don't try to proceed and do something crazy.
    //
    // Wardian patch: the in-box kernel32 ConPTY flattens an inline-viewport
    // TUI's scroll-region history (e.g. codex) into in-place repaints and loses
    // terminal scrollback. Wardian ships the modern Microsoft ConPTY
    // redistributable in a `conpty/x64` folder next to the executable; prefer
    // it (loaded by absolute path so `conpty.dll` finds its co-located
    // `OpenConsole.exe`). Falls back to a bare sideloaded `conpty.dll`, then to
    // kernel32, so absence of the bundle preserves the previous behaviour.
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
    {
        let bundled = bundled_conpty_path_from_exe_dir(&exe_dir);
        let openconsole = bundled_openconsole_path_from_exe_dir(&exe_dir);
        diagnostics.bundled_conpty_exists = bundled.exists();
        diagnostics.bundled_openconsole_exists = openconsole.exists();
        diagnostics.bundled_conpty_path = Some(bundled.clone());
        diagnostics.bundled_openconsole_path = Some(openconsole);
        if bundled.exists() {
            match ConPtyFuncs::open(&bundled) {
                Ok(sideloaded) => {
                    diagnostics.load_source = ConPtyLoadSource::Bundled;
                    return ConPtyLoad {
                        funcs: sideloaded,
                        diagnostics,
                    };
                }
                Err(error) => {
                    diagnostics.bundled_load_error = Some(format!("{:?}", error));
                }
            }
        }
    }

    match ConPtyFuncs::open(Path::new("conpty.dll")) {
        Ok(sideloaded) => {
            diagnostics.load_source = ConPtyLoadSource::Bare;
            ConPtyLoad {
                funcs: sideloaded,
                diagnostics,
            }
        }
        Err(error) => {
            diagnostics.bare_load_error = Some(format!("{:?}", error));
            ConPtyLoad {
                funcs: kernel,
                diagnostics,
            }
        }
    }
}

lazy_static! {
    static ref CONPTY: ConPtyLoad = load_conpty();
}

pub fn conpty_load_diagnostics() -> ConPtyLoadDiagnostics {
    CONPTY.diagnostics.clone()
}

pub struct PsuedoCon {
    con: HPCON,
}

unsafe impl Send for PsuedoCon {}
unsafe impl Sync for PsuedoCon {}

impl Drop for PsuedoCon {
    fn drop(&mut self) {
        unsafe { (CONPTY.funcs.ClosePseudoConsole)(self.con) };
    }
}

impl PsuedoCon {
    pub fn new(size: COORD, input: FileDescriptor, output: FileDescriptor) -> Result<Self, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let result = unsafe {
            (CONPTY.funcs.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                PSUEDOCONSOLE_INHERIT_CURSOR
                    | PSEUDOCONSOLE_RESIZE_QUIRK
                    | PSEUDOCONSOLE_WIN32_INPUT_MODE,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console: HRESULT {}",
            result
        );
        Ok(Self { con })
    }

    pub fn resize(&self, size: COORD) -> Result<(), Error> {
        let result = unsafe { (CONPTY.funcs.ResizePseudoConsole)(self.con, size) };
        ensure!(
            result == S_OK,
            "failed to resize console to {}x{}: HRESULT: {}",
            size.X,
            size.Y,
            result
        );
        Ok(())
    }

    pub fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<WinChild> {
        let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
        si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        // Explicitly set the stdio handles as invalid handles otherwise
        // we can end up with a weird state where the spawned process can
        // inherit the explicitly redirected output handles from its parent.
        // For example, when daemonizing wezterm-mux-server, the stdio handles
        // are redirected to a log file and the spawned process would end up
        // writing its output there instead of to the pty we just created.
        // Wardian patch: keep provider PTY children attached to ConPTY while
        // asking Windows not to surface a separate console window.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
        si.StartupInfo.wShowWindow = SW_HIDE as u16;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

        let mut attrs = ProcThreadAttributeList::with_capacity(1)?;
        attrs.set_pty(self.con)?;
        si.lpAttributeList = attrs.as_mut_ptr();

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        let (mut exe, mut cmdline) = cmd.cmdline()?;
        let cmd_os = OsString::from_wide(&cmdline);

        let cwd = cmd.current_directory();

        let res = unsafe {
            CreateProcessW(
                exe.as_mut_slice().as_mut_ptr(),
                cmdline.as_mut_slice().as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                conpty_child_process_creation_flags(),
                cmd.environment_block().as_mut_slice().as_mut_ptr() as *mut _,
                cwd.as_ref()
                    .map(|c| c.as_slice().as_ptr())
                    .unwrap_or(ptr::null()),
                &mut si.StartupInfo,
                &mut pi,
            )
        };
        if res == 0 {
            let err = IoError::last_os_error();
            let msg = format!(
                "CreateProcessW `{:?}` in cwd `{:?}` failed: {}",
                cmd_os,
                cwd.as_ref().map(|c| OsString::from_wide(c)),
                err
            );
            log::error!("{}", msg);
            bail!("{}", msg);
        }

        // Make sure we close out the thread handle so we don't leak it;
        // we do this simply by making it owned
        let _main_thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };
        let proc = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };

        Ok(WinChild {
            proc: Mutex::new(proc),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn conpty_child_process_creation_flags_preserve_extended_startup_info() {
        assert_ne!(
            super::conpty_child_process_creation_flags() & super::EXTENDED_STARTUPINFO_PRESENT,
            0
        );
    }

    #[test]
    fn conpty_child_process_creation_flags_do_not_create_no_window() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        assert_eq!(
            super::conpty_child_process_creation_flags() & CREATE_NO_WINDOW,
            0
        );
    }

    #[test]
    fn bundled_conpty_path_is_relative_to_executable_directory() {
        let path = super::bundled_conpty_path_from_exe_dir(Path::new(r"C:\Program Files\Wardian"));

        assert_eq!(
            path,
            Path::new(r"C:\Program Files\Wardian")
                .join("conpty")
                .join("x64")
                .join("conpty.dll")
        );
    }

    #[test]
    fn bundled_openconsole_path_sits_next_to_bundled_conpty() {
        let path =
            super::bundled_openconsole_path_from_exe_dir(Path::new(r"C:\Program Files\Wardian"));

        assert_eq!(
            path,
            Path::new(r"C:\Program Files\Wardian")
                .join("conpty")
                .join("x64")
                .join("OpenConsole.exe")
        );
    }
}
