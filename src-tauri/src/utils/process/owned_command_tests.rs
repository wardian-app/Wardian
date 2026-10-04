use super::*;
use std::os::windows::{
    io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    process::CommandExt,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use winapi::um::{
    processthreadsapi::{GetProcessTimes, OpenProcess, TerminateProcess},
    synchapi::WaitForSingleObject,
    winbase::WAIT_OBJECT_0,
    winnt::{PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZE},
};

const ROLE: &str = "WARDIAN_LIFETIME_TEST_ROLE";
const ROOT: &str = "WARDIAN_LIFETIME_TEST_ROOT";
const CASE: &str = "WARDIAN_LIFETIME_TEST_CASE";

pub(crate) struct Helper(pub(crate) std::process::Child);
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub(crate) struct Member(pub(crate) OwnedHandle, bool);
impl Member {
    /// An opened PID is observation only until its recorded birth matches.
    pub(crate) fn authorize_cleanup(&mut self, expected_birth: u64) {
        assert_eq!(birth(&self.0), expected_birth);
        self.1 = true;
    }
}
impl Drop for Member {
    fn drop(&mut self) {
        if self.1 && unsafe { WaitForSingleObject(self.0.as_raw_handle() as _, 0) } != WAIT_OBJECT_0
        {
            unsafe {
                TerminateProcess(self.0.as_raw_handle() as _, 1);
            }
            unsafe {
                WaitForSingleObject(self.0.as_raw_handle() as _, 5000);
            }
        }
    }
}
fn helper_args() -> Vec<String> {
    let (_, module) = module_path!().split_once("::").unwrap();
    vec![
        "--ignored".into(),
        "--exact".into(),
        format!("{module}::owned_process_helper"),
    ]
}
fn command(role: &str, root: &Path, case: &str) -> std::process::Command {
    let mut command =
        super::super::new_silent_std_command(std::env::current_exe().unwrap().to_str().unwrap());
    command
        .args(helper_args())
        .env(ROLE, role)
        .env(ROOT, root)
        .env(CASE, case)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}
fn child_command(root: &Path, case: &str) -> tokio::process::Command {
    let mut command =
        super::super::new_silent_command(std::env::current_exe().unwrap().to_str().unwrap());
    command
        .args(helper_args())
        .env(ROLE, "child")
        .env(ROOT, root)
        .env(CASE, case)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}
pub(crate) fn birth(handle: &OwnedHandle) -> u64 {
    let mut times = [unsafe { std::mem::zeroed::<winapi::shared::minwindef::FILETIME>() }; 4];
    let [created, exit, kernel, user] = &mut times;
    assert_ne!(
        unsafe { GetProcessTimes(handle.as_raw_handle() as _, created, exit, kernel, user) },
        0
    );
    (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
}
pub(crate) fn member(pid: u32) -> Member {
    let handle = unsafe {
        OpenProcess(
            SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | winapi::um::winnt::PROCESS_TERMINATE,
            0,
            pid,
        )
    };
    assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
    Member(unsafe { OwnedHandle::from_raw_handle(handle as _) }, false)
}
pub(crate) fn wait_file(root: &Path, name: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(value) = std::fs::read_to_string(root.join(name)) {
            if !value.is_empty() {
                return value;
            }
        }
        assert!(Instant::now() < deadline, "missing helper marker {name}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn contained(handle: &OwnedHandle, job: &win32job::Job) -> bool {
    let mut contained = 0;
    assert_ne!(
        unsafe {
            winapi::um::jobapi::IsProcessInJob(
                handle.as_raw_handle() as _,
                job.handle() as _,
                &mut contained,
            )
        },
        0
    );
    contained != 0
}
fn write_ready(root: &Path, child: &tokio::process::Child, descendant: Option<&Member>) {
    let child = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) }
        .try_clone_to_owned()
        .unwrap();
    let outer = &super::super::APP_PROCESS_SUPERVISOR
        .get()
        .unwrap()
        .as_ref()
        .unwrap()
        ._job;
    let inner = APP_OWNED_PROCESS_JOB.get().unwrap().as_ref().unwrap();
    let mut flags = 0;
    assert_ne!(
        unsafe { winapi::um::handleapi::GetHandleInformation(inner.handle() as _, &mut flags) },
        0
    );
    assert_eq!(flags & winapi::um::winbase::HANDLE_FLAG_INHERIT, 0);
    let record = serde_json::json!({
        "child": unsafe { winapi::um::processthreadsapi::GetProcessId(child.as_raw_handle() as _) },
        "child_birth": birth(&child), "inner_member": contained(&child, inner), "outer_member": contained(&child, outer),
        "descendant": descendant.map(|member| unsafe { winapi::um::processthreadsapi::GetProcessId(member.0.as_raw_handle() as _) }),
        "descendant_birth": descendant.map(|member| birth(&member.0)),
    });
    std::fs::write(root.join("ready"), record.to_string()).unwrap();
}

// Each role runs in a fresh test process. Environment changes are command-local.
#[test]
#[ignore = "isolated child entry point for process lifetime tests"]
fn owned_process_helper() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
    let case = std::env::var(CASE).unwrap();
    match role.as_str() {
        "parent" => {
            super::super::init_app_process_supervisor().unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _entered = runtime.enter();
            let mut command = child_command(&root, &case);
            if case == "preassign" {
                let _child = spawn_with_jobs(&mut command, None, |child| {
                    write_ready(&root, child, None);
                    std::thread::sleep(Duration::from_secs(30));
                })
                .unwrap();
            } else {
                let _child = if case == "updater" {
                    APP_OWNED_PROCESS_JOB
                        .get_or_init(|| {
                            super::super::create_kill_on_close_job("updater compatibility fixture")
                        })
                        .as_ref()
                        .unwrap();
                    command.creation_flags(
                        std::env::var("WARDIAN_LIFETIME_TEST_ESCAPE_FLAGS")
                            .unwrap()
                            .parse()
                            .unwrap(),
                    );
                    command.spawn().unwrap()
                } else {
                    spawn_owned_command(&mut command).unwrap()
                };
                let result = wait_file(&root, "descendant-result");
                let descendant = result.parse::<u32>().ok().map(member);
                if let Some(descendant) = &descendant {
                    assert_eq!(
                        contained(
                            &descendant.0,
                            APP_OWNED_PROCESS_JOB.get().unwrap().as_ref().unwrap()
                        ),
                        case != "updater"
                    );
                }
                write_ready(&root, &_child, descendant.as_ref());
                std::thread::sleep(Duration::from_secs(30));
            }
        }
        "child" => {
            std::fs::write(root.join("executed"), "yes").unwrap();
            // A tool that installs its own nested job must still start normally.
            let nested = win32job::Job::create().unwrap();
            nested.assign_current_process().unwrap();
            let mut command = command("descendant", &root, &case);
            if case == "breakaway" {
                command.creation_flags(0x0800_0000 | 0x0100_0000);
            }
            match command.spawn() {
                Ok(child) => {
                    let _child = Helper(child);
                    std::fs::write(root.join("descendant-result"), _child.0.id().to_string())
                        .unwrap();
                    std::thread::sleep(Duration::from_secs(30));
                }
                Err(error) => {
                    assert_eq!(case, "breakaway");
                    assert_eq!(error.raw_os_error(), Some(5));
                    std::fs::write(root.join("descendant-result"), "rejected").unwrap();
                    std::thread::sleep(Duration::from_secs(30));
                }
            }
        }
        "descendant" | "sentinel" => {
            std::thread::sleep(Duration::from_secs(30));
        }
        _ => panic!("unknown isolated helper role"),
    }
}

fn abrupt_parent_exit(case: &str) {
    abrupt_parent_exit_with_flags(case, None);
}

pub(crate) fn updater_escape_survives_parent_exit(flags: u32) {
    abrupt_parent_exit_with_flags("updater", Some(flags));
}

fn abrupt_parent_exit_with_flags(case: &str, flags: Option<u32>) {
    let temp = tempfile::tempdir().unwrap();
    let mut foreign = Helper(command("sentinel", temp.path(), case).spawn().unwrap());
    let mut parent_command = command("parent", temp.path(), case);
    if let Some(flags) = flags {
        parent_command.env("WARDIAN_LIFETIME_TEST_ESCAPE_FLAGS", flags.to_string());
    }
    let mut parent = Helper(parent_command.spawn().unwrap());
    let ready: serde_json::Value = serde_json::from_str(&wait_file(temp.path(), "ready")).unwrap();
    let mut child = member(ready["child"].as_u64().unwrap() as u32);
    child.authorize_cleanup(ready["child_birth"].as_u64().unwrap());
    let mut descendant = ready["descendant"].as_u64().map(|pid| member(pid as u32));
    if let Some(descendant) = &mut descendant {
        descendant.authorize_cleanup(ready["descendant_birth"].as_u64().unwrap());
    }
    assert_eq!(
        ready["inner_member"].as_bool().unwrap(),
        !matches!(case, "preassign" | "updater")
    );
    assert_eq!(ready["outer_member"].as_bool().unwrap(), case != "updater");
    if matches!(case, "normal" | "updater") {
        assert!(descendant.is_some());
    }
    if case == "breakaway" {
        assert_eq!(wait_file(temp.path(), "descendant-result"), "rejected");
    }
    let started = Instant::now();
    // This is abrupt termination, so no Rust destructor in the parent runs.
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    assert_eq!(
        unsafe {
            WaitForSingleObject(
                child.0.as_raw_handle() as _,
                if case == "updater" { 0 } else { 5000 },
            )
        } == WAIT_OBJECT_0,
        case != "updater"
    );
    if let Some(descendant) = &descendant {
        assert_eq!(
            unsafe {
                WaitForSingleObject(
                    descendant.0.as_raw_handle() as _,
                    if case == "updater" { 0 } else { 5000 },
                )
            } == WAIT_OBJECT_0,
            case != "updater"
        );
    }
    assert!(foreign.0.try_wait().unwrap().is_none());
    if case == "preassign" {
        assert!(!temp.path().join("executed").exists());
    }
    eprintln!("owned launch {case}: kernel identities verified, abrupt parent exit joined in {}ms, foreign sentinel survived", started.elapsed().as_millis());
}

#[test]
fn abrupt_parent_exit_kills_normal_owned_tree_and_preserves_foreign_process() {
    abrupt_parent_exit("normal");
}
#[test]
fn owned_tool_cannot_break_away_from_app_lifetime() {
    abrupt_parent_exit("breakaway");
}
#[test]
fn abrupt_parent_exit_before_inner_assignment_kills_suspended_root() {
    abrupt_parent_exit("preassign");
}

#[test]
fn mismatched_member_identity_preserves_foreign_process() {
    let temp = tempfile::tempdir().unwrap();
    let mut foreign = Helper(command("sentinel", temp.path(), "normal").spawn().unwrap());
    let mismatch = std::panic::catch_unwind(|| {
        let mut candidate = member(foreign.0.id());
        let incorrect_birth = birth(&candidate.0).wrapping_add(1);
        candidate.authorize_cleanup(incorrect_birth);
    });
    assert!(mismatch.is_err());
    assert!(foreign.0.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn failed_runtime_assignment_never_executes_and_joins_only_owned_child() {
    let temp = tempfile::tempdir().unwrap();
    let mut foreign = Helper(command("sentinel", temp.path(), "normal").spawn().unwrap());
    let job = super::super::create_kill_on_close_job("rejected test launch").unwrap();
    let mut limits: winapi::um::winnt::JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
        unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = winapi::um::winnt::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | winapi::um::winnt::JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    assert_ne!(
        unsafe {
            winapi::um::jobapi2::SetInformationJobObject(
                job.handle() as _,
                winapi::um::winnt::JobObjectExtendedLimitInformation,
                &mut limits as *mut _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        },
        0
    );
    let mut handle = None;
    let result = spawn_with_jobs(
        &mut child_command(temp.path(), "normal"),
        Some(&job),
        |child| {
            handle = Some(
                unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) }
                    .try_clone_to_owned()
                    .unwrap(),
            );
        },
    );
    assert!(result.is_err());
    assert_eq!(
        unsafe { WaitForSingleObject(handle.unwrap().as_raw_handle() as _, 0) },
        WAIT_OBJECT_0
    );
    assert!(!temp.path().join("executed").exists());
    assert!(foreign.0.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn owned_launch_preserves_stdio_arguments_environment_cwd_and_tool_child() {
    let temp = tempfile::tempdir().unwrap();
    let mut command = super::super::new_silent_command("node");
    command.args(["-e", "const fs=require('fs'),cp=require('child_process'); const input=fs.readFileSync(0,'utf8'); const child=cp.spawnSync(process.execPath,['-e','process.stdout.write(process.env.WARDIAN_LIFETIME_VALUE)'],{encoding:'utf8'}); if(child.status!==0)process.exit(2); process.stdout.write(JSON.stringify({input,arg:process.argv[1],env:child.stdout,cwd:process.cwd()})); process.stderr.write('owned stderr');", "literal spaces \"quotes\" λ"])
        .env("WARDIAN_LIFETIME_VALUE", "tool child value")
        .current_dir(temp.path())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = spawn_owned_command(&mut command).unwrap();
    use tokio::io::AsyncWriteExt;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"owned stdin")
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stderr, b"owned stderr");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["input"], "owned stdin");
    assert_eq!(value["arg"], "literal spaces \"quotes\" λ");
    assert_eq!(value["env"], "tool child value");
    assert_eq!(
        std::path::PathBuf::from(value["cwd"].as_str().unwrap()),
        temp.path()
    );
}
