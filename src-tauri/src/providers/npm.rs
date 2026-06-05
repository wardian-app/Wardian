#[cfg(target_os = "windows")]
/// Resolve an npm-generated `.cmd` shim to a direct `node <script.js>` launch.
///
/// Provider adapters should use this before returning a Windows npm CLI shim.
/// Launching the JavaScript entrypoint directly keeps headless provider argv
/// stable and avoids shell quoting/windowing behavior from `cmd.exe` shims.
pub fn node_launch_from_npm_cmd_shim(
    base_dir: &std::path::Path,
    command_name: &str,
) -> Option<(String, Vec<String>)> {
    let shim = base_dir.join(format!("{command_name}.cmd"));
    let content = std::fs::read_to_string(shim).ok()?;
    let script = npm_cmd_shim_script_path(&content)?;
    let script = base_dir.join(script);
    if !script.is_file() {
        return None;
    }

    let local_node = base_dir.join("node.exe");
    let executable = if local_node.is_file() {
        local_node.to_string_lossy().to_string()
    } else {
        "node".to_string()
    };
    Some((executable, vec![script.to_string_lossy().to_string()]))
}

#[cfg(not(target_os = "windows"))]
/// Non-Windows providers do not use npm `.cmd` shims.
pub fn node_launch_from_npm_cmd_shim(
    _base_dir: &std::path::Path,
    _command_name: &str,
) -> Option<(String, Vec<String>)> {
    None
}

#[cfg(target_os = "windows")]
fn npm_cmd_shim_script_path(content: &str) -> Option<std::path::PathBuf> {
    for quoted in content.split('"').skip(1).step_by(2) {
        let normalized = quoted.replace('/', "\\");
        let lower = normalized.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("%dp0%\\") else {
            continue;
        };
        if !rest.ends_with(".js") {
            continue;
        }
        let original_rest = &normalized["%dp0%\\".len()..];
        return Some(std::path::PathBuf::from(original_rest));
    }
    None
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn parses_node_script_from_npm_cmd_shim() {
        let content = r#"@ECHO off
"%dp0%\node.exe" "%dp0%\node_modules\package\bin\tool.js" %*
"#;

        assert_eq!(
            npm_cmd_shim_script_path(content).unwrap(),
            std::path::PathBuf::from("node_modules\\package\\bin\\tool.js")
        );
    }
}
