// Prevents an additional console window when Wardian is launched on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    wardian_app_lib::run()
}
