//! One read-only plan for every materialized Pi Resume, independent of handover.
use super::{history::SessionFileBinding, PiProvider};
use std::path::{Path, PathBuf};
use wardian_core::models::{AgentConfig, AgentProvider};

const SDK_VERSION: &str = "0.84.2";
const LAUNCHER: &str = include_str!("sdk-resume.mjs");
const MAX_GENERATION: u64 = 9_007_199_254_740_991;

/// Preparation never creates files, invokes Pi, loads extensions, or opens its
/// SDK session API. Publication/stop/bridge authority stays with the manager.
pub(crate) struct PiExactResumePlan {
    executable: String,
    cli_prefix: Vec<String>,
    package_root: PathBuf,
    directory: PathBuf,
    native_id: String,
    destination: PathBuf,
    history: SessionFileBinding,
}

impl PiExactResumePlan {
    /// Called by adapter configuration preflight before shared shutdown and by
    /// ordinary spawn for Off -> Resume and application startup restoration.
    pub(crate) fn prepare(config: &AgentConfig) -> Result<Self, String> {
        if config
            .custom_args
            .as_deref()
            .is_some_and(|args| shlex::split(args).is_none())
        {
            return Err("Pi exact resume custom arguments have invalid quoting".into());
        }
        let native_id = config
            .resume_session
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or("Pi exact SDK resume requires a materialized resume identity")?;
        let directory = PiProvider::session_dir(&config.session_id)
            .ok_or("Pi owned session directory is unavailable")?;
        let destination = Path::new(&config.folder);
        let (executable, cli_prefix) = PiProvider::new().get_executable();
        let args = PiProvider::new().spawn_args(config, true, false);
        Self::prepare_from(
            &executable,
            cli_prefix,
            directory,
            native_id,
            destination,
            args,
        )
    }

    fn prepare_from(
        executable: &str,
        cli_prefix: Vec<String>,
        directory: PathBuf,
        native_id: &str,
        destination: &Path,
        mut args: Vec<String>,
    ) -> Result<Self, String> {
        if !destination.is_absolute() || !destination.is_dir() {
            return Err(
                "Pi exact SDK resume requires an existing absolute destination directory".into(),
            );
        }
        let (executable, cli_prefix, package_root) = resolve_sdk(executable, cli_prefix)?;
        let history = super::history::inspect_owned_history(&directory, native_id)?
            .complete
            .ok_or("Pi exact SDK resume requires unique complete owned history")?;
        history.sdk_open_receipt(native_id)?;
        let selector = args
            .windows(2)
            .position(|pair| pair == ["--session", native_id])
            .ok_or("Pi exact SDK resume lost canonical identity arguments")?;
        args[selector + 1] = path_text(&history.path)?;
        validate_args(&args, &history.path, &directory)?;
        Ok(Self {
            executable,
            cli_prefix,
            package_root,
            directory,
            native_id: native_id.into(),
            destination: destination.into(),
            history,
        })
    }

    pub(crate) fn destination(&self) -> &Path {
        &self.destination
    }
    pub(crate) fn session_file(&self) -> &Path {
        &self.history.path
    }

    /// Reuse adapter history inspection and its physical-file/prefix fence.
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        validate_installation(&self.package_root)?;
        let current = super::history::inspect_owned_history(&self.directory, &self.native_id)?
            .complete
            .ok_or("Pi exact SDK resume history disappeared")?;
        if current.path != self.history.path {
            return Err("Pi exact SDK resume canonical history changed".into());
        }
        self.history.sdk_open_receipt(&self.native_id)?;
        Ok(())
    }

    /// Materialize only after manager-owned history/selector validation, using
    /// its generation. Existing Pi bridge arguments and credential stay intact.
    pub(crate) fn materialize(
        &self,
        provider_args: &[String],
        generation: u64,
    ) -> Result<(String, Vec<String>), String> {
        if generation == 0 || generation > MAX_GENERATION {
            return Err("Pi exact SDK resume requires a valid provider generation".into());
        }
        let args = provider_args
            .strip_prefix(self.cli_prefix.as_slice())
            .ok_or("Pi installed CLI launch prefix changed")?;
        validate_args(args, &self.history.path, &self.directory)?;
        self.revalidate()?;
        let mut manifest = self.history.sdk_open_receipt(&self.native_id)?;
        manifest["packageRoot"] = path_text(&self.package_root)?.into();
        manifest["sessionFile"] = path_text(&self.history.path)?.into();
        manifest["sessionDir"] = path_text(&self.directory)?.into();
        manifest["sessionId"] = self.native_id.clone().into();
        manifest["cwd"] = path_text(&self.destination)?.into();
        let root = self
            .directory
            .join(".wardian-pi-exact")
            .join(generation.to_string());
        create_owned_directory(&self.directory.join(".wardian-pi-exact"))?;
        create_owned_directory(&root)?;
        let script = root.join("sdk-resume.mjs");
        let receipt = root.join("launch.json");
        publish_immutable(&script, LAUNCHER.as_bytes())?;
        publish_immutable(
            &receipt,
            serde_json::to_vec(&manifest)
                .map_err(|error| error.to_string())?
                .as_slice(),
        )?;
        let mut launch_args = vec![path_text(&script)?, path_text(&receipt)?];
        launch_args.extend_from_slice(args);
        Ok((self.executable.clone(), launch_args))
    }
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or("Pi SDK launch path must be UTF-8".into())
}

fn validate_installation(root: &Path) -> Result<(), String> {
    let package: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("package.json"))
            .map_err(|error| format!("Pi SDK package unavailable: {error}"))?,
    )
    .map_err(|error| format!("Pi SDK package metadata is invalid: {error}"))?;
    if package["name"] != "@earendil-works/pi-coding-agent"
        || package["version"] != SDK_VERSION
        || package["exports"]["."]["import"] != "./dist/index.js"
    {
        return Err(format!(
            "Pi exact resume requires supported npm SDK {SDK_VERSION}"
        ));
    }
    for relative in [
        "dist/index.js",
        "dist/cli.js",
        "dist/extensions/llama/index.js",
    ] {
        if !root.join(relative).is_file() {
            return Err(format!(
                "Pi exact resume SDK module is unavailable: {relative}"
            ));
        }
    }
    Ok(())
}

fn resolve_sdk(
    executable: &str,
    prefix: Vec<String>,
) -> Result<(String, Vec<String>, PathBuf), String> {
    let (node, cli, prefix) = if prefix.len() == 1 {
        (executable.to_owned(), PathBuf::from(&prefix[0]), prefix)
    } else if prefix.is_empty() {
        let cli = std::fs::canonicalize(executable)
            .map_err(|_| "Pi exact resume requires an installed npm SDK CLI".to_string())?;
        ("node".into(), cli, Vec::new())
    } else {
        return Err("Pi exact resume does not support this CLI executable prefix".into());
    };
    if !cli.is_absolute() || cli.file_name().and_then(|name| name.to_str()) != Some("cli.js") {
        return Err("Pi exact resume requires the npm CLI public SDK installation".into());
    }
    let root = cli
        .parent()
        .and_then(Path::parent)
        .ok_or("Pi SDK package directory could not be resolved")?
        .to_path_buf();
    validate_installation(&root)?;
    let node = if Path::new(&node).is_absolute() && Path::new(&node).is_file() {
        node
    } else {
        std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths).find_map(|path| {
                    let candidate = path.join(if cfg!(windows) { "node.exe" } else { "node" });
                    candidate
                        .is_file()
                        .then(|| candidate.to_string_lossy().into_owned())
                })
            })
            .ok_or("Pi exact resume requires an installed Node.js executable")?
    };
    Ok((node, prefix, root))
}

fn publish_immutable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(bytes)
            .map_err(|error| format!("Pi SDK publication: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !std::fs::symlink_metadata(path)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_file()
                || std::fs::read(path).map_err(|error| error.to_string())? != bytes
            {
                return Err("Pi SDK generation file changed before launch".into());
            }
            Ok(())
        }
        Err(error) => Err(format!("Pi SDK publication: {error}")),
    }
}

fn create_owned_directory(path: &Path) -> Result<(), String> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err("Pi SDK generation directory is not a plain owned directory".into());
            }
            Ok(())
        }
        Err(error) => Err(format!("Pi SDK launch directory: {error}")),
    }
}

/// Deliberately unsupported: other session modes, one-shot commands, @file CLI
/// attachments, runtime API-key overrides and unknown extension flags. Reject
/// before stopping the old owner, never silently omit their behavior.
fn validate_args(args: &[String], session_file: &Path, directory: &Path) -> Result<(), String> {
    let mut index = 0;
    let mut session_seen = false;
    let mut directory_seen = false;
    while index < args.len() {
        let argument = args[index].as_str();
        let takes_value = matches!(
            argument,
            "--provider"
                | "--model"
                | "--thinking"
                | "--system-prompt"
                | "--append-system-prompt"
                | "--name"
                | "-n"
                | "--session"
                | "--session-dir"
                | "--models"
                | "--tools"
                | "-t"
                | "--exclude-tools"
                | "-xt"
                | "--extension"
                | "-e"
                | "--skill"
                | "--prompt-template"
                | "--theme"
                | "--use-theme"
                | "--tui-mode"
        );
        if takes_value {
            let value = args
                .get(index + 1)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("Pi exact resume requires a value for {argument}"))?;
            match argument {
                "--session" => {
                    if session_seen || Path::new(value) != session_file {
                        return Err("Pi exact resume selector conflicts with owned history".into());
                    }
                    session_seen = true;
                }
                "--session-dir" => {
                    if directory_seen || Path::new(value) != directory {
                        return Err("Pi exact resume directory conflicts with owned history".into());
                    }
                    directory_seen = true;
                }
                "--thinking"
                    if !matches!(
                        value.as_str(),
                        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                    ) =>
                {
                    return Err("Pi exact resume has an unsupported thinking level".into());
                }
                "--tui-mode" if !matches!(value.as_str(), "regular" | "fullscreen") => {
                    return Err("Pi exact resume has an unsupported TUI mode".into());
                }
                _ => {}
            }
            index += 2;
            continue;
        }
        if !matches!(
            argument,
            "--no-tools"
                | "-nt"
                | "--no-builtin-tools"
                | "-nbt"
                | "--no-extensions"
                | "-ne"
                | "--no-skills"
                | "-ns"
                | "--no-prompt-templates"
                | "-np"
                | "--no-themes"
                | "--no-context-files"
                | "-nc"
                | "--verbose"
                | "--approve"
                | "-a"
                | "--no-approve"
                | "-na"
                | "--offline"
        ) && (argument.starts_with('-') || argument.starts_with('@'))
        {
            return Err(format!(
                "Pi exact SDK resume does not support argument {argument}"
            ));
        }
        index += 1;
    }
    if !session_seen || !directory_seen {
        return Err("Pi exact resume lost managed session selectors".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        version: u64,
        assistant: bool,
    ) -> (tempfile::TempDir, AgentConfig, String, Vec<String>, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("sdk");
        std::fs::create_dir_all(package.join("dist/extensions/llama")).unwrap();
        std::fs::write(package.join("package.json"), r#"{"name":"@earendil-works/pi-coding-agent","version":"0.84.2","exports":{".":{"import":"./dist/index.js"}}}"#).unwrap();
        for file in [
            "dist/index.js",
            "dist/cli.js",
            "dist/extensions/llama/index.js",
        ] {
            std::fs::write(package.join(file), "fixture").unwrap();
        }
        let directory = root.path().join("sessions");
        let destination = root.path().join("destination");
        std::fs::create_dir(&directory).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let history = format!("{{\"type\":\"session\",\"version\":{version},\"id\":\"exact\",\"cwd\":\"source\"}}\n{}\n", if assistant { r#"{"type":"message","message":{"role":"assistant"}}"# } else { r#"{"type":"message","message":{"role":"user"}}"# });
        std::fs::write(directory.join("exact.jsonl"), history).unwrap();
        let config = AgentConfig {
            provider: "pi".into(),
            folder: destination.to_string_lossy().into(),
            resume_session: Some("exact".into()),
            ..Default::default()
        };
        let args = vec![
            "--session".into(),
            "exact".into(),
            "--session-dir".into(),
            path_text(&directory).unwrap(),
            "--tui-mode".into(),
            "regular".into(),
        ];
        let executable = path_text(&package.join("dist/cli.js")).unwrap();
        (root, config, executable, args, directory)
    }

    #[test]
    fn preflight_is_read_only_and_launch_preserves_exact_file_destination_and_bridge() {
        let (root, config, cli, args, directory) = fixture(3, true);
        let before = std::fs::read(directory.join("exact.jsonl")).unwrap();
        let plan = PiExactResumePlan::prepare_from(
            "node",
            vec![cli.clone()],
            directory.clone(),
            "exact",
            Path::new(&config.folder),
            args,
        )
        .unwrap();
        assert_eq!(plan.destination(), Path::new(&config.folder));
        assert_eq!(plan.session_file(), directory.join("exact.jsonl"));
        assert!(!directory.join(".wardian-pi-exact").exists());
        let bridge = root.path().join("bridge.mjs");
        let provider_args = vec![
            cli,
            "--session".into(),
            path_text(plan.session_file()).unwrap(),
            "--session-dir".into(),
            path_text(&directory).unwrap(),
            "-e".into(),
            path_text(&bridge).unwrap(),
        ];
        let (_, launch) = plan.materialize(&provider_args, 7).unwrap();
        assert!(launch
            .windows(2)
            .any(|pair| pair == ["-e", bridge.to_str().unwrap()]));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&launch[1]).unwrap()).unwrap();
        assert_eq!(manifest["cwd"], config.folder);
        assert_eq!(manifest["sessionId"], "exact");
        assert_eq!(
            std::fs::read(directory.join("exact.jsonl")).unwrap(),
            before
        );
        plan.materialize(&provider_args, 7).unwrap();
        std::fs::write(&launch[0], "changed").unwrap();
        assert!(plan.materialize(&provider_args, 7).is_err());
    }

    #[test]
    fn unsupported_options_identity_and_history_fail_without_publication_or_mutation() {
        for (version, assistant) in [(2, true), (3, false)] {
            let (_root, config, cli, args, directory) = fixture(version, assistant);
            let before = std::fs::read(directory.join("exact.jsonl")).unwrap();
            assert!(PiExactResumePlan::prepare_from(
                "node",
                vec![cli],
                directory.clone(),
                "exact",
                Path::new(&config.folder),
                args
            )
            .is_err());
            assert_eq!(
                std::fs::read(directory.join("exact.jsonl")).unwrap(),
                before
            );
            assert!(!directory.join(".wardian-pi-exact").exists());
        }
        for unsupported in [
            "--fork",
            "--session-id",
            "--session=other",
            "--api-key",
            "--print",
            "--extension-flag",
            "@attachment",
        ] {
            let (_root, config, cli, mut args, directory) = fixture(3, true);
            args.push(unsupported.into());
            assert!(PiExactResumePlan::prepare_from(
                "node",
                vec![cli],
                directory.clone(),
                "exact",
                Path::new(&config.folder),
                args
            )
            .is_err());
            assert!(!directory.join(".wardian-pi-exact").exists());
        }
        let (_root, config, cli, args, directory) = fixture(3, true);
        assert!(PiExactResumePlan::prepare_from(
            "node",
            vec![cli],
            directory,
            "wrong",
            Path::new(&config.folder),
            args
        )
        .is_err());
    }
}
