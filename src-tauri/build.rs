fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    #[cfg(windows)]
    if let Some(out_dir) = std::env::var_os("OUT_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            out_dir.to_string_lossy()
        );
    }
    #[cfg(windows)]
    copy_bundled_conpty();
    tauri_build::build()
}

/// Place the bundled modern ConPTY (`conpty/x64/{conpty.dll,OpenConsole.exe}`)
/// next to the built executable so `load_conpty()` finds it for dev and
/// direct-run (`target/debug`, `target/release`) builds. The installer gets
/// these via `tauri.conf.json` `bundle.resources`. Best-effort: any failure is
/// ignored so it can never break the build (the runtime falls back to the
/// in-box kernel32 ConPTY when the bundle is absent).
#[cfg(windows)]
fn copy_bundled_conpty() {
    use std::path::PathBuf;

    let manifest_dir = match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => return,
    };
    let src_dir = manifest_dir.join("conpty").join("x64");
    println!("cargo:rerun-if-changed={}", src_dir.display());
    if !src_dir.exists() {
        return;
    }

    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  ->  target/<profile>
    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => return,
    };
    let profile_dir = match out_dir.ancestors().nth(3) {
        Some(dir) => dir.to_path_buf(),
        None => return,
    };
    let dst_dir = profile_dir.join("conpty").join("x64");
    if std::fs::create_dir_all(&dst_dir).is_err() {
        return;
    }
    for name in ["conpty.dll", "OpenConsole.exe"] {
        let _ = std::fs::copy(src_dir.join(name), dst_dir.join(name));
    }
}
