// Prevents an additional console window when Wardian is launched on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if let Some(exit_code) = wardian_app_lib::utils::process::run_silent_cmd_shim_if_requested() {
        std::process::exit(exit_code);
    }

    wardian_app_lib::run()
}
