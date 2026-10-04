//! Exercise the normal native broker bootstrap with an inert protocol fixture.
use super::*;
use crate::utils::process::lifetime_test_support::{birth, member, wait_file, Helper};
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::time::Duration;

#[test]
#[ignore = "isolated native broker fixture entry point"]
fn native_broker_lifetime_helper() {
    let Some(root) = std::env::var_os("WARDIAN_BROKER_LIFETIME_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = NativeHomeGuard::set(&home);
    wardian_core::db::init_db_at_path(&home.join("state.db")).unwrap();
    std::env::set_var("WARDIAN_NATIVE_TEST_SCRIPT", root.join("provider.cjs"));
    let _script = NativeTestScriptGuard;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let spec = NativeSessionSpec {
            target_agent_id: "lifetime-fixture".into(), provider: "pi".into(), generation: 1,
            workspace: root.clone(), config: AgentConfig { provider: "pi".into(), session_id: "lifetime-fixture".into(), ..AgentConfig::default() },
        };
        let capabilities = NativeProviderProtocol::PiRpc.capabilities("fixture");
        // The production launcher, pipes, bootstrap and binding all run here.
        let owner = start_runtime(&spec, NativeProviderProtocol::PiRpc, &capabilities).await.unwrap();
        assert_eq!(owner.binding.provider_session_id.as_deref(), Some("lifetime-fixture-session"));
        let child = unsafe { BorrowedHandle::borrow_raw(owner.child.raw_handle().unwrap()) }.try_clone_to_owned().unwrap();
        let descendant = member(wait_file(&root, "tool-pid").parse().unwrap());
        assert!(crate::utils::process::test_owned_job_contains(child.as_raw_handle() as _));
        assert!(crate::utils::process::test_owned_job_contains(descendant.0.as_raw_handle() as _));
        std::fs::write(root.join("ready"), serde_json::json!({
            "child": owner.child.id(), "child_birth": birth(&child),
            "descendant": unsafe { winapi::um::processthreadsapi::GetProcessId(descendant.0.as_raw_handle() as _) },
            "descendant_birth": birth(&descendant.0),
        }).to_string()).unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(owner);
    });
}

#[test]
fn native_broker_owned_tree_exits_when_parent_is_abruptly_terminated() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("provider.cjs"), r#"
const fs = require('node:fs'), path = require('node:path'), cp = require('node:child_process');
const tool = cp.spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {stdio: 'ignore', windowsHide: true});
fs.writeFileSync(path.join(process.cwd(), 'tool-pid'), String(tool.pid));
require('node:readline').createInterface({input: process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  if (request.type === 'get_state') console.log(JSON.stringify({id: request.id, type: 'response', command: 'get_state', success: true, data: {sessionId: 'lifetime-fixture-session'}}));
});
"#).unwrap();
    let mut foreign = crate::utils::process::new_silent_std_command("node");
    foreign.args(["-e", "setInterval(() => {}, 1000)"]);
    let mut foreign = Helper(foreign.spawn().unwrap());
    let (_, module) = module_path!().split_once("::").unwrap();
    let mut command = crate::utils::process::new_silent_std_command(
        std::env::current_exe().unwrap().to_str().unwrap(),
    );
    command
        .args([
            "--ignored",
            "--exact",
            &format!("{module}::native_broker_lifetime_helper"),
        ])
        .env("WARDIAN_BROKER_LIFETIME_TEST_ROOT", temp.path())
        .env("WARDIAN_HOME", temp.path().join("home"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut parent = Helper(command.spawn().unwrap());
    let ready: serde_json::Value = serde_json::from_str(&wait_file(temp.path(), "ready")).unwrap();
    let mut child = member(ready["child"].as_u64().unwrap() as u32);
    child.authorize_cleanup(ready["child_birth"].as_u64().unwrap());
    let mut descendant = member(ready["descendant"].as_u64().unwrap() as u32);
    descendant.authorize_cleanup(ready["descendant_birth"].as_u64().unwrap());
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    for member in [&child, &descendant] {
        assert_eq!(
            unsafe {
                winapi::um::synchapi::WaitForSingleObject(member.0.as_raw_handle() as _, 5000)
            },
            winapi::um::winbase::WAIT_OBJECT_0
        );
    }
    assert!(foreign.0.try_wait().unwrap().is_none());
}
