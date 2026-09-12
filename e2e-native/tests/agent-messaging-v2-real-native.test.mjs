// @tier manual — Two paid Codex agents; coordinator must freeze artifacts and use the native runner.
// POSIX: WARDIAN_E2E_REAL_MESSAGING_V2=1 WARDIAN_NATIVE_SKIP_BUILD=1 \
// WARDIAN_NATIVE_APP='<frozen-packaged-app>' WARDIAN_E2E_MESSAGING_CLI='<matching-cli>' \
// WARDIAN_E2E_CODEX_AUTH_HOME='<authorized-codex-home>' \
// node scripts/run-native-e2e.mjs e2e-native/tests/agent-messaging-v2-real-native.test.mjs
// PowerShell: set the same names with $env:NAME='value', then run that node command.
// The runner chooses/locks one fresh home and exports its home/run ID to this
// test. Optional WARDIAN_E2E_NATIVE_HOME must be fresh; use a new deep path under
// <checkout>/.tmp/e2e-native/messaging-v2-real/ for explicit compact-home evidence.
// On Windows the runner assigns the suspended test root to a kill-on-close Job
// Object before forks. Direct node --test is only for the non-provider contracts.
// Optional WARDIAN_E2E_MESSAGING_V2_APPROVE_TOOLS=1 approves ONLY the six named
// tools in the two already auto-registered private agent homes, before any turn.
// WARDIAN_E2E_MESSAGING_V2_LIFECYCLE=1 additionally tests idle information and
// one active-turn interrupt in attached_tui mode, with one extra provider task.
// WARDIAN_E2E_MESSAGING_V2_MODE=background (default) or attached_tui.
// Upgraded runs require WARDIAN_E2E_CODEX_EXPECTED_VERSION=0.154.0-alpha.6
// and WARDIAN_E2E_CODEX_EXECUTABLE=<absolute-native-executable>. These are
// evidence pins, not product executable overrides; readiness must match them.
// Each invocation owns fresh agents. Never use a skip as real acceptance.
// Test-only WARDIAN_E2E_MESSAGING_V2_TRUST_FIXTURE_WORKSPACES=0 leaves the
// isolated Git roots untrusted for diagnostics; default/1 explicitly trusts them.
// WARDIAN_E2E_MESSAGING_V2_SCREENSHOTS=1 captures one private startup image per
// agent for parent review. The harness never uploads screenshots.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { createReadStream } from "node:fs";
import path from "node:path";
import os from "node:os";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createNativeHarness, prepareIsolatedHome, startNativeSession, waitForAppShell, invokeTauri } from "../lib/harness.mjs";
import { startStdioRpc } from "../lib/stdio-json-rpc.mjs";
import { compactHomeEvidence, cleanupAgentCredentials, cleanupFixtureCredential } from "../lib/codex-compact-home-evidence.mjs";
import { HOME_LOCK_DIRECTORY, HOME_LOCK_FILE, lockHolderAlive, readHomeLock } from "../lib/sessionHome.mjs";

const MODEL = "gpt-5.6-luna";
const EFFORT = "low";
const BASELINE_VERSION = "0.153.4";
const TESTED_ALPHA_VERSION = "0.154.0-alpha.6";
const TOOLS = ["send_message", "followup_task", "receive_messages", "reply", "interrupt_agent", "list_agents"];
const execute = promisify(execFile);
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const SOURCES = [
  "Cargo.toml", "Cargo.lock", "src-tauri/Cargo.toml", "crates/wardian-cli/Cargo.toml", "crates/wardian-core/Cargo.toml",
  "e2e-native/tests/agent-messaging-v2-real-native.test.mjs", "e2e-native/lib/harness.mjs",
  "e2e-native/lib/stdio-json-rpc.mjs",
  "e2e-native/lib/codex-compact-home-evidence.mjs",
  "e2e-native/lib/sessionHome.mjs", "e2e-native/lib/sessionPorts.mjs",
  "e2e-native/lib/frozenArtifacts.mjs", "e2e-native/lib/native-artifact-resolution.mjs",
  "e2e-native/tests/frozen-artifacts.test.mjs",
  "scripts/run-native-e2e.mjs", "scripts/native-e2e-runner.mjs",
  "scripts/native-e2e-windows-supervisor.ps1",
  "crates/wardian-cli/src/mcp.rs", "crates/wardian-cli/src/mcp/definitions.rs",
  "crates/wardian-cli/src/mcp/messaging.rs", "crates/wardian-cli/src/live/messaging.rs",
  "crates/wardian-cli/src/main.rs", "crates/wardian-cli/src/live.rs",
  "crates/wardian-cli/src/args.rs", "crates/wardian-cli/src/errors.rs", "crates/wardian-cli/tests/mcp_stdio.rs",
  "crates/wardian-core/src/agent_messaging.rs", "crates/wardian-core/src/db/agent_messaging.rs",
  "crates/wardian-core/src/db/agent_messaging/provider_claims.rs",
  "src-tauri/src/control/agent_messaging.rs", "src-tauri/src/utils/codex_messaging.rs",
  "src-tauri/src/control/agent_messaging/native.rs", "src-tauri/src/state/interactions/agent_messaging.rs",
  "src-tauri/src/control/codex_background.rs", "src-tauri/src/control/headless_delivery.rs",
  "src-tauri/src/delivery/native_broker.rs", "src-tauri/src/delivery/native_session.rs",
  "src-tauri/src/delivery/codex_shared.rs", "src-tauri/src/delivery/codex_shared/owner.rs",
  "src-tauri/src/delivery/codex_shared/attachment.rs",
  "src-tauri/src/delivery/codex_shared/launch_config.rs",
  "src-tauri/src/delivery/codex_shared/launch_model.rs",
  "src-tauri/src/delivery/codex_shared/launch_config/journal.rs",
  "src-tauri/src/delivery/codex_shared/launch_config/leaves.rs",
  "src-tauri/src/delivery/codex_shared/launch_config/storage.rs",
  "src-tauri/src/delivery/codex_shared/launch_config/tests.rs",
  "src-tauri/src/delivery/codex_shared/launch_config/tests/recovery.rs",
  "src-tauri/src/delivery/codex_shared/proxy.rs",
  "src-tauri/src/delivery/codex_shared/completion.rs",
  "src-tauri/src/delivery/codex_shared/version.rs",
  "src-tauri/src/delivery/codex_shared/diagnostics.rs",
  "src-tauri/src/delivery/codex_shared/startup_tests.rs",
  "src-tauri/src/delivery/codex_shared/owner_preparation_tests.rs",
  "src-tauri/src/delivery/native_broker/codex.rs", "src-tauri/src/delivery/mod.rs",
  "src-tauri/src/manager/codex_shared.rs", "src-tauri/src/manager/mod.rs",
  "src-tauri/src/manager/codex_stop.rs", "src-tauri/src/manager/codex_stop/tests.rs",
  "src-tauri/src/manager/spawn.rs", "src-tauri/src/manager/headless.rs",
  "src-tauri/src/manager/codex.rs", "src-tauri/src/delivery/live_surface.rs",
  "src-tauri/src/delivery/codex_composer.rs", "src-tauri/src/delivery/provider_events.rs",
  "src-tauri/src/commands/terminal.rs", "src-tauri/src/commands/terminal_session.rs",
  "src-tauri/src/commands/agent.rs", "src-tauri/src/state/app_state.rs",
  "src-tauri/src/commands/agent/removal.rs",
  "src-tauri/src/commands/agent/removal_tests.rs",
  "src-tauri/src/commands/agent/lifecycle_tests.rs",
  "src-tauri/src/commands/agent_lifecycle.rs",
  "src-tauri/src/state/interactions.rs", "src-tauri/src/providers/codex.rs",
  "src-tauri/src/providers/factory.rs", "src-tauri/src/providers/readiness.rs",
  "src-tauri/src/providers/models.rs", "src-tauri/src/utils/app_settings.rs",
  "src-tauri/src/utils/fs.rs", "src-tauri/src/utils/mod.rs", "src-tauri/src/lib.rs",
  "src-tauri/src/utils/codex_home.rs",
  "src-tauri/src/utils/codex_home/cleanup.rs",
  "src-tauri/src/utils/codex_home/copy_metadata.rs",
  "src-tauri/src/utils/codex_home/migration.rs",
  "src-tauri/src/utils/codex_home/platform.rs",
  "src-tauri/src/utils/codex_home/platform_tests.rs",
  "src-tauri/src/utils/codex_home/storage.rs",
  "src-tauri/src/utils/codex_home/tests.rs",
  "src-tauri/src/utils/codex_home/tree.rs",
  "src-tauri/src/control.rs", "crates/wardian-core/src/control.rs",
  "crates/wardian-core/src/db.rs", "crates/wardian-core/src/lib.rs",
  "src-tauri/src/providers/fixtures/codex-0.153.4-inbox-output.json",
];

async function sha256(file) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
}

export function expectedTestVersion(env = process.env) {
  const pin = env.WARDIAN_E2E_CODEX_EXPECTED_VERSION;
  const executable = env.WARDIAN_E2E_CODEX_EXECUTABLE;
  if (executable !== undefined) {
    assert.ok(path.isAbsolute(executable), "Codex executable evidence pin must be absolute");
    assert.ok(pin, "An executable pin requires explicit WARDIAN_E2E_CODEX_EXPECTED_VERSION");
  }
  const version = pin ?? BASELINE_VERSION;
  assert.ok([BASELINE_VERSION, TESTED_ALPHA_VERSION].includes(version),
    "Harness trace contracts are pinned to 0.153.4 or exact 0.154.0-alpha.6; other releases need separate validation");
  if (version !== BASELINE_VERSION) assert.ok(executable, "Upgraded runs require WARDIAN_E2E_CODEX_EXECUTABLE evidence pin");
  return version;
}

export function assertExecutableVersion(output, expectedVersion) {
  assert.equal(output, `codex-cli ${expectedVersion}`, "Actual executable version differs from the exact test pin");
  return expectedVersion;
}

export function executableIdentity(executable, header, expectedVersion, platform = process.platform) {
  const codexName = (platform === "win32" ? /^codex[^/\\]*\.exe$/iu : /^codex[^/\\]*$/u).test(path.basename(executable));
  const magic = header.subarray(0, 4).toString("hex");
  const nativeFormat = platform === "win32" ? magic.startsWith("4d5a") :
    ["7f454c46", "feedface", "feedfacf", "cefaedfe", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca"].includes(magic);
  const nativeCodex = codexName && nativeFormat;
  if (expectedVersion !== BASELINE_VERSION) assert.ok(nativeCodex,
    "Upgraded executable evidence requires the pinned native Codex binary, not a wrapper");
  return { identity_kind: nativeCodex ? "native_codex_file" : "selected_launcher",
    native_binary_identity: nativeCodex ? "selected_path_and_hash" : "unresolved_from_readiness" };
}

async function executableEvidence(driver, expectedVersion) {
  const readiness = (await invokeTauri(driver, "list_provider_readiness")).find((entry) => entry.provider === "codex");
  assert.ok(readiness?.available && path.isAbsolute(readiness.executable ?? ""), "Codex readiness must identify its executable");
  const executable = await fs.realpath(readiness.executable);
  const file = await fs.open(executable, "r");
  const header = Buffer.alloc(4);
  try { await file.read(header, 0, header.length, 0); } finally { await file.close(); }
  // Readiness can expose Node without its script argument. Preserve baseline
  // resolution, but never label its launcher hash as the embedded Codex hash.
  const identity = executableIdentity(executable, header, expectedVersion);
  if (process.env.WARDIAN_E2E_CODEX_EXECUTABLE !== undefined) {
    assert.equal(executable, await fs.realpath(process.env.WARDIAN_E2E_CODEX_EXECUTABLE),
      "Product executable selection differs from the evidence pin; check isolated process PATH discovery before running");
  }
  const evidence = { source: executable, selected_path: readiness.executable, sha256: await sha256(executable),
    expected_version: expectedVersion, version_source: "forced_live_provider_catalog", ...identity };
  return evidence;
}

function within(root, file) {
  const relative = path.relative(path.toNamespacedPath(path.resolve(root)), path.toNamespacedPath(path.resolve(file)));
  assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative), "Path must remain inside the owned fixture");
  return file;
}

async function writableOwnedFile(root, file) {
  within(root, file);
  const stat = await fs.lstat(file);
  assert.ok(stat.isFile() && !stat.isSymbolicLink() && stat.nlink === 1, "Fixture mutation requires an unlinked private file");
  within(await fs.realpath(root), await fs.realpath(file));
}

async function validateRunnerHome(harness, env = process.env) {
  assert.ok(path.isAbsolute(env.WARDIAN_E2E_NATIVE_HOME ?? ""), "Real acceptance requires scripts/run-native-e2e.mjs to supply its owned home");
  assert.equal(harness.isolatedHome, env.WARDIAN_E2E_NATIVE_HOME);
  assert.equal(harness.runId, env.WARDIAN_E2E_RUN_ID);
  assert.match(harness.runId ?? "", /^[a-z0-9_-]{1,96}$/iu, "A runner-owned run ID is required");
  const home = harness.isolatedHome;
  const directory = await fs.lstat(home);
  assert.ok(directory.isDirectory() && !directory.isSymbolicLink(), "Runner home must be a real directory");
  assert.equal(path.toNamespacedPath(await fs.realpath(home)), path.toNamespacedPath(path.resolve(home)), "Runner home must have canonical, unlinked parents");
  assert.deepEqual(await fs.readdir(home), [HOME_LOCK_DIRECTORY], "Real acceptance requires a fresh home containing only the runner's lock; existing evidence is never reset");
  const lockDirectory = await fs.lstat(path.join(home, HOME_LOCK_DIRECTORY));
  assert.ok(lockDirectory.isDirectory() && !lockDirectory.isSymbolicLink(), "Runner lock directory must not be linked");
  await writableOwnedFile(home, path.join(home, HOME_LOCK_FILE));
  const lock = readHomeLock(home);
  assert.equal(lock?.runId, harness.runId, "Runner home lock belongs to another run");
  assert.ok(lock.pid !== process.pid && lockHolderAlive(lock.pid), "The upstream runner must hold this home from outside the test process");
  return lock;
}

function fixtureEnvironmentOverrides(profile, fixtureCodex, env = process.env, platform = process.platform) {
  const overrides = { HOME: profile, CODEX_HOME: fixtureCodex };
  // Redirecting Windows USERPROFILE prevents WebView2's debugging endpoint
  // from starting. Keep its native value; provider paths remain explicit.
  if (platform !== "win32") overrides.USERPROFILE = profile;
  for (const key of Object.keys(env)) {
    if (/^(WARDIAN_SESSION_ID|WARDIAN_MEMORY_CAPABILITY|OPENAI_|ANTHROPIC_)/iu.test(key)) overrides[key] = undefined;
  }
  return overrides;
}

async function command(cli, home, cwd, args, timeout = 15_000) {
  const env = { ...process.env, WARDIAN_HOME: home };
  delete env.WARDIAN_SESSION_ID;
  try {
    const { stdout } = await execute(cli, args, { cwd, env, timeout, windowsHide: true, maxBuffer: 4 * 1024 * 1024 });
    return JSON.parse(stdout);
  } catch (cause) {
    // Never copy arbitrary provider stderr/environment into the evidence report.
    // eslint-disable-next-line preserve-caught-error -- execFile errors contain unfiltered stdout/stderr; retain only the transport code.
    throw new Error(`CLI ${args[0]} failed; operation was not replayed (code ${cause.code ?? "unknown"})`);
  }
}

async function fixtureGit(workspace, args) {
  // Ignore caller Git redirects and global templates/config. Only the new
  // fixture repositories may be mutated, even inside this checkout's worktree.
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^GIT_/iu.test(key)));
  env.GIT_CONFIG_GLOBAL = process.platform === "win32" ? "NUL" : os.devNull;
  env.GIT_CONFIG_NOSYSTEM = "1";
  const { stdout } = await execute("git", ["-c", "core.longpaths=true", "-C", workspace, ...args],
    { env, windowsHide: true, timeout: 15_000, maxBuffer: 64 * 1024 });
  return stdout.trim();
}

async function prepareFixtureWorkspaces(home) {
  const canonicalHome = await fs.realpath(home);
  const parent = within(home, path.join(home, "workspaces"));
  try { await fs.mkdir(parent); } catch (error) { if (error.code !== "EEXIST") throw error; }
  assert.ok((await fs.lstat(parent)).isDirectory() && !(await fs.lstat(parent)).isSymbolicLink());
  within(canonicalHome, await fs.realpath(parent));
  const workspaces = {};
  for (const role of ["sender", "receiver"]) {
    const workspace = path.join(parent, role);
    await fs.mkdir(workspace); // Exclusive creation; never adopt an existing workspace.
    await fixtureGit(workspace, ["init", "--quiet", "--template=", "--initial-branch=fixture"]);
    const gitDir = path.join(workspace, ".git");
    const stat = await fs.lstat(gitDir);
    assert.ok(stat.isDirectory() && !stat.isSymbolicLink(), "Fixture must have its own Git directory, not a worktree link");
    const root = await fs.realpath(await fixtureGit(workspace, ["rev-parse", "--show-toplevel"]));
    assert.equal(root, await fs.realpath(workspace), "Fixture inherited the parent repository root");
    assert.equal(await fs.realpath(await fixtureGit(workspace, ["rev-parse", "--absolute-git-dir"])), await fs.realpath(gitDir));
    within(canonicalHome, root);
    assert.equal(await fixtureGit(workspace, ["rev-list", "--all", "--count"]), "0", "Fixture repositories must have no commits");
    workspaces[role] = { workspace, git_root: root, git_dir: gitDir, commits: 0 };
  }
  return workspaces;
}

function fixtureTrustEnabled(env = process.env) {
  const value = env.WARDIAN_E2E_MESSAGING_V2_TRUST_FIXTURE_WORKSPACES;
  assert.ok(value === undefined || value === "0" || value === "1", "Fixture trust flag must be 0 or 1");
  return value !== "0";
}

async function configureFixtureTrust(driver, home, workspaces, trusted) {
  assert.deepEqual(Object.keys(workspaces).sort(), ["receiver", "sender"]);
  for (const { workspace, git_root } of Object.values(workspaces)) {
    within(await fs.realpath(home), await fs.realpath(workspace));
    assert.equal(await fs.realpath(workspace), git_root);
  }
  assert.equal((await invokeTauri(driver, "list_agents")).length, 0, "Enable test trust only before creating the two fixture agents");
  const document = await invokeTauri(driver, "load_shell_settings");
  assert.equal(document.schema_version, 2);
  const settings = { ...document,
    settings: { ...document.settings, codex_runtime_policy: { ...document.settings.codex_runtime_policy, trust_workspaces: trusted } },
    overrides: { ...document.overrides, codex_runtime_policy: { ...document.overrides.codex_runtime_policy, trust_workspaces: trusted } } };
  const saved = await invokeTauri(driver, "save_shell_settings", { settings });
  assert.equal(saved.settings.codex_runtime_policy.trust_workspaces, trusted);
  const file = path.join(home, "settings", "shell.json");
  await writableOwnedFile(home, file);
  const persisted = JSON.parse(await fs.readFile(file, "utf8"));
  assert.equal(persisted.overrides.codex_runtime_policy.trust_workspaces, trusted);
  return { api: "save_shell_settings", scope: "isolated_test_home_only", mode: trusted ? "trusted" : "untrusted", trust_workspaces: trusted,
    workspaces, settings_path: file, settings_sha256: await sha256(file) };
}

function fixtureProjectTrust(config, home, workspace) {
  within(home, workspace);
  // Inspect only the generated project-table spelling, not arbitrary user TOML.
  const headers = [...config.matchAll(/^\[projects\.(.+)\]\s*$/gmu)];
  assert.ok(headers.length <= 1, "Private config must not contain additional project trust tables");
  if (!headers.length) return [];
  const key = generatedString(`key = ${headers[0][1]}`, "key");
  assert.equal(path.toNamespacedPath(path.resolve(key)), path.toNamespacedPath(path.resolve(workspace)), "Trust key points outside the exact fixture Git root");
  const block = section(config, `projects.${headers[0][1]}`);
  // LaunchConfigGuard restores the owned leaf, retaining empty parent tables.
  if (!/^\s*trust_level\s*=/mu.test(block)) return [];
  return [{ project_key: key, trust_level: generatedString(block, "trust_level") }];
}

function assertFixtureLaunchTrust(doctor, home, agent, trusted) {
  assert.equal(doctor.applicable, true);
  assert.equal(doctor.agent.uuid, agent.session_id);
  assert.equal(doctor.agent.provider, "codex");
  within(home, agent.workspace);
  assert.equal(path.toNamespacedPath(path.resolve(doctor.agent.workspace)), path.toNamespacedPath(path.resolve(agent.workspace)));
  assert.equal(path.toNamespacedPath(path.resolve(doctor.codex_home)), path.toNamespacedPath(path.resolve(agent.registration.codex_home)));
  assert.ok(Array.isArray(doctor.launch_flags));
  const projects = [];
  for (let index = 0; index < doctor.launch_flags.length; index += 1) {
    if (!["-c", "--config"].includes(doctor.launch_flags[index])) continue;
    const value = doctor.launch_flags[++index];
    assert.equal(typeof value, "string");
    if (!value.startsWith("projects.")) continue;
    const match = /^projects\.("(?:[^"\\]|\\.)*"|'[^']*')\.trust_level\s*=\s*("trusted"|'trusted')$/u.exec(value);
    assert.ok(match, "Unrecognized planned project trust override");
    const key = generatedString(`key = ${match[1]}`, "key");
    assert.equal(path.toNamespacedPath(path.resolve(key)), path.toNamespacedPath(path.resolve(agent.workspace)), "Planned trust points outside the exact fixture Git root");
    projects.push({ project_key: key, trust_level: "trusted" });
  }
  assert.equal(projects.length, trusted ? 1 : 0, "Normal planned launch flags must match the selected fixture trust policy");
  return { source: "agent_doctor_launch_flags", planned_launch_only: true, projects,
    launch_flags_sha256: createHash("sha256").update(JSON.stringify(doctor.launch_flags)).digest("hex") };
}

function assertFixtureTrustRestored(config, home, agent) {
  const restored = fixtureProjectTrust(config, home, agent.workspace);
  assert.deepEqual(restored, agent.project_trust_before, "Successful attachment must restore the pre-launch project trust leaves");
  return restored;
}

function section(config, name) {
  const header = `[${name}]`;
  const lines = config.split(/\r?\n/u);
  const start = lines.findIndex((line) => line.trim() === header);
  assert.ok(start >= 0, `Normal startup did not produce ${header}`);
  const tail = lines.slice(start + 1);
  const end = tail.findIndex((line) => line.trim().startsWith("["));
  return tail.slice(0, end < 0 ? undefined : end).join("\n");
}

function generatedString(block, key) {
  const line = block.split("\n").find((entry) => entry.trimStart().startsWith(`${key} =`));
  assert.ok(line, `Missing generated field ${key}`);
  // This inspects single-line strings generated by Wardian's TOML writer, not
  // arbitrary user TOML. An unexpected representation fails instead of guessing.
  const value = line.slice(line.indexOf("=") + 1).trim();
  if (/^'[^'\r\n]*'$/u.test(value)) return value.slice(1, -1);
  return JSON.parse(value);
}

async function registration(home, agent, expectedCliHash, approve, compact = null) {
  const codexHome = path.join(home, "agents", agent.session_id, "habitat", ".codex");
  const configPath = path.join(codexHome, "config.toml");
  if (compact) {
    assert.equal(compact.agent_id, agent.session_id);
    assert.equal(compact.complete, true);
    assert.equal(path.toNamespacedPath(await fs.realpath(codexHome)), path.toNamespacedPath(compact.physical_home));
    await writableOwnedFile(compact.physical_home, path.join(compact.physical_home, "config.toml"));
  } else {
    await writableOwnedFile(home, configPath);
  }
  const owner = JSON.parse(await fs.readFile(path.join(codexHome, ".wardian-messaging.json"), "utf8"));
  assert.equal(owner.agent_id, agent.session_id);
  assert.equal(path.toNamespacedPath(path.resolve(owner.wardian_home)), path.toNamespacedPath(path.resolve(home)));
  const original = await fs.readFile(configPath, "utf8");
  const server = section(original, "mcp_servers.wardian");
  const env = section(original, "mcp_servers.wardian.env");
  assert.match(server, /^args\s*=\s*\[\s*"mcp"\s*,\s*"serve"\s*\]\s*$/mu);
  const installedCli = generatedString(server, "command");
  assert.equal(path.toNamespacedPath(path.resolve(installedCli)), path.toNamespacedPath(path.resolve(owner.command)));
  within(home, await fs.realpath(installedCli));
  assert.equal(await sha256(installedCli), expectedCliHash, "Managed startup registered a different CLI artifact");
  assert.equal(path.toNamespacedPath(path.resolve(generatedString(env, "WARDIAN_HOME"))), path.toNamespacedPath(path.resolve(home)));
  assert.equal(generatedString(env, "WARDIAN_SESSION_ID"), agent.session_id);
  const beforeHash = await sha256(configPath);
  let fixtureGrant = "";
  if (approve) {
    assert.ok(!original.includes("[mcp_servers.wardian.tools."), "Fresh fixture unexpectedly contains per-tool policy; do not overwrite it");
    fixtureGrant = TOOLS.map((tool) => `\n[mcp_servers.wardian.tools.${tool}]\napproval_mode = "approve"\n`).join("");
    await fs.appendFile(configPath, fixtureGrant);
  }
  return { codex_home: codexHome, normal_registration: true, owner, args: ["mcp", "serve"],
    config_before_sha256: beforeHash, config_after_sha256: await sha256(configPath),
    fixture_permission_delta: fixtureGrant, installed_cli_sha256: expectedCliHash };
}

async function filesBelow(root) {
  const files = [];
  async function visit(directory) {
    for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) await visit(file);
      else if (entry.isFile() && entry.name.endsWith(".jsonl")) files.push(file);
      // Never follow a directory junction into shared history.
    }
  }
  await visit(root);
  return files;
}

function canonicalFrame(frame) {
  assert.equal(frame.schema_version, 1);
  for (const field of ["sender", "recipient", "interaction_id"]) assert.ok(typeof frame[field] === "string" && frame[field].length > 0);
  assert.equal(typeof frame.body, "string");
  assert.ok(["message", "task", "reply"].includes(frame.kind));
  if (frame.kind === "reply") {
    assert.ok(typeof frame.parent_interaction_id === "string" && frame.parent_interaction_id.length > 0);
    assert.equal(frame.request_id, frame.parent_interaction_id);
    assert.ok(["done", "blocked", "failed"].includes(frame.reply_status));
  } else {
    assert.equal(frame.parent_interaction_id, null);
    assert.equal(frame.reply_status, null);
    assert.equal(frame.request_id, frame.kind === "task" ? frame.interaction_id : null);
  }
  return frame;
}

// Installed host item shape; current broker has two deliberately distinct
// operations. No assumption that every output has a wrapper/message_id field.
export function decodeHostDelivery(payload) {
  assert.equal(payload.type, "function_call_output");
  assert.equal(payload.namespace, "wardian");
  assert.equal(payload.call_id ?? null, null, "Host delivery must not invent a model tool-call ID");
  const output = JSON.parse(payload.output);
  let context;
  let messageId;
  let frameType;
  if (payload.name === "wardian_task_delivery") {
    context = output;
    if (Object.hasOwn(output, "schema_version")) {
      canonicalFrame(context);
      assert.equal(context.kind, "task");
      messageId = context.interaction_id;
      frameType = "canonical_task";
    } else {
      // Initial ordinary `wardian send` remains a NativeMessageEnvelope,
      // directly serialized by dispatch_shared_codex. It is not a peer Task.
      assert.equal(output.operation, "start_turn");
      for (const field of ["interaction_id", "message_id", "target_agent_id"]) assert.ok(typeof output[field] === "string" && output[field].length > 0);
      assert.ok(Number.isSafeInteger(output.generation));
      assert.equal(typeof output.body, "string");
      messageId = output.message_id;
      frameType = "native_initial_envelope";
    }
  } else {
    assert.equal(payload.name, "wardian_inbox_delivery");
    context = canonicalFrame(output);
    assert.ok(["message", "reply"].includes(context.kind));
    messageId = context.interaction_id;
    frameType = "canonical_inbox";
  }
  return { name: payload.name, namespace: payload.namespace, message_id: messageId, context,
    frame_type: frameType, output_text: payload.output };
}

function recordMcpCall(calls, items, id, turnId, tool, args) {
  assert.ok(typeof id === "string" && id && turnId, "MCP call lacks its owned call/turn identity");
  const previous = calls.get(id);
  if (previous) {
    assert.equal(previous.turn_id, turnId, "MCP call ID was reused across turns");
    assert.equal(previous.tool, tool, "MCP call ID changed tool");
    assert.deepEqual(previous.arguments, args, "MCP call ID changed arguments");
    return previous;
  }
  const entry = { turn_id: turnId, type: "mcpToolCall", id, server: "wardian", tool,
    status: "inProgress", arguments: args };
  calls.set(id, entry);
  items.push(entry);
  return entry;
}

function recordMcpResult(entry, result) {
  let receipt;
  try {
    receipt = result.structuredContent ?? (result.content ? JSON.parse(result.content.find((item) => item.type === "text").text) : result);
    assert.ok(!result.isError && receipt && !receipt.error);
  } catch {
    entry.status = "failed";
    entry.error = "provider_tool_receipt_failed";
    return;
  }
  if (entry.receipt) assert.deepEqual(receipt, entry.receipt, "Duplicate MCP evidence changed its receipt");
  entry.receipt = receipt;
  if (!entry.error) entry.status = "completed";
}

async function ownedMetadata(codexHome, threadId, workspace, expectedVersion) {
  const candidates = (await filesBelow(path.join(codexHome, "sessions"))).filter((file) => path.basename(file).includes(threadId));
  assert.equal(candidates.length, 1, "Expected one exact owned provider rollout");
  const file = candidates[0];
  const { createInterface } = await import("node:readline");
  const lines = createInterface({ input: createReadStream(file), crlfDelay: Infinity });
  let meta;
  const contexts = new Map();
  const hostDeliveries = [];
  const visibleItems = [];
  const calls = new Map();
  const turns = new Map();
  let currentTurn;
  for await (const line of lines) {
    let row;
    try { row = JSON.parse(line); } catch { continue; } // A currently appended final line may be partial.
    if (row.type === "session_meta") {
      const p = row.payload;
      meta = { id: p.id, cwd: p.cwd, cli_version: p.cli_version, originator: p.originator };
    } else if (row.type === "turn_context") {
      const p = row.payload;
      currentTurn = p.turn_id;
      const effort = p.effort ?? p.collaboration_mode?.settings?.reasoning_effort ?? null;
      contexts.set(p.turn_id, { turn_id: p.turn_id, model: p.model, effort,
        effort_evidence: effort === null ? "not_reported_by_provider" : "owned_turn_context", cwd: p.cwd });
    } else if (row.type === "response_item" && row.payload?.type === "function_call_output" &&
      row.payload.namespace === "wardian" && ["wardian_task_delivery", "wardian_inbox_delivery"].includes(row.payload.name)) {
      hostDeliveries.push({ ...decodeHostDelivery(row.payload), turn_id: currentTurn, observed_at: row.timestamp });
    }
    const p = row.payload;
    if (row.type === "event_msg" && ["task_started", "task_complete", "turn_aborted"].includes(p?.type)) {
      const id = p.turn_id ?? currentTurn;
      assert.ok(id, "Provider lifecycle event lacks an owned turn");
      currentTurn = id;
      turns.set(id, { turn_id: id, status: p.type === "task_started" ? "inProgress" : p.type === "task_complete" ? "completed" : "interrupted" });
    }
    if (row.type === "response_item" && p?.type === "function_call" && p.namespace === "mcp__wardian" && TOOLS.includes(p.name)) {
      recordMcpCall(calls, visibleItems, p.call_id,
        p.internal_chat_message_metadata_passthrough?.turn_id ?? currentTurn, p.name, JSON.parse(p.arguments));
    } else if (row.type === "event_msg" && ["mcp_tool_call_begin", "mcp_tool_call_end"].includes(p?.type) &&
      p.invocation?.server === "wardian" && TOOLS.includes(p.invocation.tool)) {
      // Nested code-mode MCP calls have their own IDs. Legacy rollouts can
      // persist only the end event, which carries invocation and Result data.
      const entry = recordMcpCall(calls, visibleItems, p.call_id, currentTurn, p.invocation.tool, p.invocation.arguments);
      if (p.type === "mcp_tool_call_end") recordMcpResult(entry, p.result?.Ok ?? { isError: true });
    } else if (row.type === "response_item" && p?.type === "function_call_output" && calls.has(p.call_id)) {
      const entry = calls.get(p.call_id);
      let result;
      try {
        const marker = "\nOutput:\n";
        const body = p.output.includes(marker) ? p.output.slice(p.output.indexOf(marker) + marker.length) : p.output;
        result = JSON.parse(body);
      } catch { entry.status = "failed"; entry.error = "provider_tool_receipt_failed"; }
      if (result) recordMcpResult(entry, result);
    } else if (row.type === "response_item" && p?.type === "message" &&
      (p.role === "user" || (p.role === "assistant" && [undefined, null, "commentary", "final"].includes(p.channel)))) {
      visibleItems.push({ turn_id: p.internal_chat_message_metadata_passthrough?.turn_id ?? currentTurn,
        type: p.role === "assistant" ? "agentMessage" : "userMessage",
        channel: p.channel ?? (p.phase === "final_answer" ? "final" : p.phase === "commentary" ? "commentary" : undefined),
        text: (p.content ?? []).filter((item) => ["output_text", "input_text"].includes(item.type)).map((item) => item.text).join("") });
    }
    // No reasoning, token accounting, arbitrary tool output, or auth is retained.
  }
  assert.equal(meta?.id, threadId);
  assert.equal(path.toNamespacedPath(path.resolve(meta.cwd)), path.toNamespacedPath(path.resolve(workspace)));
  // Wardian's prepared session header identifies its bootstrap writer. Resuming
  // it does not rewrite that header; the negotiated binding proves CLI version.
  if (meta.originator !== "Wardian") assert.equal(meta.cli_version, expectedVersion);
  assert.ok(visibleItems.length <= 500, "Owned visible rollout exceeds its evidence bound");
  return { meta, contexts: [...contexts.values()], host_deliveries: hostDeliveries, rollout_path: file,
    rollout_items: visibleItems, rollout_turns: [...turns.values()] };
}

export async function visibleTrace(_DatabaseSync, codexHome, binding, workspace, expectedVersion = expectedTestVersion()) {
  const threadId = binding.provider_session_id;
  assert.ok(threadId, "Native binding lacks the actual Codex thread ID");
  assert.equal(binding.capabilities.protocol_version, expectedVersion, "Negotiated CLI differs from the exact test pin");
  const metadata = await ownedMetadata(codexHome, threadId, workspace, expectedVersion);
  // The attached app-server can leave thread_history_1.sqlite empty while
  // appending the complete native rollout. File existence is not projection
  // readiness. Always use the exact owned rollout for this protocol version,
  // retaining its native call IDs, turn context, receipts and completion events.
  return { provider_thread_id: threadId, generation: binding.generation, ...metadata,
    items: metadata.rollout_items, turns: metadata.rollout_turns, evidence_source: "owned_provider_rollout" };
}

/** Recover history after owner exit; this never manufactures a capable binding.
 * The persisted resume ID selects the file, while immutable acceptance evidence
 * must match the exact agent, interaction, generation and turn in that file.
 */
export async function historicalTrace(DatabaseSync, home, agent, interactionId) {
  const db = new DatabaseSync(path.join(home, "state.db"), { readOnly: true });
  let record;
  let accepted;
  let task;
  try {
    const row = db.prepare("SELECT record_json FROM native_deliveries WHERE interaction_id=?").get(interactionId);
    if (!row) {
      task = db.prepare("SELECT d.recipient,d.sender,d.generation,d.owner,i.body_ref FROM agent_message_delivery d JOIN interactions i ON i.id=d.interaction_id WHERE d.interaction_id=? AND d.operation='followup_task'").get(interactionId);
      if (!task || !["provider_accepted", "provider_visible", "provider_completed"].includes(task.owner)) return null;
      assert.equal(task.recipient, agent.session_id);
      assert.ok(Number.isSafeInteger(task.generation));
    } else {
      record = JSON.parse(row.record_json);
      assert.equal(record.envelope.interaction_id, interactionId);
      assert.equal(record.envelope.target_agent_id, agent.session_id);
      assert.equal(record.provider, "codex");
      assert.ok(Number.isSafeInteger(record.envelope.generation));
      accepted = db.prepare("SELECT evidence_json FROM native_delivery_evidence WHERE interaction_id=? AND phase='provider_accepted'")
        .all(interactionId).map((row) => JSON.parse(row.evidence_json));
    }
  } finally { db.close(); }
  if (!task && !accepted.length) return null;
  for (const evidence of accepted ?? []) {
    assert.equal(evidence.interaction_id, interactionId);
    assert.equal(evidence.target_agent_id, agent.session_id);
    assert.equal(evidence.generation, record.envelope.generation);
    assert.equal(evidence.provider, "codex");
    assert.equal(evidence.source, "provider_event");
    assert.equal(evidence.provider_turn_id, record.provider_turn_id);
    assert.ok(evidence.provider_turn_id);
  }
  const configs = JSON.parse(await fs.readFile(path.join(home, "settings", "state.json"), "utf8"));
  const configsForAgent = configs.filter((config) => config.session_id === agent.session_id);
  assert.equal(configsForAgent.length, 1, "Expected one persisted agent identity");
  const threadId = configsForAgent[0].resume_session;
  assert.ok(typeof threadId === "string" && threadId, "Persisted agent lacks its accepted resume ID");
  const metadata = await ownedMetadata(agent.registration.codex_home, threadId, agent.workspace, agent.expected_version);
  let turnId = record?.provider_turn_id;
  if (task) {
    const body = JSON.parse(task.body_ref);
    assert.equal(body.storage, "inline");
    const deliveries = metadata.host_deliveries.filter((delivery) => delivery.frame_type === "canonical_task" && delivery.message_id === interactionId);
    assert.ok(deliveries.length <= 1, "Task appeared more than once in provider history");
    if (!deliveries.length) return null;
    const context = hostContext(deliveries[0]);
    assert.equal(context.sender, task.sender);
    assert.equal(context.recipient, task.recipient);
    assert.equal(context.request_id, interactionId);
    assert.equal(hostBody(context), body.body);
    turnId = deliveries[0].turn_id;
  }
  // Projection can lag acceptance. Do not attribute a different turn or claim
  // failure from absence until the exact terminal event is present.
  if (!turnId || !metadata.rollout_turns.some((turn) => turn.turn_id === turnId)) return null;
  return { provider_thread_id: threadId, generation: task?.generation ?? record.envelope.generation, ...metadata,
    items: metadata.rollout_items, turns: metadata.rollout_turns, evidence_source: "owned_provider_rollout",
    identity_source: task ? "persisted_resume_and_task_acceptance" : "persisted_resume_and_native_acceptance", accepted_turn_id: turnId,
    interaction_id: interactionId, delivery_phase: task?.owner ?? record.phase,
    protocol_version_evidence: metadata.meta.originator === "Wardian" ? "bootstrap_header_only_not_negotiation" : "provider_rollout_header" };
}

export function terminalExchangeFailure(trace, turnId, requiredTool) {
  const turn = trace?.turns.find((turn) => turn.turn_id === turnId);
  if (!turn || !["completed", "interrupted"].includes(turn.status)) return null;
  if (turn.status === "completed" && trace.items.some((item) => item.turn_id === turnId && item.tool === requiredTool)) return null;
  return { reason: `Owned provider turn ${turn.status} without ${requiredTool}`,
    provider_thread_id: trace.provider_thread_id, provider_turn_id: turnId,
    final_message: trace.items.filter((item) => item.type === "agentMessage" && item.turn_id === turnId &&
      item.channel === "final").map((item) => item.text).join("\n") };
}

async function exchangeTrace(DatabaseSync, cli, home, cwd, agent, interactionId, attached) {
  const capability = await command(cli, home, cwd, ["delivery", "capabilities", agent.session_id]);
  agent.last_native_negotiated = capability.native_negotiated;
  let trace;
  if (capability.binding?.provider_session_id) {
    agent.observed_binding = assertBinding(capability, agent, agent.attachment_before?.binding ?? agent.observed_identity, attached);
    trace = await visibleTrace(DatabaseSync, agent.registration.codex_home, agent.observed_binding, agent.workspace, agent.expected_version);
  } else if (!attached) {
    trace = await historicalTrace(DatabaseSync, home, agent, interactionId);
  }
  if (trace) {
    const identity = { provider_session_id: trace.provider_thread_id, generation: trace.generation };
    if (agent.observed_identity) assert.deepEqual(identity, agent.observed_identity, "Exchange changed provider identity/generation");
    agent.observed_identity = identity;
    agent.compact_home ??= await compactHomeEvidence(home, agent.session_id);
  }
  return trace;
}

export function canonicalProof(DatabaseSync, home, sender, receiver, taskId, expectedTasks = 1) {
  const db = new DatabaseSync(path.join(home, "state.db"), { readOnly: true });
  try {
    const tasks = db.prepare("SELECT id,target_session_ids,body_ref,status FROM interactions WHERE kind='task' AND sender_session_id=?").all(sender);
    assert.equal(tasks.length, expectedTasks, "Only explicitly submitted tasks are allowed; no uncertain replay");
    const task = tasks.find((entry) => entry.id === taskId);
    assert.ok(task, "The exact admitted task must exist");
    assert.deepEqual(JSON.parse(task.target_session_ids), [receiver]);
    const reply = db.prepare("SELECT id,sender_session_id,target_session_ids,parent_interaction_id,body_ref FROM interactions WHERE kind='reply' AND parent_interaction_id=?").all(taskId);
    const body = (value) => {
      const ref = JSON.parse(value);
      assert.equal(ref.storage, "inline", "Small fixture text should retain its exact inline canonical body");
      return ref.body;
    };
    if (!reply.length) return { request_id: task.id, task_state: task.status, task_body: body(task.body_ref), reply_id: null };
    assert.equal(reply.length, 1);
    assert.equal(reply[0].sender_session_id, receiver);
    assert.deepEqual(JSON.parse(reply[0].target_session_ids), [sender]);
    return { request_id: task.id, task_state: task.status, task_body: body(task.body_ref),
      reply_id: reply[0].id, parent_interaction_id: reply[0].parent_interaction_id, reply_body: body(reply[0].body_ref) };
  } finally { db.close(); }
}

function hostBody(context) {
  if (typeof context === "string") {
    try { context = JSON.parse(context); } catch { return context; }
  }
  // The native envelope uses body; the canonical AgentMessage projection uses
  // message. Parse JSON before comparing literal text containing newlines.
  return context?.body ?? context?.message;
}

function hostContext(delivery) {
  return typeof delivery.context === "string" ? JSON.parse(delivery.context) : delivery.context;
}

function assertBinding(capability, agent, expected, requireNegotiated = true) {
  assert.equal(capability.target_agent_id, agent.session_id);
  // Explicit background owners can exit before the next poll. Their retained
  // binding plus exact owned completed trace remains evidence, not a live lease.
  if (requireNegotiated) assert.equal(capability.native_negotiated, true);
  assert.equal(capability.capabilities.provider, "codex");
  const binding = capability.binding;
  assert.equal(binding.capabilities.protocol_version, agent.expected_version, "Owner CLI differs from the exact test pin");
  assert.equal(binding.provider, "codex");
  assert.equal(binding.target_agent_id, agent.session_id);
  assert.equal(binding.transport, "codex_app_server_ws");
  assert.ok(binding.provider_session_id);
  assert.ok(Number.isSafeInteger(binding.generation));
  if (expected) {
    assert.equal(binding.provider_session_id, expected.provider_session_id, "TUI and messaging must retain one provider thread");
    assert.equal(binding.generation, expected.generation, "Messaging replaced the attached owner");
  }
  return binding;
}

// Read-only attachment evidence: no terminal input, menu acceptance, or model
// turns. Runtime and provider generations are separate counters, not equated.
async function attachmentEvidence(driver, cli, home, cwd, agent, expected, marker) {
  const capability = await command(cli, home, cwd, ["delivery", "capabilities", agent.session_id]);
  const binding = assertBinding(capability, agent, expected?.binding);
  const config = (await invokeTauri(driver, "list_agents")).find((value) => value.session_id === agent.session_id);
  assert.equal(config?.is_off, false);
  assert.equal(config.resume_session, binding.provider_session_id, "Original TUI must resume the shared owner's exact thread");
  assert.equal(config.model, MODEL);
  const snapshot = await invokeTauri(driver, "request_terminal_snapshot", { request: { session_id: agent.session_id } });
  assert.equal(snapshot.session_id, agent.session_id);
  assert.ok(snapshot.runtime_generation > 0);
  if (expected) assert.equal(snapshot.runtime_generation, expected.runtime_generation, "Attached terminal was replaced");
  const grid = snapshot.visible_grid;
  assert.equal(typeof grid, "string");
  return { binding, runtime_generation: snapshot.runtime_generation, sequence_barrier: snapshot.sequence_barrier,
    visible_sha256: createHash("sha256").update(grid).digest("hex"),
    codex_visible: /Codex/iu.test(grid), model_visible: grid.includes(MODEL),
    composer_visible: /›/u.test(grid), marker_visible: marker ? grid.includes(marker) : false };
}

async function terminalDomEvidence(driver, agentId, resumeKey = null) {
  return driver.executeScript((agentId, resumeKey) => {
    const cards = [...document.querySelectorAll("[data-agent-grid-card-id]")]
      .filter((card) => card.getAttribute("data-agent-grid-card-id") === agentId);
    const card = cards.length === 1 ? cards[0] : null;
    const hosts = card ? [...card.querySelectorAll('[data-testid="agent-terminal-host"]')] : [];
    const measure = (element) => {
      if (!element) return { present: false, visible: false, rect: null };
      const box = element.getBoundingClientRect();
      let styledVisible = true;
      for (let ancestor = element; ancestor; ancestor = ancestor.parentElement) {
        const style = window.getComputedStyle(ancestor);
        if (style.display === "none" || ["hidden", "collapse"].includes(style.visibility) || Number(style.opacity) === 0) styledVisible = false;
      }
      const style = window.getComputedStyle(element);
      return { present: true, visible: element.isConnected && styledVisible && box.width > 0 && box.height > 0 &&
        box.bottom > 0 && box.right > 0 && box.top < window.innerHeight && box.left < window.innerWidth,
      rect: { x: box.x, y: box.y, width: box.width, height: box.height },
      style: { display: style.display, visibility: style.visibility, opacity: style.opacity } };
    };
    return { session_id: agentId, resume_status: window.__WARDIAN_MESSAGING_RESUME_ATTEMPTS__?.[resumeKey]?.status ?? null,
      card_count: cards.length, terminal_host_count: hosts.length,
      card: measure(card), terminal_host: measure(hosts.length === 1 ? hosts[0] : null) };
  }, agentId, resumeKey);
}

async function captureStartupScreenshot(driver, repoRoot, runId, agentId) {
  for (const value of [runId, agentId]) assert.match(value, /^[a-z0-9_-]{1,96}$/iu);
  const root = await fs.realpath(repoRoot);
  let directory = root;
  for (const segment of ["e2e", "screenshots", "codex-startup", runId]) {
    directory = within(root, path.join(directory, segment));
    try { await fs.mkdir(directory); } catch (error) { if (error.code !== "EEXIST") throw error; }
    const stat = await fs.lstat(directory);
    assert.ok(stat.isDirectory() && !stat.isSymbolicLink(), "Screenshot directory must not be linked");
    within(root, await fs.realpath(directory));
  }
  const file = path.join(directory, `${agentId}-starting.png`);
  const bytes = Buffer.from(await driver.takeScreenshot(), "base64");
  assert.ok(bytes.length <= 20 * 1024 * 1024, "Startup screenshot exceeds its size bound");
  assert.equal(bytes.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", "WebDriver did not return a PNG");
  await fs.writeFile(file, bytes, { flag: "wx" });
  return { path: file, sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length,
    captured_at: new Date().toISOString(), publication: "private_pending_parent_review" };
}

async function resumeAttached(driver, cli, home, cwd, agent, runId, save,
  screenshots = process.env.WARDIAN_E2E_MESSAGING_V2_SCREENSHOTS === "1") {
  const key = JSON.stringify([runId, agent.session_id]);
  const diagnostics = agent.attachment_startup = { run_id: runId, session_id: agent.session_id,
    window_state_key: key, started_at: new Date().toISOString(), status: "starting", snapshot_samples: 0,
    pending_terminal_visible: false, screenshots_enabled: screenshots };
  let deadline = Date.now() + 200_000; // Capture the owner's bounded 180s attachment failure.
  let resumed = false;
  let lastSave = 0;
  let last;
  try {
    await save();
    const started = await driver.executeScript((key, sessionId) => {
      const attempts = window.__WARDIAN_MESSAGING_RESUME_ATTEMPTS__ ??= Object.create(null);
      if (Object.hasOwn(attempts, key)) return false;
      const state = attempts[key] = { status: "pending" };
      const failed = (error) => {
        state.status = "rejected";
        state.error = String(error?.message ?? error).slice(0, 4096);
      };
      try {
        window.__TAURI_INTERNALS__.invoke("resume_agent", { sessionId }).then(
          () => { state.status = "fulfilled"; }, failed,
        );
      } catch (error) { failed(error); }
      return true; // Never return the pending promise to WebDriver.
    }, key, agent.session_id);
    assert.equal(started, true, "Resume already initiated for this run/agent; do not replay");
    while (Date.now() < deadline) {
      diagnostics.resume_result = await driver.executeScript((key) =>
        window.__WARDIAN_MESSAGING_RESUME_ATTEMPTS__?.[key] ?? null, key);
      assert.ok(diagnostics.resume_result, "Resume promise state disappeared; do not replay");
      diagnostics.observed_at = new Date().toISOString();
      try {
        const snapshot = await invokeTauri(driver, "request_terminal_snapshot", { request: { session_id: agent.session_id } });
        assert.equal(snapshot.session_id, agent.session_id);
        assert.equal(typeof snapshot.visible_grid, "string");
        diagnostics.snapshot_samples += 1;
        diagnostics.latest_snapshot = { observed_at: diagnostics.observed_at, session_id: snapshot.session_id,
          runtime_generation: snapshot.runtime_generation, sequence_barrier: snapshot.sequence_barrier,
          visible_grid: snapshot.visible_grid.slice(-16_384), visible_grid_truncated: snapshot.visible_grid.length > 16_384 };
      } catch (error) { diagnostics.snapshot_error = String(error.message).slice(0, 4096); }
      diagnostics.latest_dom = null;
      try {
        diagnostics.latest_dom = { observed_at: diagnostics.observed_at, ...await terminalDomEvidence(driver, agent.session_id, key) };
        if (diagnostics.latest_dom.resume_status === "pending" && diagnostics.latest_snapshot?.runtime_generation > 0) {
          diagnostics.pending_dom = diagnostics.latest_dom;
          if (diagnostics.latest_dom.card.visible && diagnostics.latest_dom.terminal_host.visible) {
            diagnostics.pending_terminal_visible = true;
            if (screenshots && !diagnostics.screenshot_attempted) {
              diagnostics.screenshot_attempted = true; // At most once, including capture/write failures.
              await save();
              try { diagnostics.starting_screenshot = await captureStartupScreenshot(driver, cwd, runId, agent.session_id); }
              catch (error) { diagnostics.screenshot_error = String(error.message).slice(0, 4096); }
              await save();
            }
          }
        }
      } catch (error) { diagnostics.dom_error = String(error.message).slice(0, 4096); }
      const metrics = (await invokeTauri(driver, "list_agent_metrics")).find((value) => value.session_id === agent.session_id);
      diagnostics.current_status = metrics?.current_status ?? null;
      if (diagnostics.resume_result.status === "rejected") throw new Error(`resume_agent failed: ${diagnostics.resume_result.error}`);
      if (diagnostics.resume_result.status === "fulfilled") {
        if (!resumed) { resumed = true; deadline = Date.now() + 90_000; }
        assert.ok(!/action.*required|error/iu.test(metrics?.current_status ?? ""), "TUI requires action; do not select a model or approve a modal automatically");
        last = await attachmentEvidence(driver, cli, home, cwd, agent);
        if (metrics?.current_status === "Idle" && last.codex_visible && last.model_visible && last.composer_visible &&
          diagnostics.latest_dom?.card.visible && diagnostics.latest_dom.terminal_host.visible) {
          assert.equal(diagnostics.latest_dom.session_id, agent.session_id);
          diagnostics.status = "ready";
          await save();
          return last;
        }
      }
      if (Date.now() - lastSave >= 2000) { await save(); lastSave = Date.now(); }
      await delay(500); // Observe only; no terminal input or resume retry.
    }
    throw new Error(`Attached Codex ${resumed ? "readiness" : "resume"} timed out without input: ${JSON.stringify(last)}`);
  } catch (error) {
    diagnostics.status = "failed";
    diagnostics.error = String(error.message).slice(0, 4096);
    await save();
    throw error;
  }
}

/** Read-only optional-case proof for coordinator-owned explicit idle information.
 * Caller captures same-thread traces around one send, after observing idle.
 * This helper neither sends nor polls and proves only that observed interval.
 */
export function proveIdleInformation(before, after, receipt, senderId, receiverId, body) {
  assert.equal(before.provider_thread_id, after.provider_thread_id);
  assert.equal(before.generation, after.generation);
  assert.ok(before.turns.every((turn) => ["completed", "interrupted", "failed"].includes(turn.status)));
  assert.deepEqual(after.turns, before.turns, "Information started or changed a provider turn");
  assert.ok(!before.host_deliveries.some((row) => row.message_id === receipt.interaction_id));
  const delivered = after.host_deliveries.filter((row) => row.message_id === receipt.interaction_id && row.name === "wardian_inbox_delivery");
  assert.equal(delivered.length, 1);
  const frame = hostContext(delivered[0]);
  assert.equal(frame.schema_version, 1);
  assert.equal(frame.interaction_id, receipt.interaction_id);
  assert.equal(frame.kind, "message");
  assert.equal(frame.sender, senderId);
  assert.equal(frame.recipient, receiverId);
  assert.equal(frame.body, body);
  assert.equal(frame.request_id, null);
  return { status: "pass", interaction_id: receipt.interaction_id, provider_session_id: after.provider_thread_id, scope: "observed_interval" };
}

/** Exact active-turn interruption proof, never a natural-completion race.
 * Coordinator owns a separate active stimulus and one interrupt admission.
 */
export function proveInterruptedTurn(before, after, receipt, agentId, turnId) {
  assert.equal(before.provider_thread_id, after.provider_thread_id);
  assert.equal(before.generation, after.generation);
  assert.ok(before.turns.some((turn) => turn.turn_id === turnId && turn.status === "inProgress"));
  assert.equal(receipt.target_agent_id, agentId);
  assert.equal(receipt.provider_session_id, before.provider_thread_id);
  assert.equal(receipt.generation, before.generation);
  assert.equal(receipt.provider_turn_id, turnId);
  assert.ok(["interrupt_requested", "interrupted"].includes(receipt.delivery_state));
  assert.equal(receipt.interruption_confirmed, receipt.delivery_state === "interrupted");
  assert.ok(after.turns.some((turn) => turn.turn_id === turnId && turn.status === "interrupted"));
  return { status: "pass", provider_session_id: after.provider_thread_id, provider_turn_id: turnId,
    receipt_state: receipt.delivery_state, confirmation_source: "matching_provider_completion" };
}

/** Explicit harness-authored lifecycle stimuli after the model-authored exchange.
 * Every operation is submitted once; observation timeout never causes replay.
 */
async function lifecycleCases(DatabaseSync, session, cli, home, cwd, sender, receiver, report, save) {
  const client = startStdioRpc(cli, ["mcp", "serve"], {
    cwd, env: { ...process.env, WARDIAN_HOME: home, WARDIAN_SESSION_ID: sender.session_id },
  });
  const trace = () => visibleTrace(DatabaseSync, receiver.registration.codex_home, receiver.observed_binding, receiver.workspace);
  const call = async (name, args) => {
    const result = await client.request("tools/call", { name, arguments: args }, 70_000);
    assert.notEqual(result.isError, true, `Lifecycle ${name} failed; it will not be replayed`);
    return result.structuredContent ?? JSON.parse(result.content.find((item) => item.type === "text").text);
  };
  try {
    await client.request("initialize", { protocolVersion: "2025-11-25", capabilities: {},
      clientInfo: { name: "wardian-v2-lifecycle", version: "1" } });
    client.notify("notifications/initialized");
    const idleDeadline = Date.now() + 15_000;
    let idle = false;
    do {
      const metrics = await invokeTauri(session.driver, "list_agent_metrics");
      idle = metrics.find((agent) => agent.session_id === receiver.session_id)?.current_status === "Idle";
      if (idle) break;
      await delay(200);
    } while (Date.now() < idleDeadline);
    assert.ok(idle, "Receiver must be observed idle before the non-waking information case");
    const beforeInfo = await trace();
    const body = `Information only. Preserve this literal text; do not start work.\n${randomBytes(12).toString("hex")} ✓\n`;
    report.optional_cases.idle_information = { status: "running", submission_attempts: 1 };
    await save();
    const sent = await call("send_message", { target: receiver.session_id, message: body });
    let afterInfo;
    const infoDeadline = Date.now() + 15_000;
    do {
      afterInfo = await trace();
      if (afterInfo.host_deliveries.some((item) => item.message_id === sent.interaction_id)) break;
      await delay(200);
    } while (Date.now() < infoDeadline);
    // Extend the observed interval after persisted delivery, without more input.
    await delay(1000);
    afterInfo = await trace();
    report.optional_cases.idle_information = { ...proveIdleInformation(beforeInfo, afterInfo, sent,
      sender.session_id, receiver.session_id, body), receipt: sent, before: beforeInfo, after: afterInfo };
    await save();

    const priorTurns = new Set(afterInfo.turns.map((turn) => turn.turn_id));
    report.optional_cases.active_interrupt = { status: "running", task_submission_attempts: 1, interrupt_attempts: 0 };
    await save();
    const task = await call("followup_task", { target: receiver.session_id,
      message: "Use your shell tool to sleep for thirty seconds. After the sleep completes, reply to this request with the word FINISHED." });
    report.optional_cases.active_interrupt.task = task;
    await save();
    let active;
    let turnId;
    const activeDeadline = Date.now() + 30_000;
    do {
      active = await trace();
      turnId = active.turns.find((turn) => !priorTurns.has(turn.turn_id) && turn.status === "inProgress")?.turn_id;
      if (turnId && active.host_deliveries.some((delivery) => delivery.name === "wardian_task_delivery" &&
        delivery.message_id === task.request_id && hostContext(delivery).recipient === receiver.session_id)) break;
      await delay(100);
    } while (Date.now() < activeDeadline);
    assert.ok(turnId, "Follow-up did not produce an observable active turn; no interrupt was submitted");
    assert.ok(active.host_deliveries.some((delivery) => delivery.name === "wardian_task_delivery" &&
      delivery.message_id === task.request_id && hostContext(delivery).recipient === receiver.session_id),
    "Active receiver history must contain this exact follow-up task before interruption");
    report.optional_cases.active_interrupt = { ...report.optional_cases.active_interrupt, task, turn_id: turnId, interrupt_attempts: 1 };
    await save();
    const interrupted = await call("interrupt_agent", { target: receiver.session_id });
    let afterInterrupt;
    const interruptDeadline = Date.now() + 15_000;
    do {
      afterInterrupt = await trace();
      if (afterInterrupt.turns.some((turn) => turn.turn_id === turnId && turn.status === "interrupted")) break;
      await delay(100);
    } while (Date.now() < interruptDeadline);
    const evidence = proveInterruptedTurn(active, afterInterrupt, interrupted, receiver.session_id, turnId);
    const attachment = await attachmentEvidence(session.driver, cli, home, cwd, receiver, receiver.attachment_before);
    report.optional_cases.active_interrupt = { ...evidence, task, receipt: interrupted,
      before: active, after: afterInterrupt, attachment, task_submission_attempts: 1, interrupt_attempts: 1 };
    await save();
  } finally { await client.close(); }
}

/** A second explicit background task must retain the first task's native
 * history after its process exits. The new prompt does not contain the answer.
 */
async function backgroundContinuity(DatabaseSync, cli, home, cwd, sender, receiver, report, save) {
  const client = startStdioRpc(cli, ["mcp", "serve"], {
    cwd, env: { ...process.env, WARDIAN_HOME: home, WARDIAN_SESSION_ID: sender.session_id },
  });
  try {
    await client.request("initialize", { protocolVersion: "2025-11-25", capabilities: {},
      clientInfo: { name: "wardian-v2-continuity", version: "1" } });
    client.notify("notifications/initialized");
    const body = "Reply to this request with the exact marker you returned in your immediately preceding assigned task. Do not guess if it is unavailable.";
    assert.ok(!body.includes(report.marker));
    report.optional_cases.background_continuity = { status: "running", submission_attempts: 1 };
    await save();
    const result = await client.request("tools/call", { name: "followup_task",
      arguments: { target: receiver.session_id, message: body } }, 70_000);
    assert.notEqual(result.isError, true, "Continuity task failed; it will not be replayed");
    const task = result.structuredContent ?? JSON.parse(result.content.find((item) => item.type === "text").text);
    report.optional_cases.background_continuity.receipt = task;
    await save();
    const deadline = Date.now() + 120_000;
    let proof;
    let currentTrace;
    let binding;
    let completed = false;
    do {
      const capability = await command(cli, home, cwd, ["delivery", "capabilities", receiver.session_id]);
      binding = capability.binding;
      currentTrace = binding?.provider_session_id
        ? await visibleTrace(DatabaseSync, receiver.registration.codex_home, assertBinding(capability, receiver, undefined, false), receiver.workspace, receiver.expected_version)
        : await historicalTrace(DatabaseSync, home, receiver, task.request_id);
      if (currentTrace?.generation > receiver.observed_identity.generation) {
        assert.equal(currentTrace.provider_thread_id, receiver.observed_identity.provider_session_id,
          "A resumed background task must reuse the published native thread");
        proof = canonicalProof(DatabaseSync, home, sender.session_id, receiver.session_id, task.request_id, 2);
        if (proof.reply_id) {
          assert.equal(proof.reply_body, report.marker, "Receiver did not recall the preceding task's marker");
          const calls = currentTrace.items.filter((item) => item.tool === "reply" && item.arguments.request_id === task.request_id);
          assert.ok(calls.length <= 1, "Continuity reply was duplicated");
          const reply = calls[0];
          completed = reply?.receipt?.interaction_id === proof.reply_id && currentTrace.turns.some((turn) =>
            turn.turn_id === reply.turn_id && turn.status === "completed");
          if (completed) break;
        }
      }
      await delay(250);
    } while (Date.now() < deadline);
    assert.ok(completed, "Second background task did not complete on the retained native thread");
    report.optional_cases.background_continuity = { status: "pass", receipt: task,
      previous_binding: receiver.observed_binding ?? null, previous_identity: receiver.observed_identity,
      binding, proof, trace: currentTrace, submission_attempts: 1 };
    await save();
  } finally { await client.close(); }
}

export function proveExchange(senderTrace, receiverTrace, proof, marker, senderId, receiverId) {
  const calls = senderTrace.items.filter((item) => item.tool === "followup_task");
  assert.equal(calls.length, 1, "Sender must actually call followup_task once");
  const task = calls[0];
  if (!task.receipt || !proof?.reply_id) return false;
  assert.equal(task.receipt.request_id, proof.request_id);
  assert.equal(task.arguments.message, proof.task_body);
  // The shared owner starts work with real standalone turn/start.toolOutput,
  // not a user-message echo or a fabricated model-authored tool call.
  const providerReceivedTask = receiverTrace.host_deliveries.some((delivery) => delivery.frame_type === "canonical_task" &&
    delivery.message_id === proof.request_id && hostBody(delivery.context) === proof.task_body &&
    hostContext(delivery).sender === senderId && hostContext(delivery).recipient === receiverId &&
    hostContext(delivery).kind === "task" && hostContext(delivery).request_id === proof.request_id) ||
    receiverTrace.items.some((item) => item.tool === "receive_messages" && item.receipt?.messages?.some((message) => message.kind === "task" &&
      message.interaction_id === proof.request_id && message.sender === senderId && message.message === proof.task_body));
  assert.ok(providerReceivedTask, "Receiver trace must contain the actual owned task, not just a matching final marker");
  assert.equal(proof.reply_body, marker);
  assert.equal(proof.task_state, "completed");
  const replies = receiverTrace.items.filter((item) => item.tool === "reply");
  if (!replies.length) return false;
  assert.equal(replies.length, 1, "Receiver must complete the request with one model-authored MCP reply");
  const reply = replies[0];
  if (!reply.receipt) return false;
  assert.equal(reply.arguments.request_id, proof.request_id);
  assert.equal(reply.arguments.status, "done");
  assert.equal(reply.arguments.message, marker);
  assert.equal(reply.receipt.interaction_id, proof.reply_id);
  const received = senderTrace.items.filter((item) => item.tool === "receive_messages" && item.receipt)
    .flatMap((item) => item.receipt.messages ?? []).filter((message) => message.interaction_id === proof.reply_id);
  // Native push is an alternative receiver boundary, but only actual typed
  // provider history qualifies. An RPC acceptance receipt alone never does.
  for (const delivery of senderTrace.host_deliveries) {
    if (delivery.name !== "wardian_inbox_delivery" || delivery.message_id !== proof.reply_id) continue;
    let context = delivery.context;
    if (typeof context === "string") {
      try { context = JSON.parse(context); } catch { continue; }
    }
    if (context.interaction_id === proof.reply_id) {
      assert.equal(context.schema_version, 1);
      assert.equal(context.recipient, senderId);
      assert.equal(context.request_id, proof.request_id);
      received.push(context);
    }
  }
  if (!received.length) return false;
  for (const message of received) {
    assert.equal(message.kind, "reply");
    assert.equal(message.sender, receiverId);
    assert.equal(message.parent_interaction_id, proof.request_id);
    assert.equal(message.reply_status, "done");
    assert.equal(hostBody(message), marker);
  }
  assert.notEqual(senderId, receiverId);
  for (const [trace, requiredTurn] of [[senderTrace, task.turn_id], [receiverTrace, reply.turn_id]]) {
    const context = trace.contexts.find((entry) => entry.turn_id === requiredTurn);
    assert.ok(context, "Actual model/effort turn context is required");
    assert.equal(context.model, MODEL, "No substituted or migration-selected model is accepted");
    // Some 0.153.4 rollouts omit effort even though the native configuration
    // response reports it. Require the actual model and reject any reported
    // substitution; absent effort remains explicitly unverified, never inferred.
    if (context.effort !== null) assert.equal(context.effort, EFFORT);
    assert.equal(path.toNamespacedPath(path.resolve(context.cwd)), path.toNamespacedPath(path.resolve(trace.meta.cwd)));
    if (!trace.turns.some((turn) => turn.turn_id === requiredTurn && turn.status === "completed")) return false;
  }
  return senderTrace.items.some((item) => item.type === "agentMessage" && item.turn_id === task.turn_id && item.text?.includes(marker));
}

test("baseline launchers remain eligible while upgraded evidence requires a native Codex file", () => {
  const nativeHeader = Buffer.from("4d5a9000", "hex");
  for (const [executable, header] of [
    ["node.exe", nativeHeader], ["codex.cmd", Buffer.from("@ECH")],
    ["codex.exe", Buffer.from("#!sh")],
  ]) {
    assert.deepEqual(executableIdentity(executable, header, BASELINE_VERSION, "win32"),
      { identity_kind: "selected_launcher", native_binary_identity: "unresolved_from_readiness" });
    assert.throws(() => executableIdentity(executable, header, TESTED_ALPHA_VERSION, "win32"));
  }
  assert.equal(executableIdentity("codex.exe", nativeHeader, TESTED_ALPHA_VERSION, "win32").identity_kind, "native_codex_file");
  assert.equal(executableIdentity("codex", Buffer.from("#!/u"), BASELINE_VERSION, "darwin").identity_kind, "selected_launcher");
  assert.throws(() => executableIdentity("codex", Buffer.from("#!/u"), TESTED_ALPHA_VERSION, "darwin"));
});

test("upgraded test pins fail closed before any native startup", () => {
  const executable = path.resolve("codex-test-native");
  assert.equal(expectedTestVersion({}), BASELINE_VERSION);
  assert.equal(expectedTestVersion({ WARDIAN_E2E_CODEX_EXPECTED_VERSION: BASELINE_VERSION }), BASELINE_VERSION);
  assert.equal(expectedTestVersion({ WARDIAN_E2E_CODEX_EXPECTED_VERSION: TESTED_ALPHA_VERSION, WARDIAN_E2E_CODEX_EXECUTABLE: executable }), TESTED_ALPHA_VERSION);
  for (const env of [
    { WARDIAN_E2E_CODEX_EXECUTABLE: executable },
    { WARDIAN_E2E_CODEX_EXPECTED_VERSION: TESTED_ALPHA_VERSION },
    { WARDIAN_E2E_CODEX_EXPECTED_VERSION: TESTED_ALPHA_VERSION, WARDIAN_E2E_CODEX_EXECUTABLE: "relative.exe" },
    ...["", " 0.153.4", "0.154.0", "0.153.5", "0.154.0-alpha.5", "0.154.0-alpha.7", "0.154.0-alpha.6+build.1"].map((version) =>
      ({ WARDIAN_E2E_CODEX_EXPECTED_VERSION: version, WARDIAN_E2E_CODEX_EXECUTABLE: executable })),
  ]) assert.throws(() => expectedTestVersion(env));
  assert.equal(assertExecutableVersion("codex-cli 0.154.0-alpha.6", TESTED_ALPHA_VERSION), TESTED_ALPHA_VERSION);
  for (const version of [null, "0.154.0-alpha.6", "codex-cli 0.154.0-alpha.7", "codex-cli 0.153.4"])
    assert.throws(() => assertExecutableVersion(version, TESTED_ALPHA_VERSION));
});

for (const version of [BASELINE_VERSION, TESTED_ALPHA_VERSION]) {
test(`owned Codex ${version} rollout remains authoritative when the history projection is empty`, async (t) => {
  const parent = path.resolve(import.meta.dirname, "../../.tmp");
  await fs.mkdir(parent, { recursive: true });
  const root = await fs.mkdtemp(path.join(parent, "messaging-evidence-"));
  const sessions = path.join(root, "sessions");
  await fs.mkdir(sessions);
  const rollout = path.join(sessions, "rollout-owned-thread.jsonl");
  const database = path.join(root, "thread_history_1.sqlite");
  t.after(async () => {
    for (const file of [rollout, database]) await fs.unlink(within(root, file));
    await fs.rmdir(sessions);
    await fs.rmdir(root);
  });
  const rows = [
    { type: "session_meta", payload: { id: "owned-thread", cwd: root, originator: "codex_cli_rs", cli_version: version } },
    { type: "turn_context", payload: { turn_id: "owned-turn", model: MODEL, effort: EFFORT, cwd: root } },
    { type: "event_msg", payload: { type: "task_started", turn_id: "owned-turn" } },
    { type: "response_item", payload: { type: "function_call", namespace: "mcp__wardian", name: "receive_messages", call_id: "owned-call", arguments: "{}" } },
    { type: "response_item", payload: { type: "function_call_output", call_id: "owned-call", output: JSON.stringify({ operation: "receive_messages", messages: [{ interaction_id: "owned-reply", message: "exact\n  reply " }] }) } },
    { type: "response_item", payload: { type: "message", role: "assistant", channel: "final", content: [{ type: "output_text", text: "exact\n  reply " }] } },
    { type: "event_msg", payload: { type: "task_complete", turn_id: "owned-turn" } },
  ];
  await fs.writeFile(rollout, `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(database);
  db.exec("CREATE TABLE thread_items(thread_id TEXT, turn_id TEXT, item_json TEXT, item_type TEXT, rollout_ordinal INTEGER); CREATE TABLE thread_turns(thread_id TEXT, turn_id TEXT, status TEXT, rollout_ordinal INTEGER)");
  db.close();
  const binding = { provider_session_id: "owned-thread", generation: 3, capabilities: { protocol_version: version } };
  const trace = await visibleTrace(DatabaseSync, root, binding, root, version);
  assert.equal(trace.evidence_source, "owned_provider_rollout");
  assert.equal(trace.items[0].id, "owned-call");
  assert.equal(trace.items[0].receipt.messages[0].interaction_id, "owned-reply");
  assert.equal(trace.items[1].text, "exact\n  reply ");
  assert.deepEqual(trace.turns, [{ turn_id: "owned-turn", status: "completed" }]);
  await assert.rejects(visibleTrace(DatabaseSync, root, binding, path.join(root, "foreign-workspace"), version));
  await assert.rejects(visibleTrace(DatabaseSync, root, { ...binding, provider_session_id: "foreign-thread" }, root, version));
  await assert.rejects(visibleTrace(DatabaseSync, root, { ...binding, capabilities: { protocol_version: "0.154.0-alpha.7" } }, root, version));
  rows[0].payload.cli_version = "0.154.0-alpha.7";
  await fs.writeFile(rollout, `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
  await assert.rejects(visibleTrace(DatabaseSync, root, binding, root, version));
  rows[0].payload.originator = "Wardian";
  delete rows[0].payload.cli_version;
  await fs.writeFile(rollout, `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
  assert.equal((await visibleTrace(DatabaseSync, root, binding, root, version)).meta.originator, "Wardian");
});
}

test("fixture Git roots and runtime trust remain confined to fresh private workspaces", async (t) => {
  const parent = path.resolve(import.meta.dirname, "../../.tmp");
  await fs.mkdir(parent, { recursive: true });
  const home = await fs.mkdtemp(path.join(parent, "messaging-trust-é中-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const inheritedRoot = await fs.realpath(await fixtureGit(home, ["rev-parse", "--show-toplevel"]));
  const workspaces = await prepareFixtureWorkspaces(home);
  for (const value of Object.values(workspaces)) {
    assert.notEqual(value.git_root, inheritedRoot);
    assert.equal(value.git_root, await fs.realpath(value.workspace));
    assert.equal(value.commits, 0);
  }
  await assert.rejects(prepareFixtureWorkspaces(home), { code: "EEXIST" });
  await fs.mkdir(path.join(home, "settings"));
  const original = { schema_version: 2, settings: { shell_id: "auto", codex_runtime_policy: { sandbox_mode: "read-only", approval_policy: "never", full_auto: false, trust_workspaces: false } },
    overrides: { codex_runtime_policy: { approval_policy: "never" } } };
  const { runInNewContext } = await import("node:vm");
  let saved;
  const window = { __TAURI_INTERNALS__: { async invoke(command, args) {
    if (command === "list_agents") return [];
    if (command === "load_shell_settings") return structuredClone(original);
    assert.equal(command, "save_shell_settings");
    saved = structuredClone(args.settings);
    await fs.writeFile(path.join(home, "settings/shell.json"), JSON.stringify({ schema_version: 2, overrides: saved.overrides }));
    return saved;
  } } };
  const driver = { executeAsyncScript(fn, ...args) {
    return new Promise((resolve) => runInNewContext(`(${fn.toString()})(...args, done)`, { window, args, done: resolve }));
  } };
  assert.equal(fixtureTrustEnabled({}), true);
  assert.equal(fixtureTrustEnabled({ WARDIAN_E2E_MESSAGING_V2_TRUST_FIXTURE_WORKSPACES: "0" }), false);
  assert.throws(() => fixtureTrustEnabled({ WARDIAN_E2E_MESSAGING_V2_TRUST_FIXTURE_WORKSPACES: "false" }));
  for (const trusted of [true, false]) {
    const evidence = await configureFixtureTrust(driver, home, workspaces, trusted);
    assert.equal(evidence.mode, trusted ? "trusted" : "untrusted");
    const expected = structuredClone(original);
    expected.settings.codex_runtime_policy.trust_workspaces = trusted;
    expected.overrides.codex_runtime_policy.trust_workspaces = trusted;
    assert.deepEqual(saved, expected, "Trust selection must preserve every other runtime setting");
    for (const { workspace } of Object.values(workspaces)) {
      const agent = { session_id: "fixture", workspace, registration: { codex_home: path.join(home, "private-codex") } };
      const doctor = { applicable: true, agent: { uuid: agent.session_id, provider: "codex", workspace }, codex_home: agent.registration.codex_home,
        launch_flags: ["--no-alt-screen", "-c", "model_reasoning_effort=\"low\"",
          ...(trusted ? ["-c", `projects.${JSON.stringify(workspace)}.trust_level="trusted"`] : [])] };
      const planned = assertFixtureLaunchTrust(doctor, home, agent, trusted);
      assert.equal(planned.projects.length, trusted ? 1 : 0);
      assert.equal(planned.planned_launch_only, true);
      // Actual owner ordering: absent before preparation, present only in the
      // startup overlay, then removed before capable binding. Restore retains
      // the empty project table (launch_config/leaves.rs::remove).
      const baseline = "model = \"fixture\"\n";
      agent.project_trust_before = fixtureProjectTrust(baseline, home, workspace);
      assert.deepEqual(agent.project_trust_before, []);
      const applied = trusted ? `${baseline}[projects.${JSON.stringify(workspace)}]\ntrust_level = "trusted"\n` : baseline;
      assert.deepEqual(fixtureProjectTrust(applied, home, workspace), planned.projects);
      const restored = trusted ? `${baseline}[projects.${JSON.stringify(workspace)}]\n` : baseline;
      assert.deepEqual(assertFixtureTrustRestored(restored, home, agent), []);
      if (trusted) assert.throws(() => assertFixtureTrustRestored(applied, home, agent), /restore the pre-launch/);
      const foreign = `[projects.${JSON.stringify(inheritedRoot)}]\ntrust_level = "trusted"\n`;
      assert.throws(() => fixtureProjectTrust(foreign, home, workspace));
      const foreignDoctor = structuredClone(doctor);
      foreignDoctor.launch_flags.push("-c", `projects.${JSON.stringify(inheritedRoot)}.trust_level="trusted"`);
      assert.throws(() => assertFixtureLaunchTrust(foreignDoctor, home, agent, trusted));
      const changedPolicy = structuredClone(doctor);
      changedPolicy.launch_flags = trusted ? [] : ["-c", `projects.${JSON.stringify(workspace)}.trust_level="trusted"`];
      assert.throws(() => assertFixtureLaunchTrust(changedPolicy, home, agent, trusted));
    }
  }
});

test("attached startup observes a pending resume and saves bounded diagnostics without replay or input", async (t) => {
  const { runInNewContext } = await import("node:vm");
  const screenshotRoot = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-startup-image-"));
  t.after(() => fs.rm(screenshotRoot, { recursive: true, force: true }));
  for (const outcome of ["rejected", "fulfilled"]) {
    const calls = [];
    const saves = [];
    let settle;
    const window = { __TAURI_INTERNALS__: { invoke(command, args) {
      calls.push({ command, args });
      if (command === "resume_agent") return new Promise((resolve, reject) => {
        settle = () => outcome === "rejected" ? reject(new Error("owned TUI attachment timed out")) : resolve();
      });
      if (command === "request_terminal_snapshot") {
        setTimeout(settle, 50); // Keep the first DOM sample pending; settle before the next poll.
        return Promise.resolve({ session_id: args.request.session_id, runtime_generation: 7, sequence_barrier: 42,
          visible_grid: "x".repeat(20_000) + "Trust this directory?" });
      }
      if (command === "list_agent_metrics") return Promise.resolve([{ session_id: "agent", current_status: "Action Required" }]);
      throw new Error(`Unexpected command: ${command}`);
    } } };
    const box = { x: 0, y: 0, top: 0, left: 0, right: 600, bottom: 400, width: 600, height: 400 };
    const card = { isConnected: true, parentElement: null, getAttribute: () => "agent", getBoundingClientRect: () => box };
    const host = { isConnected: true, parentElement: card, getBoundingClientRect: () => box };
    card.querySelectorAll = (selector) => { assert.equal(selector, '[data-testid="agent-terminal-host"]'); return [host]; };
    const document = { querySelectorAll(selector) { assert.equal(selector, "[data-agent-grid-card-id]"); return [card]; } };
    window.innerWidth = 1000;
    window.innerHeight = 800;
    let hidden = true;
    window.getComputedStyle = (element) => ({ display: "block", visibility: element === host && hidden ? "hidden" : "visible", opacity: "1" });
    const driver = {
      async executeScript(fn, ...args) { return structuredClone(runInNewContext(`(${fn.toString()})(...args)`, { window, document, args })); },
      executeAsyncScript(fn, ...args) {
        return new Promise((resolve) => runInNewContext(`(${fn.toString()})(...args, done)`, { window, args, done: resolve }));
      },
    };
    const hiddenEvidence = await terminalDomEvidence(driver, "agent");
    assert.equal(hiddenEvidence.card.visible, true);
    assert.equal(hiddenEvidence.terminal_host.visible, false);
    assert.equal(hiddenEvidence.terminal_host.style.visibility, "hidden");
    assert.equal((await terminalDomEvidence(driver, "foreign-agent")).card.present, false);
    hidden = false;
    let screenshotCalls = 0;
    driver.takeScreenshot = async () => { screenshotCalls += 1; return "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jr1cAAAAASUVORK5CYII="; };
    const agent = { session_id: "agent" };
    const save = async () => { saves.push(structuredClone(agent.attachment_startup)); };
    const screenshots = outcome === "rejected";
    await assert.rejects(resumeAttached(driver, null, null, screenshotRoot, agent, "fixture-run", save, screenshots),
      outcome === "rejected" ? /owned TUI attachment timed out/ : /TUI requires action/);
    assert.equal(calls.filter((call) => call.command === "resume_agent").length, 1);
    assert.ok(saves.some((state) => state.resume_result?.status === "pending" && state.snapshot_samples > 0),
      "A visible snapshot must be saved while the resume promise remains pending");
    const diagnostic = saves.at(-1);
    assert.equal(diagnostic.status, "failed");
    assert.equal(diagnostic.resume_result.status, outcome);
    assert.equal(diagnostic.latest_snapshot.runtime_generation, 7);
    assert.equal(diagnostic.latest_snapshot.sequence_barrier, 42);
    assert.equal(diagnostic.latest_snapshot.visible_grid.length, 16_384);
    assert.equal(diagnostic.latest_snapshot.visible_grid_truncated, true);
    assert.ok(diagnostic.latest_snapshot.visible_grid.endsWith("Trust this directory?"));
    assert.equal(diagnostic.current_status, "Action Required");
    assert.equal(diagnostic.pending_terminal_visible, true);
    assert.equal(diagnostic.pending_dom.session_id, "agent");
    assert.equal(diagnostic.pending_dom.resume_status, "pending");
    assert.equal(diagnostic.pending_dom.terminal_host.rect.width, 600);
    assert.equal(screenshotCalls, screenshots ? 1 : 0);
    if (screenshots) {
      assert.equal(diagnostic.starting_screenshot.path, path.join(await fs.realpath(screenshotRoot), "e2e/screenshots/codex-startup/fixture-run/agent-starting.png"));
      assert.equal(await sha256(diagnostic.starting_screenshot.path), diagnostic.starting_screenshot.sha256);
      assert.equal(diagnostic.starting_screenshot.publication, "private_pending_parent_review");
    }
    assert.deepEqual(Object.keys(window.__WARDIAN_MESSAGING_RESUME_ATTEMPTS__), [JSON.stringify(["fixture-run", "agent"])]);
    await assert.rejects(resumeAttached(driver, null, null, null, { session_id: "agent" }, "fixture-run", async () => {}), /do not replay/);
    assert.equal(calls.filter((call) => call.command === "resume_agent").length, 1);
    assert.equal(screenshotCalls, screenshots ? 1 : 0);
    assert.ok(calls.every((call) => ["resume_agent", "request_terminal_snapshot", "list_agent_metrics"].includes(call.command)));
  }
});

test("nested MCP end events retain canonical receipts and deduplicate direct evidence", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-mcp-events-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  await fs.mkdir(path.join(root, "sessions"));
  const file = path.join(root, "sessions", "rollout-thread.jsonl");
  const args = { target: "receiver", message: "exact task" };
  const receipt = { request_id: "request", delivery_state: "queued" };
  const end = { type: "event_msg", payload: { type: "mcp_tool_call_end", call_id: "nested-call",
    invocation: { server: "wardian", tool: "followup_task", arguments: args }, result: { Ok: { structuredContent: receipt } } } };
  const direct = [
    { type: "response_item", payload: { type: "function_call", namespace: "mcp__wardian", name: "followup_task", call_id: "nested-call", arguments: JSON.stringify(args) } },
    { type: "response_item", payload: { type: "function_call_output", call_id: "nested-call", output: JSON.stringify(receipt) } },
  ];
  const read = async (events) => {
    const rows = [
      { type: "session_meta", payload: { id: "thread", cwd: root, originator: "codex_cli_rs", cli_version: TESTED_ALPHA_VERSION } },
      { type: "event_msg", payload: { type: "task_started", turn_id: "turn" } },
      ...events,
      { type: "event_msg", payload: { type: "task_complete", turn_id: "turn" } },
    ];
    await fs.writeFile(file, rows.map(JSON.stringify).join("\n") + "\n");
    return ownedMetadata(root, "thread", root, TESTED_ALPHA_VERSION);
  };
  for (const events of [[end], [...direct, end], [end, ...direct]]) {
    const trace = await read(events);
    assert.equal(trace.rollout_items.length, 1);
    assert.deepEqual(trace.rollout_items[0].receipt, receipt);
    assert.equal(trace.rollout_items[0].turn_id, "turn");
    assert.equal(trace.rollout_items[0].id, "nested-call");
  }
  const textEnd = structuredClone(end);
  textEnd.payload.result.Ok = { content: [{ type: "text", text: JSON.stringify(receipt) }] };
  assert.deepEqual((await read([textEnd])).rollout_items[0].receipt, receipt);
  for (const result of [{ Err: "MCP unavailable" }, { Ok: { isError: true, structuredContent: receipt } },
    { Ok: { content: [{ type: "text", text: "invalid JSON" }] } }]) {
    const failed = structuredClone(end);
    failed.payload.result = result;
    for (const events of [[failed], [...direct, failed], [failed, ...direct]]) {
      assert.equal((await read(events)).rollout_items[0].status, "failed", "Duplicate success must not erase provider failure");
    }
  }
  const conflict = structuredClone(end);
  conflict.payload.result.Ok.structuredContent.request_id = "foreign-request";
  await assert.rejects(read([...direct, conflict]), /changed its receipt/);
  conflict.payload.invocation.arguments.message = "different task";
  await assert.rejects(read([...direct, conflict]), /changed arguments/);
  const foreign = structuredClone(end);
  foreign.payload.invocation.server = "foreign";
  assert.equal((await read([foreign])).rollout_items.length, 0);
});

test("historical acceptance survives an erased binding and diagnoses only the exact completed turn", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-history-"));
  const { DatabaseSync } = await import("node:sqlite");
  const db = new DatabaseSync(path.join(root, "state.db"));
  t.after(() => { db.close(); return fs.rm(root, { recursive: true, force: true }); });
  for (const dir of ["settings", "sessions"]) await fs.mkdir(path.join(root, dir));
  const agent = { session_id: "sender", expected_version: TESTED_ALPHA_VERSION, workspace: root, registration: { codex_home: root } };
  await fs.writeFile(path.join(root, "settings/state.json"), JSON.stringify([{ session_id: "sender", resume_session: "thread" }]));
  const rows = [
    { type: "session_meta", payload: { id: "thread", cwd: root, originator: "Wardian", cli_version: "0.6.0" } },
    { type: "event_msg", payload: { type: "task_started", turn_id: "accepted-turn" } },
    { type: "response_item", payload: { type: "message", role: "assistant", phase: "final_answer", content: [{ type: "output_text", text: "Tool host unavailable; no task sent." }] } },
    { type: "event_msg", payload: { type: "task_complete", turn_id: "accepted-turn" } },
  ];
  const file = path.join(root, "sessions/rollout-thread.jsonl");
  await fs.writeFile(file, rows.map(JSON.stringify).join("\n") + "\n");
  db.exec("CREATE TABLE native_deliveries(interaction_id TEXT,record_json TEXT); CREATE TABLE native_delivery_evidence(interaction_id TEXT,phase TEXT,evidence_json TEXT)");
  const record = { envelope: { interaction_id: "initial", target_agent_id: "sender", generation: 2 }, provider: "codex", phase: "completed", provider_turn_id: "accepted-turn" };
  const evidence = { interaction_id: "initial", target_agent_id: "sender", generation: 2, provider: "codex", source: "provider_event", provider_turn_id: "accepted-turn" };
  db.prepare("INSERT INTO native_deliveries VALUES(?,?)").run("initial", JSON.stringify(record));
  db.prepare("INSERT INTO native_delivery_evidence VALUES(?,'provider_accepted',?)").run("initial", JSON.stringify(evidence));
  const trace = await historicalTrace(DatabaseSync, root, agent, "initial");
  assert.equal(trace.provider_thread_id, "thread");
  assert.equal(trace.generation, 2);
  assert.equal(trace.protocol_version_evidence, "bootstrap_header_only_not_negotiation");
  assert.equal(trace.capabilities, undefined, "Historical recovery must not fabricate a native binding");
  assert.equal(terminalExchangeFailure(trace, "accepted-turn", "followup_task").final_message, "Tool host unavailable; no task sent.");
  assert.equal(terminalExchangeFailure(trace, "foreign-turn", "followup_task"), null);
  assert.equal(terminalExchangeFailure({ ...trace, turns: [{ turn_id: "accepted-turn", status: "inProgress" }] }, "accepted-turn", "followup_task"), null);
  assert.equal(terminalExchangeFailure({ ...trace, items: [{ turn_id: "accepted-turn", tool: "followup_task" }] }, "accepted-turn", "followup_task"), null);
  db.prepare("UPDATE native_delivery_evidence SET evidence_json=?").run(JSON.stringify({ ...evidence, generation: 3 }));
  await assert.rejects(historicalTrace(DatabaseSync, root, agent, "initial"));
  db.prepare("UPDATE native_delivery_evidence SET evidence_json=?").run(JSON.stringify(evidence));
  await assert.rejects(historicalTrace(DatabaseSync, root, { ...agent, session_id: "foreign" }, "initial"));
  rows[1].payload.turn_id = rows[3].payload.turn_id = "different-turn";
  await fs.writeFile(file, rows.map(JSON.stringify).join("\n") + "\n");
  assert.equal(await historicalTrace(DatabaseSync, root, agent, "initial"), null, "Unrelated history cannot prove this delivery completed");
  // Peer background tasks retain their claim outcome, not a native envelope.
  // The canonical host item must identify the same request/body and turn.
  db.exec("CREATE TABLE agent_message_delivery(interaction_id TEXT,recipient TEXT,sender TEXT,generation INTEGER,owner TEXT,operation TEXT); CREATE TABLE interactions(id TEXT,body_ref TEXT)");
  db.prepare("INSERT INTO agent_message_delivery VALUES('peer-task','sender','requester',3,'provider_completed','followup_task')").run();
  db.prepare("INSERT INTO interactions VALUES('peer-task',?)").run(JSON.stringify({ storage: "inline", body: "owned task" }));
  const context = { schema_version: 1, interaction_id: "peer-task", request_id: "peer-task", kind: "task",
    sender: "requester", recipient: "sender", body: "owned task", parent_interaction_id: null, reply_status: null };
  rows.splice(2, 0, { type: "response_item", payload: { type: "function_call_output", namespace: "wardian",
    name: "wardian_task_delivery", output: JSON.stringify(context) } });
  await fs.writeFile(file, rows.map(JSON.stringify).join("\n") + "\n");
  const taskTrace = await historicalTrace(DatabaseSync, root, agent, "peer-task");
  assert.equal(taskTrace.generation, 3);
  assert.equal(taskTrace.accepted_turn_id, "different-turn");
  assert.equal(taskTrace.identity_source, "persisted_resume_and_task_acceptance");
  db.prepare("UPDATE agent_message_delivery SET owner='dispatching'").run();
  assert.equal(await historicalTrace(DatabaseSync, root, agent, "peer-task"), null, "A claim is not provider acceptance");
  db.prepare("UPDATE agent_message_delivery SET owner='provider_completed'").run();
  context.body = "foreign body";
  rows[2].payload.output = JSON.stringify(context);
  await fs.writeFile(file, rows.map(JSON.stringify).join("\n") + "\n");
  await assert.rejects(historicalTrace(DatabaseSync, root, agent, "peer-task"));
});

async function compactCredentialFixture(t, { copied = true, staging = false } = {}) {
  const tempParent = await fs.realpath(os.tmpdir());
  const root = await fs.mkdtemp(path.join(tempParent, "wc-"));
  t.after(async () => {
    within(tempParent, root);
    assert.equal(await fs.realpath(root), root, "Owned fixture root changed before test teardown");
    await fs.rm(root, { recursive: true }); // Only this test's fresh temporary tree.
  });
  const home = path.join(root, "deep-isolated-wardian-home-for-socket-migration".repeat(3));
  const agentId = randomUUID();
  const token = randomUUID();
  const agent = path.join(home, "agents", agentId);
  const source = path.join(agent, "habitat", ".codex");
  const slot = path.join(root, "p", "00112233");
  const target = path.join(slot, "h");
  const transfer = path.join(slot, "c");
  const backup = path.join(agent, "habitat", `.codex-precompact-${token}`);
  const fixtureCodex = path.join(home, "fixture-profile", ".codex");
  await fs.mkdir(source, { recursive: true, mode: 0o700 });
  await fs.mkdir(slot, { recursive: true, mode: 0o700 });
  await fs.mkdir(fixtureCodex, { recursive: true, mode: 0o700 });
  const credential = Buffer.from("synthetic-test-credential-only");
  await fs.writeFile(path.join(fixtureCodex, "auth.json"), credential);
  await fs.writeFile(path.join(source, "auth.json"), credential);
  const sourceStat = await fs.stat(source, { bigint: true });
  if (copied) {
    await fs.mkdir(staging ? transfer : target, { mode: 0o700 });
    await fs.copyFile(path.join(source, "auth.json"), path.join(staging ? transfer : target, "auth.json"));
    if (!staging) await fs.rename(source, backup);
  } else {
    assert.equal(staging, false);
    await fs.rename(source, target);
  }
  if (!staging) await fs.symlink(target, source, process.platform === "win32" ? "junction" : "dir");
  const targetStat = await fs.stat(staging ? transfer : target, { bigint: true });
  const intent = { version: 1, token, agent_id: agentId, wardian_home: home, source, target,
    source_identity: [sourceStat.dev, sourceStat.ino], snapshot: "a".repeat(64) };
  const ready = { version: 1, token, target_identity: [targetStat.dev, targetStat.ino], copied };
  const agentRecord = path.join(agent, ".wardian-codex-home.json");
  const slotRecord = path.join(slot, ".wardian-codex-home.json");
  const readyRecord = path.join(slot, ".wardian-codex-home-ready.json");
  const write = (file, value) => fs.writeFile(file, JSON.stringify(value,
    (_key, item) => typeof item === "bigint" ? JSON.rawJSON(String(item)) : item));
  await write(agentRecord, intent);
  await write(slotRecord, intent);
  await write(readyRecord, ready);
  return { root, home, agentId, source, slot, target, transfer, backup, fixtureCodex, intent, ready,
    agentRecord, slotRecord, readyRecord, write, authHash: createHash("sha256").update(credential).digest("hex") };
}

test("compact credential cleanup authenticates copied target, original backup, and fixture", async (t) => {
  const f = await compactCredentialFixture(t);
  // This fixture models copy publication; it does not claim an actual volume crossing.
  const socketBytes = Buffer.byteLength(path.join(f.target, "app-server-control", "app-server-control.sock"));
  const capacity = process.platform === "darwin" ? 104 : 108;
  if (socketBytes < capacity) {
    const evidence = await compactHomeEvidence(f.home, f.agentId);
    assert.equal(evidence.socket_bytes, socketBytes);
    assert.equal(evidence.physical_home, await fs.realpath(f.source));
    assert.equal(evidence.token, f.intent.token);
    assert.deepEqual(evidence.source_identity, f.intent.source_identity.map(String));
    assert.ok(evidence.records.every((record) => /^[0-9a-f]{64}$/u.test(record.sha256)));
  } else {
    await assert.rejects(compactHomeEvidence(f.home, f.agentId), /socket path exceeds/u);
  }
  const cleaned = await cleanupAgentCredentials(f.home, f.agentId);
  assert.deepEqual(new Set(cleaned.credential_copies_removed), new Set([f.target, f.backup].map((directory) => path.join(directory, "auth.json"))));
  await cleanupFixtureCredential(f.home, f.fixtureCodex);
  for (const directory of [f.target, f.backup, f.fixtureCodex]) {
    await assert.rejects(fs.lstat(path.join(directory, "auth.json")), { code: "ENOENT" });
    assert.ok((await fs.lstat(directory)).isDirectory());
  }
  assert.ok((await fs.lstat(f.agentRecord)).isFile());
  assert.ok((await fs.lstat(f.source)).isSymbolicLink());
});

test("compact credential cleanup handles verified staging and same-volume rename", async (t) => {
  for (const options of [{ copied: true, staging: true }, { copied: false }]) {
    const f = await compactCredentialFixture(t, options);
    const cleaned = await cleanupAgentCredentials(f.home, f.agentId);
    const expected = options.staging ? [f.transfer, f.source] : [f.target];
    assert.deepEqual(new Set(cleaned.credential_copies_removed), new Set(expected.map((directory) => path.join(directory, "auth.json"))));
    if (options.staging) await assert.rejects(compactHomeEvidence(f.home, f.agentId), /did not complete/u);
  }
});

test("foreign compact records and changed identities retain every credential copy", async (t) => {
  for (const damage of ["slot-token", "agent-id", "wardian-home", "source", "ready-token", "source-identity", "target-identity"]) {
    const f = await compactCredentialFixture(t);
    if (damage === "slot-token") await f.write(f.slotRecord, { ...f.intent, token: randomUUID() });
    if (damage === "ready-token") await f.write(f.readyRecord, { ...f.ready, token: randomUUID() });
    if (damage === "target-identity") await f.write(f.readyRecord, { ...f.ready, target_identity: [1n, 0xffffffffffffffffn] });
    if (["agent-id", "wardian-home", "source", "source-identity"].includes(damage)) {
      const foreign = { ...f.intent };
      if (damage === "agent-id") foreign.agent_id = randomUUID();
      if (damage === "wardian-home") foreign.wardian_home = f.root;
      if (damage === "source") foreign.source = f.fixtureCodex;
      if (damage === "source-identity") foreign.source_identity = [1n, 0xffffffffffffffffn];
      await f.write(f.agentRecord, foreign);
      await f.write(f.slotRecord, foreign);
    }
    await assert.rejects(cleanupAgentCredentials(f.home, f.agentId), undefined, damage);
    for (const directory of [f.target, f.backup, f.fixtureCodex]) assert.equal(await sha256(path.join(directory, "auth.json")), f.authHash);
  }
});

test("linked compact parent and unverified partial copy refuse credential cleanup", async (t) => {
  const f = await compactCredentialFixture(t);
  const originalSlot = `${f.slot}-retained`;
  await fs.rename(f.slot, originalSlot);
  await fs.symlink(originalSlot, f.slot, process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(cleanupAgentCredentials(f.home, f.agentId), /linked/u);
  assert.equal(await sha256(path.join(originalSlot, "h", "auth.json")), f.authHash);
  const partial = await compactCredentialFixture(t, { staging: true });
  await fs.unlink(partial.readyRecord);
  await assert.rejects(cleanupAgentCredentials(partial.home, partial.agentId), /unverified copies/u);
  for (const directory of [partial.source, partial.transfer]) assert.equal(await sha256(path.join(directory, "auth.json")), partial.authHash);
  const moved = await compactCredentialFixture(t);
  const originalHome = `${moved.home}-retained`;
  await fs.rename(moved.home, originalHome);
  await fs.symlink(originalHome, moved.home, process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(cleanupAgentCredentials(moved.home, moved.agentId), /linked/u);
  assert.equal(await sha256(path.join(moved.target, "auth.json")), moved.authHash);
});

test("credential cleanup removes refreshed OAuth copies but refuses auth links", async (t) => {
  const changed = await compactCredentialFixture(t);
  await fs.writeFile(path.join(changed.target, "auth.json"), "refreshed-synthetic-credential-only");
  const outside = path.join(changed.root, "outside-allowlist");
  await fs.mkdir(outside);
  await fs.writeFile(path.join(outside, "auth.json"), "outside-synthetic-credential-only");
  await cleanupAgentCredentials(changed.home, changed.agentId);
  for (const directory of [changed.target, changed.backup]) await assert.rejects(fs.lstat(path.join(directory, "auth.json")), { code: "ENOENT" });
  assert.equal(await fs.readFile(path.join(outside, "auth.json"), "utf8"), "outside-synthetic-credential-only");
  assert.equal(await sha256(path.join(changed.fixtureCodex, "auth.json")), changed.authHash);
  const linked = await compactCredentialFixture(t);
  await fs.unlink(path.join(linked.target, "auth.json"));
  await fs.link(path.join(linked.fixtureCodex, "auth.json"), path.join(linked.target, "auth.json"));
  await assert.rejects(cleanupAgentCredentials(linked.home, linked.agentId), /linked/u);
  assert.equal(await sha256(path.join(linked.fixtureCodex, "auth.json")), linked.authHash);
});

test("fixture environment preserves Windows USERPROFILE while isolating provider paths and filtering secrets", () => {
  const inherited = {
    HOME: "native-home", USERPROFILE: "native-profile", CODEX_HOME: "native-codex",
    WARDIAN_HOME: "owned-run", PATH: "provider-bin", WARDIAN_E2E_RUN_ID: "owned-id",
    WARDIAN_SESSION_ID: "caller", WARDIAN_MEMORY_CAPABILITY: "capability",
    OPENAI_API_KEY: "test-openai", ANTHROPIC_API_KEY: "test-anthropic",
  };
  const before = { ...inherited };
  for (const platform of ["win32", "linux", "darwin"]) {
    const overrides = fixtureEnvironmentOverrides("fixture-profile", "fixture-codex", inherited, platform);
    assert.equal(overrides.HOME, "fixture-profile");
    assert.equal(overrides.CODEX_HOME, "fixture-codex");
    if (platform === "win32") assert.equal(Object.hasOwn(overrides, "USERPROFILE"), false);
    else assert.equal(overrides.USERPROFILE, "fixture-profile");
    for (const key of ["WARDIAN_SESSION_ID", "WARDIAN_MEMORY_CAPABILITY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]) {
      assert.ok(Object.hasOwn(overrides, key), `${key} must be explicitly removed`);
      assert.equal(overrides[key], undefined);
    }
    for (const key of ["WARDIAN_HOME", "PATH", "WARDIAN_E2E_RUN_ID"]) assert.equal(Object.hasOwn(overrides, key), false);
  }
  assert.deepEqual(inherited, before, "Computing overrides must not change the environment restored after the run");
  assert.equal(Object.hasOwn(fixtureEnvironmentOverrides("fixture-profile", "fixture-codex", {}, "win32"), "USERPROFILE"), false);
});

test("real messaging setup accepts only the runner's fresh matching home lock", async (t) => {
  const tempParent = await fs.realpath(os.tmpdir());
  const home = await fs.mkdtemp(path.join(tempParent, "messaging-runner-contract-"));
  t.after(async () => {
    within(tempParent, home);
    assert.equal(await fs.realpath(home), home);
    await fs.rm(home, { recursive: true });
  });
  const harness = { isolatedHome: home, runId: "owned-run" };
  const env = { WARDIAN_E2E_NATIVE_HOME: home, WARDIAN_E2E_RUN_ID: harness.runId };
  await assert.rejects(validateRunnerHome(harness, {}), /native-e2e/u);
  await fs.mkdir(path.join(home, HOME_LOCK_DIRECTORY));
  const owner = { runId: harness.runId, pid: process.ppid, startedAt: new Date().toISOString() };
  const ownerFile = path.join(home, HOME_LOCK_FILE);
  await fs.writeFile(ownerFile, JSON.stringify(owner));
  assert.deepEqual(await validateRunnerHome(harness, env), owner);
  await fs.writeFile(ownerFile, JSON.stringify({ ...owner, runId: "foreign-run" }));
  await assert.rejects(validateRunnerHome(harness, env), /another run/u);
  await fs.writeFile(ownerFile, JSON.stringify({ ...owner, pid: process.pid }));
  await assert.rejects(validateRunnerHome(harness, env), /outside the test process/u);
  await fs.writeFile(ownerFile, JSON.stringify(owner));
  await fs.writeFile(path.join(home, "retained-evidence.json"), "retained");
  await assert.rejects(validateRunnerHome(harness, env), /existing evidence is never reset/u);
  assert.equal(await fs.readFile(path.join(home, "retained-evidence.json"), "utf8"), "retained");
});

test("real Codex sender assigns one task, receiver replies, and sender receives the correlated reply", { timeout: 1_020_000 }, async (t) => {
  if (process.env.WARDIAN_E2E_REAL_MESSAGING_V2 !== "1") {
    return t.skip("Real case NOT RUN: requires WARDIAN_E2E_REAL_MESSAGING_V2=1 and coordinator runner/artifact authorization");
  }
  const expectedVersion = expectedTestVersion();
  const mode = process.env.WARDIAN_E2E_MESSAGING_V2_MODE ?? "background";
  assert.ok(["background", "attached_tui"].includes(mode), "Unknown messaging mode; no provider was launched");
  if (process.env.WARDIAN_E2E_MESSAGING_V2_LIFECYCLE === "1") assert.equal(mode, "attached_tui");
  assert.equal(process.env.WARDIAN_NATIVE_SKIP_BUILD, "1", "This test never builds");
  const requestedCli = process.env.WARDIAN_E2E_MESSAGING_CLI;
  const authSource = process.env.WARDIAN_E2E_CODEX_AUTH_HOME;
  for (const file of [requestedCli, authSource, process.env.WARDIAN_NATIVE_APP]) assert.ok(path.isAbsolute(file ?? ""), "Explicit absolute frozen artifact/auth paths required");
  const harness = await createNativeHarness();
  harness.watchMode = false; // Acceptance cleanup cannot wait for interactive input.
  const runId = harness.runId;
  const runnerLock = await validateRunnerHome(harness);
  // The explicit acceptance CLI, not Cargo target discovery, supplies the
  // run-private CLI. prepareIsolatedHome freezes it alongside the selected app.
  harness.sharedCliPath = requestedCli;
  prepareIsolatedHome(harness);
  assert.deepEqual(harness.homeLock, runnerLock, "Preparation replaced the runner's home claim");
  const cli = harness.cliPath;
  assert.ok(path.isAbsolute(cli ?? "") && harness.frozenArtifacts.cli, "The explicitly selected CLI was not frozen");
  assert.equal(harness.frozenArtifacts.cli.source, requestedCli);
  assert.equal(harness.frozenArtifacts.cli.path, cli);
  const requiredPayloads = [`resources/bin/${path.basename(cli)}`];
  if (process.platform === "win32") requiredPayloads.push("conpty/x64/conpty.dll", "conpty/x64/OpenConsole.exe");
  const runtimePayloads = {};
  for (const relative of requiredPayloads) {
    const source = path.join(path.dirname(process.env.WARDIAN_NATIVE_APP), relative);
    const frozen = path.join(path.dirname(harness.appPath), relative);
    const expected = await sha256(source);
    assert.equal(await sha256(frozen), expected, `Required runtime payload was not frozen: ${relative}`);
    runtimePayloads[relative] = { source, frozen, sha256: expected };
  }
  assert.equal(runtimePayloads[`resources/bin/${path.basename(cli)}`].sha256, await sha256(cli), "Runtime MCP CLI must match the selected CLI");
  const home = harness.isolatedHome;
  const profile = path.join(home, "fixture-profile");
  const fixtureCodex = path.join(profile, ".codex");
  await fs.mkdir(fixtureCodex, { recursive: true });
  const report = { schema: 2, issue: 1218, mode, status: "running", actual_cases_passed: 0,
    started_at: new Date().toISOString(), model: MODEL, requested_effort: EFFORT,
    runtime_payloads: runtimePayloads,
    isolation: { run_id: runId, home, home_lock: runnerLock,
      lock_release_owner: "upstream_runner_finally_after_supervised_child_exit" },
    model_selection: {
      basis: "Lowest published standard Codex credit rates among the available general-purpose models",
      checked_at: "2026-09-08", source: "https://learn.chatgpt.com/docs/pricing",
      credits_per_million_tokens: { input: 5, cached_input: 0.5, output: 30 },
      special_purpose_and_unpriced_models_ranked: false,
    },
    expected_codex_version: expectedVersion, requested_codex_executable: process.env.WARDIAN_E2E_CODEX_EXECUTABLE ?? null,
    cases: { correlated_real_exchange: { status: "running" } }, sources: {}, artifacts: {}, agents: [],
    attached_tui: mode === "attached_tui" ? "pending" : "not_run_in_background_mode",
    optional_cases: { idle_information: "not_run", active_interrupt: "not_run" }, cleanup: [] };
  const reportPath = path.join(home, "real-messaging-v2-report.json");
  const save = () => fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  const previousEnv = new Map();
  let session;
  let cleanupFailed = false;
  let fixtureAuthHash;
  try {
    await save();
    for (const source of SOURCES) {
      const destination = path.join(home, "source-bytes", source);
      await fs.mkdir(path.dirname(destination), { recursive: true });
      await fs.copyFile(path.join(harness.repoRoot, source), destination);
      report.sources[source] = await sha256(destination);
    }
    for (const [name, file] of [["app", harness.appPath], ["cli", cli]]) {
      const destination = path.join(home, "artifact-bytes", name, path.basename(file));
      await fs.mkdir(path.dirname(destination), { recursive: true });
      await fs.copyFile(file, destination);
      report.artifacts[name] = { source: file, retained: destination, sha256: await sha256(destination),
        requested_source: harness.frozenArtifacts[name].source, freeze: harness.frozenArtifacts[name] };
      assert.equal(await sha256(file), report.artifacts[name].sha256);
      assert.equal(await sha256(report.artifacts[name].requested_source), report.artifacts[name].sha256, "Frozen artifact differs from its selected source");
      assert.equal(harness.frozenArtifacts[name].sha256, report.artifacts[name].sha256);
    }
    // No MCP entry is seeded here: normal managed startup must create it.
    const fixtureConfig = `model = "${MODEL}"\nmodel_reasoning_effort = "${EFFORT}"\napproval_policy = "never"\nsandbox_mode = "read-only"\n[features]\napps = false\nmulti_agent = false\nskip_host_skill_discovery = true\n[agents]\nenabled = false\n`;
    await fs.writeFile(path.join(fixtureCodex, "config.toml"), fixtureConfig);
    fixtureAuthHash = await sha256(path.join(authSource, "auth.json")); // Initial copy integrity only; refreshed credentials remain cleanup-authorized by the mapping.
    await fs.copyFile(path.join(authSource, "auth.json"), path.join(fixtureCodex, "auth.json"));
    assert.equal(await sha256(path.join(fixtureCodex, "auth.json")), fixtureAuthHash);
    report.fixture_config = fixtureConfig;
    report.permissions = process.env.WARDIAN_E2E_MESSAGING_V2_APPROVE_TOOLS === "1" ? "six_named_tools_in_private_agent_homes" : "provider_policy_unchanged";
    const overrides = fixtureEnvironmentOverrides(profile, fixtureCodex);
    for (const [key, value] of Object.entries(overrides)) {
      previousEnv.set(key, process.env[key]);
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    session = await startNativeSession(harness);
    assert.equal(harness.driverPortOwnership?.verified, true, "WebDriver listener ownership was not established");
    assert.equal(harness.nativeDriverPortOwnership?.verified, true, "Native driver listener ownership was not established");
    report.isolation.ports = { driver: harness.driverPort, native_driver: harness.nativeDriverPort,
      driver_ownership: harness.driverPortOwnership, native_driver_ownership: harness.nativeDriverPortOwnership };
    // Shared-owner startup has a bounded 120-second provider budget. WebDriver's
    // default 30-second script limit must not abandon a still-owned resume.
    await session.driver.manage().setTimeouts({ script: 180_000 });
    await waitForAppShell(session.driver, 30_000);
    report.isolation.webdriver_session_id = (await session.driver.getSession()).getId();
    await save();
    report.artifacts.codex = await executableEvidence(session.driver, expectedVersion);
    await save();
    const catalog = await invokeTauri(session.driver, "list_provider_model_catalog", { provider: "codex", forceRefresh: true });
    report.artifacts.codex.version_output = catalog.version;
    await save();
    report.artifacts.codex.actual_version = assertExecutableVersion(catalog.version, expectedVersion);
    assert.equal(catalog.source, "live_catalog");
    assert.ok(!catalog.refresh_error, "Fresh Codex catalogue unavailable; no substitution");
    const selected = catalog.models?.find((model) => model.id === MODEL);
    assert.ok(selected, `${MODEL} absent from live catalogue; do not pick another model`);
    assert.ok(selected.effort_options.includes(EFFORT), "Low effort absent from live catalogue");
    report.catalogue = { provider: catalog.provider, version: catalog.version, source: catalog.source, selected };
    const fixtureWorkspaces = await prepareFixtureWorkspaces(home);
    const trustFixtures = fixtureTrustEnabled();
    report.fixture_workspace_trust = await configureFixtureTrust(session.driver, home, fixtureWorkspaces, trustFixtures);
    await save();
    for (const role of ["sender", "receiver"]) {
      const { workspace } = fixtureWorkspaces[role];
      const configOverride = { provider: "codex", model: MODEL, conversation_logging: "enabled", session_persistence: "resume",
        provider_config: { type: "codex", reasoning_effort: EFFORT, sandbox_mode: "read-only", approval_policy: "never" } };
      const config = await invokeTauri(session.driver, "spawn_agent", { req: {
        sessionName: `Messaging-${role}-${randomUUID().slice(0, 8)}`, agentClass: "TestClass", folder: workspace, isOff: true, resumeSession: null, configOverride,
      } });
      const agent = { role, session_id: config.session_id, session_name: config.session_name, workspace, requested_config: configOverride, expected_version: expectedVersion };
      report.agents.push(agent); // Record ownership immediately, before later assertions.
      assert.equal(config.provider, "codex");
      assert.equal(config.model, MODEL);
      assert.equal(config.provider_config.reasoning_effort, EFFORT);
      agent.persisted_provider_config = config.provider_config;
      agent.registration = await registration(home, config, report.artifacts.cli.sha256, report.permissions === "six_named_tools_in_private_agent_homes");
      const doctor = await command(cli, home, harness.repoRoot, ["agent", "doctor", agent.session_id], 60_000);
      agent.launch_trust = assertFixtureLaunchTrust(doctor, home, agent, trustFixtures);
      agent.project_trust_before = fixtureProjectTrust(await fs.readFile(path.join(agent.registration.codex_home, "config.toml"), "utf8"), home, workspace);
      if (!trustFixtures) assert.ok(agent.project_trust_before.every((entry) => entry.trust_level !== "trusted"), "Untrusted diagnostics must not inherit fixture trust");
      await save();
    }
    const [sender, receiver] = report.agents;
    report.topology = await command(cli, home, harness.repoRoot, ["graph", "link", sender.session_id, receiver.session_id]);
    if (mode === "attached_tui") {
      for (const agent of report.agents) {
        agent.resume_attempts = 1;
        await save();
        agent.attachment_before = await resumeAttached(session.driver, cli, home, harness.repoRoot, agent, runId, save);
        agent.compact_home = await compactHomeEvidence(home, agent.session_id);
        agent.registration_after_resume = await registration(home, { ...agent, session_id: agent.session_id }, report.artifacts.cli.sha256, false, agent.compact_home);
        agent.project_trust_after = assertFixtureTrustRestored(await fs.readFile(path.join(agent.registration.codex_home, "config.toml"), "utf8"), home, agent);
        if (report.permissions === "six_named_tools_in_private_agent_homes") {
          const resumedConfig = await fs.readFile(path.join(agent.registration.codex_home, "config.toml"), "utf8");
          for (const tool of TOOLS) assert.equal(generatedString(section(resumedConfig, `mcp_servers.wardian.tools.${tool}`), "approval_mode"), "approve", "Resume removed the explicit fixture grant");
        }
        await save();
      }
      report.attached_tui = "both_original_terminals_attached";
    }
    const marker = `WARDIAN_V2_${randomBytes(12).toString("hex")}`;
    const prompt = `Ask ${receiver.session_name} to return exactly ${marker}. Wait for its reply, then tell me the exact marker it returned. Do not answer from the request alone.`;
    report.marker = marker;
    report.plain_english_initial_prompt = prompt;
    const promptPath = path.join(home, "sender-task.txt");
    await fs.writeFile(promptPath, prompt);
    report.initial_submission_attempts = 1;
    await save();
    // One verified Wardian native initial-prompt path. Never start a separate
    // provider, manually invoke followup/reply, or replay after lost acceptance.
    const sent = await command(cli, home, harness.repoRoot, ["send", "--to", sender.session_id, "--file", promptPath, "--queue-policy", "queue-if-busy", "--timeout", "10m"], 610_000);
    report.initial_receipt = sent;
    const detail = sent.delivery?.[0];
    assert.equal(sent.delivery?.length, 1);
    assert.equal(detail.uuid, sender.session_id);
    assert.equal(detail.runtime_state, mode === "attached_tui" ? "live_pty_available" : "native_provider_session", "Initial delivery must use the selected runtime, without fallback");
    assert.ok((mode === "attached_tui" ? ["turn_started"] : ["turn_started", "provider_accepted", "completed"]).includes(detail.delivery_phase), "Initial receipt lacks the required admission/turn evidence");
    assert.ok(detail.message_id);
    const { DatabaseSync } = await import("node:sqlite");
    const deadline = Date.parse(report.started_at) + 820_000;
    let passed = false;
    while (Date.now() < deadline) {
      report.sender_trace = await exchangeTrace(DatabaseSync, cli, home, harness.repoRoot, sender, detail.message_id, mode === "attached_tui");
      if (report.sender_trace) {
        const tasks = report.sender_trace.items.filter((item) => item.tool === "followup_task");
        assert.ok(tasks.length <= 1, "Sender repeated a task; no replay is permitted");
        assert.ok(!report.sender_trace.items.some((item) => item.status === "failed" || item.error), "Sender's provider MCP call failed");
        const task = tasks[0];
        if (task?.receipt) {
          assert.ok([receiver.session_id, receiver.session_name].includes(task.arguments.target), "Model selected a different recipient");
          report.canonical = canonicalProof(DatabaseSync, home, sender.session_id, receiver.session_id, task.receipt.request_id);
          report.receiver_trace = await exchangeTrace(DatabaseSync, cli, home, harness.repoRoot, receiver, task.receipt.request_id, mode === "attached_tui");
          report.receiver_binding = receiver.observed_binding ?? null;
          if (report.receiver_trace) {
            assert.ok(!report.receiver_trace.items.some((item) => item.status === "failed" || item.error), "Receiver's provider MCP call failed");
            passed = proveExchange(report.sender_trace, report.receiver_trace, report.canonical, marker, sender.session_id, receiver.session_id);
            const failure = terminalExchangeFailure(report.receiver_trace, report.receiver_trace.accepted_turn_id, "reply");
            if (failure) { report.terminal_failure = failure; throw new Error(`${failure.reason}: ${failure.final_message}`); }
          }
        }
      }
      if (mode === "attached_tui") {
        // Live PTY delivery has an InteractionRecord, not a native-broker
        // envelope. Its literal user message and completion must be observed
        // on the very thread attached before submission.
        const initialUsers = report.sender_trace?.items.filter((item) => item.type === "userMessage" && item.text === prompt) ?? [];
        assert.ok(initialUsers.length <= 1, "Initial prompt appeared more than once; no replay permitted");
        const initialTurn = initialUsers[0]?.turn_id;
        const completed = initialTurn && report.sender_trace.turns.some((turn) => turn.turn_id === initialTurn && turn.status === "completed");
        report.initial_delivery = { interaction_id: detail.message_id, phase: completed ? "completed" : "turn_started",
          provider: "codex", provider_turn_id: initialTurn ?? null, target_agent_id: sender.session_id, runtime_state: detail.runtime_state };
      } else {
        const inspection = await command(cli, home, harness.repoRoot, ["delivery", "show", detail.message_id, "--evidence-limit", "100"]);
        assert.equal(inspection.record?.envelope?.interaction_id, detail.message_id);
        assert.equal(inspection.record?.envelope?.target_agent_id, sender.session_id);
        assert.equal(inspection.record?.envelope?.body, prompt);
        assert.equal(inspection.record?.provider, "codex");
        report.initial_delivery = { interaction_id: inspection.record?.envelope?.interaction_id, phase: inspection.record?.phase,
          provider: inspection.record?.provider, provider_turn_id: inspection.record?.provider_turn_id,
          target_agent_id: inspection.record?.envelope?.target_agent_id,
          phases: (inspection.evidence ?? []).map((entry) => ({ phase: entry.phase, source: entry.source })) };
        assert.ok(!["failed", "failed_before_submit", "cancelled", "expired", "stale_generation"].includes(inspection.record?.phase), "Initial native delivery terminated unsuccessfully");
      }
      const failure = terminalExchangeFailure(report.sender_trace, report.initial_delivery.provider_turn_id, "followup_task");
      if (failure) { report.terminal_failure = failure; throw new Error(`${failure.reason}: ${failure.final_message}`); }
      await save();
      if (passed && report.initial_delivery.phase === "completed") break;
      // A broker completion can precede provider-history projection. Keep
      // reconciling the same IDs within the deadline; never submit another turn.
      await delay(1000); // Poll observable evidence, never delay then resubmit.
    }
    assert.ok(passed && report.initial_delivery.phase === "completed", "Correlated real-provider exchange timed out; no retry was made");
    report.cases.correlated_real_exchange = { status: "pass", request_id: report.canonical.request_id, reply_id: report.canonical.reply_id };
    report.actual_cases_passed = 1;
    if (mode === "attached_tui") {
      for (const agent of report.agents) {
        const renderDeadline = Date.now() + 15_000;
        do {
          agent.attachment_after = await attachmentEvidence(session.driver, cli, home, harness.repoRoot, agent, agent.attachment_before, marker);
          if ((agent.role !== "sender" || agent.attachment_after.marker_visible) &&
            agent.attachment_after.sequence_barrier > agent.attachment_before.sequence_barrier) break;
          await delay(500); // Renderer projection may follow provider completion.
        } while (Date.now() < renderDeadline);
        assert.ok(agent.attachment_after.sequence_barrier > agent.attachment_before.sequence_barrier, "Original terminal did not render activity from the shared exchange");
        // The requester was asked to display the answer. The receiver may
        // correctly finish with a short acknowledgement after its MCP reply;
        // its exact reply content is proved in the owned provider trace above.
        if (agent.role === "sender") assert.ok(agent.attachment_after.marker_visible,
          "The requester's final answer must also reach its original attached TUI");
      }
      report.attached_tui = "pass_same_owner_thread_and_original_terminals";
    }
    if (process.env.WARDIAN_E2E_MESSAGING_V2_LIFECYCLE === "1") {
      assert.equal(mode, "attached_tui", "Lifecycle acceptance requires a retained attached owner");
      await lifecycleCases(DatabaseSync, session, cli, home, harness.repoRoot, sender, receiver, report, save);
    }
    if (mode === "background") {
      await backgroundContinuity(DatabaseSync, cli, home, harness.repoRoot, sender, receiver, report, save);
    }
    for (const [source, expected] of Object.entries(report.sources)) assert.equal(await sha256(path.join(harness.repoRoot, source)), expected, "Source changed during frozen acceptance");
    for (const artifact of Object.values(report.artifacts)) {
      assert.equal(await sha256(artifact.source), artifact.sha256, "Artifact changed during acceptance");
      if (artifact.requested_source) assert.equal(await sha256(artifact.requested_source), artifact.sha256, "Selected source artifact changed during acceptance");
    }
    for (const payload of Object.values(runtimePayloads)) assert.equal(await sha256(payload.frozen), payload.sha256, "Runtime payload changed during acceptance");
    report.actual_cases_passed = 1 + Object.values(report.optional_cases).filter((item) => item?.status === "pass").length;
    report.status = "pass";
  } catch (error) {
    report.status = "fail";
    if (report.cases.correlated_real_exchange.status === "running") report.cases.correlated_real_exchange.status = "fail";
    for (const item of Object.values(report.optional_cases)) if (item?.status === "running") item.status = "fail";
    report.error = error.message;
    await save();
    throw error;
  } finally {
    let processesStopped = !session;
    if (session) {
      for (const agent of [...report.agents].reverse()) {
        try { await invokeTauri(session.driver, "pause_agent", { sessionId: agent.session_id }); report.cleanup.push({ agent_id: agent.session_id, paused: true }); }
        catch { report.cleanup.push({ agent_id: agent.session_id, paused: false }); }
      }
      try { await session.close(); processesStopped = true; }
      catch { report.cleanup.push({ app_closed: false }); }
    }
    const currentLock = readHomeLock(home);
    const lockStillOwned = currentLock?.runId === runnerLock.runId && currentLock?.pid === runnerLock.pid;
    if (!lockStillOwned) cleanupFailed = true;
    report.cleanup.push({ home_lock_still_owned: lockStillOwned, lock_release_owner: "upstream_runner_after_test_exit" });
    // The child never releases the runner's claim. The runner's finally runs
    // after this test's credential cleanup and the supervised process exit.
    if (fixtureAuthHash) {
      if (!processesStopped || !lockStillOwned) {
        cleanupFailed = true;
        report.cleanup.push({ credential_copies_removed: false, retained_reason: "Owned process shutdown or runner home ownership was not confirmed" });
      } else {
        for (const agent of report.agents) {
          try { report.cleanup.push(await cleanupAgentCredentials(home, agent.session_id)); }
          catch {
            cleanupFailed = true;
            report.cleanup.push({ agent_id: agent.session_id, credential_copies_removed: false,
              retained_reason: "Mapping, directory identity, or test-created credential copy could not be authenticated" });
          }
        }
        try { report.cleanup.push(await cleanupFixtureCredential(home, fixtureCodex)); }
        catch {
          cleanupFailed = true;
          report.cleanup.push({ fixture_credential_removed: false, retained_reason: "Exact test-created fixture credential could not be authenticated" });
        }
      }
    }
    if (cleanupFailed) report.status = "fail";
    for (const [key, value] of previousEnv) { if (value === undefined) delete process.env[key]; else process.env[key] = value; }
    report.finished_at = new Date().toISOString();
    await save();
    t.diagnostic(`Real Codex messaging: ${report.status}; actual cases passed=${report.actual_cases_passed}; report=${reportPath}`);
  }
  if (cleanupFailed) throw new Error("Credential cleanup failed closed; inspect the private report and retained paths");
});
