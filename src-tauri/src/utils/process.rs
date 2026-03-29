#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub use_no_window: bool,
}

impl HeadlessCommandSpec {
    fn from_program_and_args(program: &str, args: Vec<String>) -> Self {
        Self {
            program: program.to_string(),
            args,
            use_no_window: cfg!(target_os = "windows"),
        }
    }
}

pub fn headless_command_spec(program: &str) -> HeadlessCommandSpec {
    if cfg!(target_os = "windows") && program.ends_with(".cmd") {
        HeadlessCommandSpec::from_program_and_args(
            "cmd",
            vec!["/c".to_string(), program.to_string()],
        )
    } else {
        HeadlessCommandSpec::from_program_and_args(program, Vec::new())
    }
}

pub fn new_headless_command(program: &str) -> tokio::process::Command {
    use tokio::process::Command;

    let spec = headless_command_spec(program);
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);

    #[cfg(target_os = "windows")]
    {
        if spec.use_no_window {
            cmd.creation_flags(0x08000000);
        }
    }

    cmd
}

#[cfg(test)]
mod tests {
    use super::headless_command_spec;

    #[test]
    fn wraps_cmd_shims_for_headless_windows_execution() {
        let spec = headless_command_spec("example.cmd");

        if cfg!(target_os = "windows") {
            assert_eq!(spec.program, "cmd");
            assert_eq!(spec.args, vec!["/c".to_string(), "example.cmd".to_string()]);
            assert!(spec.use_no_window);
        } else {
            assert_eq!(spec.program, "example.cmd");
            assert!(spec.args.is_empty());
            assert!(!spec.use_no_window);
        }
    }

    #[test]
    fn uses_binary_directly_when_no_cmd_wrapper_is_needed() {
        let spec = headless_command_spec("codex");

        assert_eq!(spec.program, "codex");
        assert!(spec.args.is_empty());
        assert_eq!(spec.use_no_window, cfg!(target_os = "windows"));
    }
}
