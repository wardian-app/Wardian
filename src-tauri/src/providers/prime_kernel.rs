//! Provisioning for Prime Agent's IPython kernel environment.
//!
//! Prime Agent 0.7.0 cannot bootstrap its own kernel on Windows, and the kernel
//! is the only tool it exposes to the model, so an install without one produces
//! an agent that can hold a conversation and do nothing else. Wardian builds the
//! environment instead.
//!
//! Two properties matter more than speed here:
//!
//! * **Nothing half-built is ever discoverable.** [`crate::providers::prime::kernel_python`]
//!   decides a kernel exists by looking for the interpreter file. A venv created
//!   before its packages finish installing would satisfy that check and then
//!   fail every tool call, which is exactly the failure the readiness gate is
//!   supposed to prevent. Provisioning therefore builds into a sibling staging
//!   directory, verifies the packages import, and only then moves it into place.
//! * **It never blocks a render.** Readiness runs on every provider-list paint,
//!   so it only ever *reads* [`provisioning_state`]. The work happens once in the
//!   background, started from app setup.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Package the kernel needs from PyPI. The other one ships with Prime itself.
const KERNEL_PYPI_PACKAGES: [&str; 1] = ["ipykernel"];

/// Modules that must import before a staged environment is published. `rlm` is
/// the module `prime-agent-runtime` installs, so importing it proves the local
/// path install actually landed rather than silently resolving to nothing.
const KERNEL_IMPORT_PROBE: &str = "import ipykernel, rlm";

/// Directory name Prime's bundled Python runtime lives in, relative to the
/// installed npm package root.
const BUNDLED_RUNTIME_RELATIVE: [&str; 2] = ["dist", "prime-agent-runtime"];

/// Where a Prime install keeps the runtime's build manifest. Used to confirm a
/// candidate directory is really the package and not an empty leftover.
const RUNTIME_MANIFEST: &str = "pyproject.toml";

/// How long a launch will wait for an in-flight provision before giving up and
/// reporting the ordinary readiness failure.
const LAUNCH_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Progress of the one background provision this process may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningState {
    /// Never attempted, or nothing to do.
    Idle,
    /// A background provision is building the environment.
    Running,
    /// A provision finished and published an environment.
    Done,
    /// A provision failed. The reason is user-facing.
    Failed(String),
}

/// Why a provision cannot even be attempted. Each maps to advice the user can
/// act on, which is why they are distinct rather than one opaque error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningBlocker {
    /// No `uv` on PATH, so there is no way to build a virtualenv.
    NoUv,
    /// Prime's bundled Python runtime was not found next to its CLI.
    NoBundledRuntime,
    /// No Wardian home, so there is nowhere to put the environment.
    NoWardianHome,
}

impl ProvisioningBlocker {
    pub fn reason(&self) -> String {
        match self {
            Self::NoUv => "Wardian could not set up Prime Agent's Python kernel because `uv` is not installed. Install uv (https://docs.astral.sh/uv/) and restart Wardian, or set PRIME_AGENT_KERNEL_PYTHON to an interpreter that already has ipykernel and prime-agent-runtime.".to_string(),
            Self::NoBundledRuntime => "Wardian could not set up Prime Agent's Python kernel because Prime's bundled prime-agent-runtime was not found next to its CLI. Reinstall with `npm install -g prime-agent`.".to_string(),
            Self::NoWardianHome => "Wardian could not set up Prime Agent's Python kernel because the Wardian home could not be resolved.".to_string(),
        }
    }
}

fn state_cell() -> &'static Mutex<ProvisioningState> {
    static STATE: OnceLock<Mutex<ProvisioningState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ProvisioningState::Idle))
}

fn lock_state() -> std::sync::MutexGuard<'static, ProvisioningState> {
    state_cell()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Current provisioning progress. Cheap enough for the render path.
pub fn provisioning_state() -> ProvisioningState {
    lock_state().clone()
}

/// Whether a provision may start from `state`.
///
/// A finished provision is not retried, because the only way to reach `Done`
/// with no usable kernel is for something to have removed it, and a retry loop
/// driven by that would reinstall forever. A failure may be retried, which is
/// what makes restarting Wardian a fix.
fn can_claim_slot(state: &ProvisioningState) -> bool {
    matches!(
        state,
        ProvisioningState::Idle | ProvisioningState::Failed(_)
    )
}

/// Claims the right to run the one background provision, if nothing else holds
/// it. Returns false when a provision is already running or has finished, which
/// is what keeps repeated calls from starting a second install.
fn claim_provisioning_slot() -> bool {
    let mut state = lock_state();
    if !can_claim_slot(&state) {
        return false;
    }
    *state = ProvisioningState::Running;
    true
}

/// Starts a background provision unless one already ran, the kernel is already
/// usable, or nothing about this machine makes provisioning possible.
///
/// Safe to call more than once; only the first call that finds work to do will
/// spawn a thread.
pub fn start_background_provisioning() {
    if crate::providers::prime::kernel_python().is_some() {
        return;
    }

    let plan = match provisioning_plan() {
        Ok(plan) => plan,
        Err(blocker) => {
            let mut state = lock_state();
            if matches!(*state, ProvisioningState::Idle) {
                *state = ProvisioningState::Failed(blocker.reason());
            }
            return;
        }
    };

    if !claim_provisioning_slot() {
        return;
    }

    std::thread::spawn(move || {
        crate::utils::log_debug("[Wardian] Setting up Prime Agent's Python kernel.");
        let outcome = match run_provisioning_plan(&plan) {
            Ok(python) => {
                crate::utils::log_debug(&format!(
                    "[Wardian] Prime Agent kernel ready at {}",
                    python.display()
                ));
                ProvisioningState::Done
            }
            Err(error) => {
                crate::utils::log_debug(&format!(
                    "[Wardian] Prime Agent kernel setup failed: {error}"
                ));
                ProvisioningState::Failed(format!(
                    "Wardian could not set up Prime Agent's Python kernel: {error} See docs/guide/provider-readiness.md to build it by hand."
                ))
            }
        };
        *lock_state() = outcome;
    });
}

/// Blocks until an in-flight provision settles, so a launch started moments
/// after app start waits for the kernel instead of being told it is missing.
///
/// Returns immediately when nothing is running.
pub fn wait_for_in_flight_provisioning() {
    if !matches!(provisioning_state(), ProvisioningState::Running) {
        return;
    }

    let deadline = std::time::Instant::now() + LAUNCH_WAIT_TIMEOUT;
    while matches!(provisioning_state(), ProvisioningState::Running) {
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Everything a provision needs, resolved before any work starts so a missing
/// prerequisite is reported as advice rather than a failed install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisioningPlan {
    pub uv: PathBuf,
    pub runtime_source: PathBuf,
    pub target: PathBuf,
    pub staging: PathBuf,
}

/// Resolves a plan, or the reason one cannot be made.
pub fn provisioning_plan() -> Result<ProvisioningPlan, ProvisioningBlocker> {
    let target =
        crate::providers::prime::wardian_kernel_venv_dir().ok_or(ProvisioningBlocker::NoWardianHome)?;
    let uv = uv_executable().ok_or(ProvisioningBlocker::NoUv)?;
    let runtime_source = bundled_runtime_dir().ok_or(ProvisioningBlocker::NoBundledRuntime)?;

    Ok(ProvisioningPlan {
        staging: staging_dir_for(&target),
        target,
        uv,
        runtime_source,
    })
}

/// Staging directory for a target environment.
///
/// A sibling of the target, so publishing is a rename within one volume rather
/// than a cross-device copy, and so a crashed run leaves the debris somewhere
/// obvious instead of in a system temp directory.
pub fn staging_dir_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "prime-kernel-venv".to_string());
    name.push_str(".staging");
    match target.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Arguments for the virtualenv creation step.
pub fn venv_args(plan: &ProvisioningPlan) -> Vec<String> {
    vec![
        "venv".to_string(),
        plan.staging.to_string_lossy().to_string(),
    ]
}

/// Arguments for the package installation step.
///
/// `prime-agent-runtime` is **not on PyPI** -- it is a source directory inside
/// the installed npm package -- so it goes in as a path. Passing it by name
/// fails with "not found in the package registry".
pub fn install_args(plan: &ProvisioningPlan) -> Vec<String> {
    let mut args = vec![
        "pip".to_string(),
        "install".to_string(),
        "--python".to_string(),
        staged_python(plan).to_string_lossy().to_string(),
    ];
    args.extend(KERNEL_PYPI_PACKAGES.iter().map(|name| name.to_string()));
    args.push(plan.runtime_source.to_string_lossy().to_string());
    args
}

/// Interpreter inside the staging environment.
pub fn staged_python(plan: &ProvisioningPlan) -> PathBuf {
    plan.staging
        .join(crate::providers::prime::VENV_PYTHON_RELATIVE)
}

/// Locates `uv` on PATH.
pub fn uv_executable() -> Option<PathBuf> {
    crate::providers::readiness::find_executable_for_provisioning("uv")
}

/// Locates Prime's bundled Python runtime by walking up from its CLI entry
/// point, which is the only path Wardian can be sure points at the install that
/// will actually run.
pub fn bundled_runtime_dir() -> Option<PathBuf> {
    use wardian_core::models::provider::AgentProvider;

    let (executable, args) = crate::providers::prime::PrimeProvider::new().get_executable();
    let entry = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&executable));
    let entry = std::fs::canonicalize(&entry).unwrap_or(entry);

    runtime_dir_from_cli_entry(&entry).filter(|dir| dir.join(RUNTIME_MANIFEST).is_file())
}

/// Derives the bundled runtime directory from a CLI entry point path.
///
/// The entry point sits somewhere under the installed package directory (for
/// npm that is `.../node_modules/prime-agent/dist/bundle/cli.js`), so the
/// package root is the nearest ancestor named `prime-agent`. Walking to a named
/// ancestor survives the layout changing depth, which a fixed number of `..`
/// steps would not.
pub fn runtime_dir_from_cli_entry(entry: &Path) -> Option<PathBuf> {
    let package_root = entry
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "prime-agent"))?;

    Some(
        BUNDLED_RUNTIME_RELATIVE
            .iter()
            .fold(package_root.to_path_buf(), |path, segment| {
                path.join(segment)
            }),
    )
}

/// Builds and publishes the environment described by `plan`.
///
/// The staging directory is removed on every exit path, so a failure never
/// leaves something that looks like a usable kernel.
fn run_provisioning_plan(plan: &ProvisioningPlan) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(&plan.staging);

    match build_staged_environment(plan) {
        Ok(()) => publish_staged_environment(plan),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&plan.staging);
            Err(error)
        }
    }
}

fn build_staged_environment(plan: &ProvisioningPlan) -> Result<(), String> {
    run_step(&plan.uv, &venv_args(plan), "create the virtualenv")?;
    run_step(&plan.uv, &install_args(plan), "install the kernel packages")?;
    verify_staged_environment(plan)
}

/// Proves the staged environment can actually import what the kernel needs.
///
/// `uv` reporting success is not enough: the interpreter has to load both
/// packages in the layout it will run in.
fn verify_staged_environment(plan: &ProvisioningPlan) -> Result<(), String> {
    let python = staged_python(plan);
    if !python.is_file() {
        return Err(format!(
            "the virtualenv produced no interpreter at {}.",
            python.display()
        ));
    }

    run_step(
        &python,
        &["-c".to_string(), KERNEL_IMPORT_PROBE.to_string()],
        "verify the kernel packages",
    )
}

/// Moves the verified environment into place.
///
/// Losing the rename to a concurrent provision is success, not failure: the
/// published environment was built the same way, so the staged copy is simply
/// discarded.
fn publish_staged_environment(plan: &ProvisioningPlan) -> Result<PathBuf, String> {
    let published = plan
        .target
        .join(crate::providers::prime::VENV_PYTHON_RELATIVE);

    if std::fs::rename(&plan.staging, &plan.target).is_err() {
        let _ = std::fs::remove_dir_all(&plan.staging);
        if published.is_file() {
            return Ok(published);
        }
        return Err(format!(
            "the environment could not be moved into {}.",
            plan.target.display()
        ));
    }

    Ok(published)
}

fn run_step(program: &Path, args: &[String], what: &str) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    crate::utils::process::apply_silent_std_command_policy(&mut command);

    let output = command
        .output()
        .map_err(|error| format!("could not {what}: {error}."))?;
    if output.status.success() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no error output")
        .trim();
    Err(format!("could not {what}: {detail}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_for(target: &Path) -> ProvisioningPlan {
        ProvisioningPlan {
            uv: PathBuf::from("uv"),
            runtime_source: PathBuf::from("/npm/prime-agent/dist/prime-agent-runtime"),
            staging: staging_dir_for(target),
            target: target.to_path_buf(),
        }
    }

    #[test]
    fn staging_sits_next_to_the_target_so_publishing_is_a_rename() {
        let target = Path::new("/home/.wardian/prime-kernel-venv");

        let staging = staging_dir_for(target);

        assert_eq!(staging.parent(), target.parent());
        assert_ne!(staging, target);
    }

    /// The runtime is not a registry package, and naming it as one is the exact
    /// mistake that made the documented setup impossible to follow.
    #[test]
    fn the_bundled_runtime_is_installed_by_path_not_by_name() {
        let plan = plan_for(Path::new("/home/.wardian/prime-kernel-venv"));

        let args = install_args(&plan);

        assert!(args.contains(&"/npm/prime-agent/dist/prime-agent-runtime".to_string()));
        assert!(!args.iter().any(|arg| arg == "prime-agent-runtime"));
    }

    /// Everything is built in staging, so a crash cannot leave an interpreter
    /// where discovery will find it.
    #[test]
    fn nothing_is_installed_into_the_published_location() {
        let target = Path::new("/home/.wardian/prime-kernel-venv");
        let plan = plan_for(target);
        let target_text = target.to_string_lossy().to_string();

        for arg in venv_args(&plan).iter().chain(install_args(&plan).iter()) {
            assert!(
                !arg.starts_with(&target_text) || arg.starts_with(&plan.staging.to_string_lossy().to_string()),
                "{arg} points at the published location"
            );
        }
    }

    #[test]
    fn the_install_targets_the_staged_interpreter() {
        let plan = plan_for(Path::new("/home/.wardian/prime-kernel-venv"));

        let args = install_args(&plan);
        let python_index = args.iter().position(|arg| arg == "--python").expect("--python");

        assert_eq!(
            args[python_index + 1],
            staged_python(&plan).to_string_lossy().to_string()
        );
    }

    /// Built from components rather than a literal: a backslash-separated path
    /// is one opaque component on Unix, so a Windows literal would make this
    /// test pass for the wrong reason on one platform and fail on the other.
    #[test]
    fn the_package_root_is_found_from_the_npm_cli_entry_point() {
        let entry = ["npm", "node_modules", "prime-agent", "dist", "bundle", "cli.js"]
            .iter()
            .fold(PathBuf::new(), |path, segment| path.join(segment));

        let runtime = runtime_dir_from_cli_entry(&entry).expect("runtime dir");

        assert!(runtime.ends_with(
            Path::new("prime-agent")
                .join("dist")
                .join("prime-agent-runtime")
        ));
    }

    /// A CLI entry that is not inside a Prime install must not resolve to some
    /// unrelated directory that would then be installed as a package.
    #[test]
    fn an_unrelated_entry_point_yields_no_runtime_directory() {
        assert_eq!(
            runtime_dir_from_cli_entry(Path::new("/usr/local/bin/something-else")),
            None
        );
    }

    #[test]
    fn a_verified_environment_is_published_by_rename() {
        let temp = tempfile::tempdir().expect("temp dir");
        let plan = plan_for(&temp.path().join("prime-kernel-venv"));
        let staged_python = staged_python(&plan);
        std::fs::create_dir_all(staged_python.parent().expect("bin dir")).expect("staging dirs");
        std::fs::write(&staged_python, "").expect("staged interpreter");

        let published = publish_staged_environment(&plan).expect("publish");

        assert!(published.is_file());
        assert!(!plan.staging.exists());
        assert_eq!(
            published,
            plan.target
                .join(crate::providers::prime::VENV_PYTHON_RELATIVE)
        );
    }

    /// Losing the race is not an error, but it must not leave staging behind.
    #[test]
    fn a_concurrent_publish_is_accepted_and_leaves_no_debris() {
        let temp = tempfile::tempdir().expect("temp dir");
        let plan = plan_for(&temp.path().join("prime-kernel-venv"));

        let staged_python = staged_python(&plan);
        std::fs::create_dir_all(staged_python.parent().expect("bin dir")).expect("staging dirs");
        std::fs::write(&staged_python, "").expect("staged interpreter");

        let winner = plan.target.join(crate::providers::prime::VENV_PYTHON_RELATIVE);
        std::fs::create_dir_all(winner.parent().expect("bin dir")).expect("winner dirs");
        std::fs::write(&winner, "").expect("winning interpreter");

        let published = publish_staged_environment(&plan).expect("publish");

        assert_eq!(published, winner);
        assert!(!plan.staging.exists());
    }

    /// An interrupted install leaves a directory but no interpreter, and that
    /// must be reported rather than published.
    #[test]
    fn a_staging_directory_with_no_interpreter_fails_verification() {
        let temp = tempfile::tempdir().expect("temp dir");
        let plan = plan_for(&temp.path().join("prime-kernel-venv"));
        std::fs::create_dir_all(&plan.staging).expect("staging dir");

        let error = verify_staged_environment(&plan).expect_err("verification should fail");

        assert!(error.contains("no interpreter"));
        assert!(!plan.target.exists());
    }

    #[test]
    fn every_blocker_names_something_the_user_can_do() {
        for blocker in [
            ProvisioningBlocker::NoUv,
            ProvisioningBlocker::NoBundledRuntime,
            ProvisioningBlocker::NoWardianHome,
        ] {
            let reason = blocker.reason();
            assert!(reason.starts_with("Wardian could not set up"), "{reason}");
            assert!(reason.len() > 60, "{reason}");
        }
    }

    /// Kept pure rather than driving the process-wide slot: a test that parked
    /// the real state in `Running` would make any concurrent test that takes
    /// the launch gate wait on a provision that will never finish.
    #[test]
    fn a_running_or_finished_provision_is_never_restarted() {
        assert!(!can_claim_slot(&ProvisioningState::Running));
        assert!(!can_claim_slot(&ProvisioningState::Done));
    }

    /// Restarting Wardian has to be able to fix a transient failure such as a
    /// dropped network.
    #[test]
    fn a_failed_provision_can_be_retried() {
        assert!(can_claim_slot(&ProvisioningState::Idle));
        assert!(can_claim_slot(&ProvisioningState::Failed(
            "network".to_string()
        )));
    }

    /// Provisioning is started from app setup only. A test run that reached
    /// `Running` would mean something starts installs on an import path, and
    /// the launch gate would then block on it.
    #[test]
    fn a_test_run_never_starts_an_install() {
        assert!(!matches!(
            provisioning_state(),
            ProvisioningState::Running
        ));
    }
}
