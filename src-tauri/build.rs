fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    #[cfg(windows)]
    if let Some(out_dir) = std::env::var_os("OUT_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            out_dir.to_string_lossy()
        );
    }
    tauri_build::build()
}
