import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";

const EXPECTED_CONTROL_ROLES = [
  "AXCloseButton",
  "AXMinimizeButton",
  "AXZoomButton",
];
const TOP_CHROME_BAND = 36;
const LEFT_RESERVED_BAND = 72;
const COMMAND_TIMEOUT_MS = 15_000;
const APPLE_EVENT_TIMEOUT_MS = 10_000;

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

function fail(message) {
  const error = new Error(message);
  error.fatal = true;
  throw error;
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function isInside(childPath, parentPath) {
  const relative = path.relative(parentPath, childPath);
  return relative === "" || (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
}

function resolveExistingPath(value, label) {
  if (!path.isAbsolute(value) || !existsSync(value)) {
    fail(`${label} must be an existing absolute path`);
  }
  return realpathSync(value);
}

function displayPath(value, workspacePath, runnerTempPath) {
  if (isInside(value, workspacePath)) {
    return `<workspace>/${path.relative(workspacePath, value).replaceAll(path.sep, "/")}`;
  }
  if (isInside(value, runnerTempPath)) {
    return `<runner-temp>/${path.relative(runnerTempPath, value).replaceAll(path.sep, "/")}`;
  }
  return "<redacted>";
}

function commandResult(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 4 * 1024 * 1024,
    timeout: options.timeout ?? COMMAND_TIMEOUT_MS,
  });
  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  const combined = `${stdout}\n${stderr}`;
  const permissionFailure = /accessibility|assistive access|screen recording|not authorized|permission denied|not allowed to send|user interaction is not allowed|timed out/i.test(combined);
  if (permissionFailure) {
    const error = new Error(`${path.basename(command)} failed closed on a permission or timeout condition`);
    error.fatal = true;
    throw error;
  }
  if (result.error || result.status !== 0) {
    const error = new Error(`${path.basename(command)} failed with status ${result.status ?? "unknown"}`);
    error.fatal = false;
    throw error;
  }
  return { stdout, stderr };
}

function sha256File(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function readPlistValue(plistPath, key) {
  const result = commandResult("/usr/libexec/PlistBuddy", ["-c", `Print :${key}`, plistPath]);
  const value = result.stdout.trim();
  if (!value) {
    fail(`The app bundle plist has no ${key}`);
  }
  return value;
}

function readPngSize(filePath) {
  const bytes = readFileSync(filePath);
  const signature = "89504e470d0a1a0a";
  if (bytes.length < 24 || bytes.subarray(0, 8).toString("hex") !== signature || bytes.subarray(12, 16).toString("ascii") !== "IHDR") {
    fail(`Evidence file is not a PNG: ${path.basename(filePath)}`);
  }
  return {
    bytes: bytes.length,
    height: bytes.readUInt32BE(20),
    sha256: sha256File(filePath),
    width: bytes.readUInt32BE(16),
  };
}

function appleScriptFor(bundleIdentifier, action, expectedOwnedPid) {
  return `
on scalarText(theValue)
  set textValue to theValue as text
  set AppleScript's text item delimiters to {tab, return, linefeed}
  set textItems to text items of textValue
  set AppleScript's text item delimiters to "_"
  set textValue to textItems as text
  set AppleScript's text item delimiters to ""
  return textValue
end scalarText

on emitWindow(targetProcess, targetWindow)
  tell application "System Events"
    set targetPosition to position of targetWindow
    set targetSize to size of targetWindow
    try
      set targetFullScreen to value of attribute "AXFullScreen" of targetWindow
    on error
      error "AXFullScreen is unavailable"
    end try
    try
      set targetMinimized to value of attribute "AXMinimized" of targetWindow
    on error
      error "AXMinimized is unavailable"
    end try
    set outputText to "WINDOW" & tab & (unix id of targetProcess as text) & tab & (item 1 of targetPosition as text) & tab & (item 2 of targetPosition as text) & tab & (item 1 of targetSize as text) & tab & (item 2 of targetSize as text) & tab & my scalarText(name of targetWindow) & tab & (frontmost of targetProcess as text) & tab & (targetFullScreen as text) & tab & (targetMinimized as text) & linefeed
    repeat with candidate in (entire contents of targetWindow)
      try
        set candidateRef to contents of candidate
        set candidateSubrole to value of attribute "AXSubrole" of candidateRef
        if candidateSubrole is in {"AXCloseButton", "AXMinimizeButton", "AXZoomButton"} then
          set candidateRole to value of attribute "AXRole" of candidateRef
          set candidateEnabled to value of attribute "AXEnabled" of candidateRef
          set candidatePosition to value of attribute "AXPosition" of candidateRef
          set candidateSize to value of attribute "AXSize" of candidateRef
          set outputText to outputText & "BUTTON" & tab & my scalarText(candidateSubrole) & tab & my scalarText(candidateRole) & tab & (candidateEnabled as text) & tab & (item 1 of candidatePosition as text) & tab & (item 2 of candidatePosition as text) & tab & (item 1 of candidateSize as text) & tab & (item 2 of candidateSize as text) & linefeed
        end if
      end try
    end repeat
    return outputText
  end tell
end emitWindow

on buttonForSubrole(targetWindow, expectedSubrole)
  tell application "System Events"
    repeat with candidate in (entire contents of targetWindow)
      try
        set candidateRef to contents of candidate
        if (value of attribute "AXSubrole" of candidateRef) is expectedSubrole then
          return candidateRef
        end if
      end try
    end repeat
    error "Expected AX button role was not found: " & expectedSubrole
  end tell
end buttonForSubrole

on emitProcess(targetProcess)
  tell application "System Events"
    return "PROCESS" & tab & (unix id of targetProcess as text) & tab & (visible of targetProcess as text) & tab & ((count of windows of targetProcess) as text) & linefeed
  end tell
end emitProcess

on run
  tell application "System Events"
    set targetProcesses to every process whose bundle identifier is "${bundleIdentifier}"
    if (count of targetProcesses) is not 1 then
      error "Expected exactly one owned Wardian process"
    end if
    set targetProcess to item 1 of targetProcesses
    set observedPid to unix id of targetProcess
    if observedPid is not ${expectedOwnedPid} then
      error "The AX process identity is not the owned PID"
    end if
    if "${action}" is "process-state" then
      return my emitProcess(targetProcess)
    end if
    if (unix id of targetProcess) is not ${expectedOwnedPid} then
      error "The AX activation target changed before activation"
    end if
    set frontmost of targetProcess to true
    delay 0.2
    if (count of windows of targetProcess) is less than 1 then
      error "The owned Wardian process has no native window"
    end if
    set targetWindow to front window of targetProcess
    set outputText to my emitWindow(targetProcess, targetWindow)
    if "${action}" is "press-zoom" then
      if (unix id of targetProcess) is not ${expectedOwnedPid} then
        error "The AX action target changed before zoom"
      end if
      set actionPid to unix id of targetProcess
      set actionMinimized to value of attribute "AXMinimized" of targetWindow
      if actionMinimized is not false then
        error "The zoom target is not known to be non-minimized"
      end if
      set targetButton to my buttonForSubrole(targetWindow, "AXZoomButton")
      perform action "AXPress" of targetButton
      set outputText to outputText & "ACTION" & tab & (actionPid as text) & tab & "AXPress" & tab & "AXZoomButton" & tab & "ok" & tab & (actionMinimized as text) & linefeed
    else if "${action}" is "press-minimize" then
      if (unix id of targetProcess) is not ${expectedOwnedPid} then
        error "The AX action target changed before minimize"
      end if
      set actionPid to unix id of targetProcess
      set actionMinimized to value of attribute "AXMinimized" of targetWindow
      if actionMinimized is not false then
        error "The minimize target is not known to be non-minimized"
      end if
      set targetButton to my buttonForSubrole(targetWindow, "AXMinimizeButton")
      perform action "AXPress" of targetButton
      set outputText to outputText & "ACTION" & tab & (actionPid as text) & tab & "AXPress" & tab & "AXMinimizeButton" & tab & "ok" & tab & (actionMinimized as text) & linefeed
    else if "${action}" is "raise" then
      if (unix id of targetProcess) is not ${expectedOwnedPid} then
        error "The AX action target changed before deminimize"
      end if
      set actionPid to unix id of targetProcess
      set actionMinimized to value of attribute "AXMinimized" of targetWindow
      if actionMinimized is not true then
        error "The AXRaise target is not known to be minimized"
      end if
      set value of attribute "AXMinimized" of targetWindow to false
      perform action "AXRaise" of targetWindow
      set outputText to outputText & "ACTION" & tab & (actionPid as text) & tab & "AXRaise" & tab & "AXWindow" & tab & "ok" & tab & (actionMinimized as text) & linefeed
    else if "${action}" is "press-close" then
      if (unix id of targetProcess) is not ${expectedOwnedPid} then
        error "The AX action target changed before close"
      end if
      set actionPid to unix id of targetProcess
      set actionMinimized to value of attribute "AXMinimized" of targetWindow
      if actionMinimized is not false then
        error "The close target is not known to be non-minimized"
      end if
      set targetButton to my buttonForSubrole(targetWindow, "AXCloseButton")
      perform action "AXPress" of targetButton
      set outputText to outputText & "ACTION" & tab & (actionPid as text) & tab & "AXPress" & tab & "AXCloseButton" & tab & "ok" & tab & (actionMinimized as text) & linefeed
    end if
    delay 0.1
    return outputText
  end tell
end run
`;
}

function parseSnapshot(output) {
  const snapshot = { buttons: {}, actions: [] };
  for (const line of output.split(/\r?\n/).filter(Boolean)) {
    const fields = line.split("\t");
    if (fields[0] === "WINDOW" && fields.length === 10) {
      if (snapshot.window) {
        fail("AX returned more than one native window record");
      }
      snapshot.window = {
        bundleProcessId: Number(fields[1]),
        height: Number(fields[5]),
        minimized: fields[9] === "true" ? true : fields[9] === "false" ? false : undefined,
        name: fields[6],
        width: Number(fields[4]),
        x: Number(fields[2]),
        y: Number(fields[3]),
        frontmost: fields[7] === "true" ? true : fields[7] === "false" ? false : undefined,
        fullScreen: fields[8] === "true" ? true : fields[8] === "false" ? false : undefined,
      };
    } else if (fields[0] === "BUTTON" && fields.length === 8) {
      const subrole = fields[1];
      if (snapshot.buttons[subrole]) {
        fail(`AX returned duplicate ${subrole} controls`);
      }
      snapshot.buttons[subrole] = {
        enabled: fields[3] === "true" ? true : fields[3] === "false" ? false : undefined,
        height: Number(fields[7]),
        role: fields[2],
        width: Number(fields[6]),
        x: Number(fields[4]),
        y: Number(fields[5]),
      };
    } else if (fields[0] === "ACTION" && fields.length === 6) {
      snapshot.actions.push({
        action: fields[2],
        control: fields[3],
        preMinimized: fields[5] === "true" ? true : fields[5] === "false" ? false : undefined,
        processId: Number(fields[1]),
        result: fields[4],
      });
    }
  }
  if (!snapshot.window) {
    fail("AX returned no native window record");
  }
  for (const role of EXPECTED_CONTROL_ROLES) {
    if (!snapshot.buttons[role]) {
      fail(`AX did not expose required native control role ${role}`);
    }
  }
  const numericValues = [
    snapshot.window.bundleProcessId,
    snapshot.window.x,
    snapshot.window.y,
    snapshot.window.width,
    snapshot.window.height,
    ...Object.values(snapshot.buttons).flatMap((button) => [button.x, button.y, button.width, button.height]),
  ];
  if (numericValues.some((value) => !Number.isFinite(value))) {
    fail("AX returned an unknown frame or process identity");
  }
  if (snapshot.window.frontmost === undefined || snapshot.window.fullScreen === undefined || snapshot.window.minimized === undefined || Object.values(snapshot.buttons).some((button) => button.enabled === undefined)) {
    fail("AX returned an unknown boolean state");
  }
  return snapshot;
}

function parseProcessState(output) {
  const fields = output.trim().split("\t");
  if (fields.length !== 4 || fields[0] !== "PROCESS") {
    fail("AX returned no owned-process state");
  }
  const processId = Number(fields[1]);
  const visible = fields[2] === "true" ? true : fields[2] === "false" ? false : undefined;
  const windowCount = Number(fields[3]);
  if (!Number.isInteger(processId) || visible === undefined || !Number.isInteger(windowCount) || windowCount < 0) {
    fail("AX returned an unknown owned-process state");
  }
  return { processId, visible, windowCount };
}

function runAppleScript(bundleIdentifier, action, expectedOwnedPid) {
  const result = spawnSync("/usr/bin/osascript", ["-e", appleScriptFor(bundleIdentifier, action, expectedOwnedPid)], {
    encoding: "utf8",
    maxBuffer: 4 * 1024 * 1024,
    timeout: APPLE_EVENT_TIMEOUT_MS,
  });
  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  const combined = `${stdout}\n${stderr}`;
  const permissionFailure = /accessibility|assistive access|screen recording|not authorized|permission denied|not allowed to send|user interaction is not allowed|timed out/i.test(combined);
  if (permissionFailure || result.error || result.status !== 0) {
    const error = new Error(`AppleScript ${action} failed closed`);
    error.fatal = permissionFailure || Boolean(result.error?.code === "ETIMEDOUT");
    throw error;
  }
  return action === "process-state" ? parseProcessState(stdout) : parseSnapshot(stdout);
}

function assertNativeState(snapshot, phase, appPid) {
  if (snapshot.window.bundleProcessId !== appPid || snapshot.window.name !== "Wardian") {
    fail(`${phase}: AX window identity does not match the owned app process`);
  }
  if (!snapshot.window.frontmost || snapshot.window.fullScreen || snapshot.window.minimized) {
    fail(`${phase}: native window is not a frontmost, non-fullscreen, non-minimized window`);
  }
  const windowRight = snapshot.window.x + snapshot.window.width;
  const windowBottom = snapshot.window.y + snapshot.window.height;
  const controls = {};
  for (const role of EXPECTED_CONTROL_ROLES) {
    const button = snapshot.buttons[role];
    const buttonRight = button.x + button.width;
    const buttonBottom = button.y + button.height;
    if (button.role !== "AXButton" || !button.enabled || button.width <= 0 || button.height <= 0 || button.x < snapshot.window.x || button.y < snapshot.window.y || buttonRight > windowRight || buttonBottom > windowBottom || buttonBottom > snapshot.window.y + TOP_CHROME_BAND) {
      fail(`${phase}: ${role} is not an enabled, nonzero, hittable control in the top ${TOP_CHROME_BAND}px band`);
    }
    controls[role] = {
      enabled: button.enabled,
      frame: { height: button.height, width: button.width, x: button.x, y: button.y },
      hittable: false,
      role: button.role,
      topOffset: button.y - snapshot.window.y,
    };
  }
  return {
    controls,
    fullscreen: snapshot.window.fullScreen,
    frame: {
      height: snapshot.window.height,
      width: snapshot.window.width,
      x: snapshot.window.x,
      y: snapshot.window.y,
    },
    minimized: snapshot.window.minimized,
    name: snapshot.window.name,
    pid: snapshot.window.bundleProcessId,
  };
}

function actionRecord(snapshot, action, expectedControl, appPid) {
  const evidence = snapshot.actions.find((entry) => entry.action === "AXPress" || entry.action === "AXRaise");
  if (!evidence || evidence.processId !== appPid || evidence.control !== expectedControl || evidence.result !== "ok" || evidence.preMinimized !== (expectedControl === "AXWindow")) {
    fail(`${action}: AX did not report the expected native action`);
  }
  return {
    action,
    control: expectedControl,
    deminimize: expectedControl === "AXWindow" ? "set AXMinimized=false before AXRaise" : null,
    result: evidence.result,
    processId: evidence.processId,
    preMinimized: evidence.preMinimized,
    frameBeforeAction: snapshot.buttons[expectedControl] ?? null,
  };
}

async function waitForSnapshot(bundleIdentifier, appPid, label, predicate, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const snapshot = runAppleScript(bundleIdentifier, "snapshot", appPid);
      if (predicate(snapshot)) {
        return snapshot;
      }
    } catch (error) {
      lastError = error;
      if (error.fatal) {
        throw error;
      }
    }
    await sleep(250);
  }
  throw new Error(`${label}: native state did not settle${lastError ? ` (${lastError.message})` : ""}`);
}

function capturePng(filePath, rectangle = null) {
  const args = ["-x", "-o"];
  if (rectangle) {
    args.push("-R", [rectangle.x, rectangle.y, rectangle.width, rectangle.height].map((value) => Math.round(value)).join(","));
  }
  args.push(filePath);
  commandResult("/usr/sbin/screencapture", args, { timeout: COMMAND_TIMEOUT_MS });
  return readPngSize(filePath);
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

function processTable() {
  const result = spawnSync("/bin/ps", ["-axo", "pid=,pgid=,command="], { encoding: "utf8" });
  if (result.status !== 0) {
    return [];
  }
  return (result.stdout ?? "").split(/\r?\n/).flatMap((line) => {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/);
    return match ? [{ command: match[3], pgid: Number(match[2]), pid: Number(match[1]) }] : [];
  });
}

function processRecord(pid) {
  return processTable().find((record) => record.pid === pid) ?? null;
}

function ownedPidMatches(pid, executable) {
  const record = processRecord(pid);
  return Boolean(record && record.command.includes(executable));
}

function captureOwnedProcessGroup(pid, executable) {
  const leader = processRecord(pid);
  if (!leader || leader.pgid !== pid || !leader.command.includes(executable)) {
    fail("The app leader did not establish a private process group");
  }
  return { executable, leaderPid: pid, pgid: leader.pgid };
}

function processGroupMembers(group) {
  return processTable().filter((record) => record.pgid === group.pgid);
}

async function waitForGroupQuiescence(group, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let members = processGroupMembers(group);
  while (members.length > 0 && Date.now() < deadline) {
    await sleep(250);
    members = processGroupMembers(group);
  }
  return members;
}

async function terminateOwnedProcessGroup(group) {
  const cleanup = {
    groupQuiescent: false,
    leaderPid: group.leaderPid,
    leaderQuiescent: !processIsAlive(group.leaderPid),
    pgid: group.pgid,
    signals: [],
    status: "unresolved",
  };
  if (group.pgid !== group.leaderPid) {
    cleanup.status = "refused-group-identity-mismatch";
    return cleanup;
  }
  let members = processGroupMembers(group);
  if (members.length === 0) {
    cleanup.groupQuiescent = true;
    cleanup.leaderQuiescent = true;
    cleanup.status = "quiescent";
    return cleanup;
  }
  if (processIsAlive(group.leaderPid) && !ownedPidMatches(group.leaderPid, group.executable)) {
    cleanup.status = "refused-leader-identity-mismatch";
    return cleanup;
  }
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    try {
      process.kill(-group.pgid, signal);
      cleanup.signals.push(signal);
    } catch (error) {
      if (error.code !== "ESRCH") {
        cleanup.signals.push(`${signal}:error`);
      }
    }
    members = await waitForGroupQuiescence(group, signal === "SIGTERM" ? 5_000 : 2_000);
    if (members.length === 0) {
      cleanup.groupQuiescent = true;
      cleanup.leaderQuiescent = !processIsAlive(group.leaderPid);
      cleanup.status = cleanup.leaderQuiescent ? "quiescent" : "leader-still-alive";
      return cleanup;
    }
  }
  cleanup.status = "owned-group-still-alive";
  return cleanup;
}

const evidence = {
  actions: [],
  app: {},
  cleanup: null,
  error: null,
  fullscreenPolicy: {
    observed: "AXFullScreen=false is required for every visible acceptance state",
    scope: "This proves the owned window did not enter AX fullscreen; it does not enumerate Mission Control Spaces",
  },
  screenshots: {},
  source: {},
  states: {},
  status: "running",
};

let childPid;
let appExecutable;
let qaDir;
let workspacePath;
let runnerTempPath;
let ownedGroup;
let acceptancePassed = false;

function writeEvidence() {
  writeFileSync(path.join(qaDir, "manifest.json"), `${JSON.stringify(evidence, null, 2)}\n`);
}

function recordState(name, snapshot, assertion) {
  evidence.states[name] = assertion;
  evidence.states[name].ax = {
    controls: snapshot.buttons,
    window: snapshot.window,
  };
  writeEvidence();
}

function runScreenPositionHitTest(appPid, assertion) {
  const helperPath = path.join(qaDir, "macos-native-ax-hit-test");
  if (!existsSync(helperPath)) {
    fail("The compiled owned-app AX hit-test helper is missing");
  }
  const output = commandResult(helperPath, [String(appPid), String(TOP_CHROME_BAND), String(LEFT_RESERVED_BAND)], { timeout: COMMAND_TIMEOUT_MS }).stdout.trim();
  let result;
  try {
    result = JSON.parse(output);
  } catch {
    fail("The AX hit-test helper returned invalid evidence");
  }
  if (result.pid !== appPid || result.passed !== true || result.nonOverlapping !== true || result.topChromeBand !== TOP_CHROME_BAND || result.leftReservedBand !== LEFT_RESERVED_BAND) {
    fail("The AX hit-test helper did not prove the owned screen-position contract");
  }
  const controls = new Map(result.controls.map((control) => [control.subrole, control]));
  for (const role of EXPECTED_CONTROL_ROLES) {
    const control = controls.get(role);
    const expectedFrame = assertion.controls?.[role]?.frame;
    if (!control || !control.insideLeftReservedBand || !control.nonOverlapping || control.hitPid !== appPid || control.hitAncestorSubrole !== role || !expectedFrame) {
      fail(`The AX hit-test helper did not resolve ${role} to the owned app`);
    }
    for (const key of ["x", "y", "width", "height"]) {
      if (Math.abs(control.frame[key] - expectedFrame[key]) > 1) {
        fail(`AX and System Events frames disagree for ${role}`);
      }
    }
  }
  return result;
}

function recordVisibleState(name, snapshot, assertion) {
  const hitTest = runScreenPositionHitTest(snapshot.window.bundleProcessId, assertion);
  assertion.hitTest = hitTest;
  for (const role of EXPECTED_CONTROL_ROLES) {
    assertion.controls[role].hittable = true;
  }
  recordState(name, snapshot, assertion);
}

try {
  if (process.platform !== "darwin") {
    fail("This acceptance harness is macOS-only");
  }
  workspacePath = resolveExistingPath(requiredEnv("GITHUB_WORKSPACE"), "GITHUB_WORKSPACE");
  runnerTempPath = resolveExistingPath(requiredEnv("RUNNER_TEMP"), "RUNNER_TEMP");
  qaDir = path.resolve(requiredEnv("WARDIAN_QA_DIR"));
  const isolatedHome = path.resolve(requiredEnv("WARDIAN_HOME"));
  if (!isInside(qaDir, runnerTempPath) || !isInside(isolatedHome, runnerTempPath)) {
    fail("QA output and WARDIAN_HOME must remain under RUNNER_TEMP");
  }
  mkdirSync(qaDir, { recursive: true });
  mkdirSync(isolatedHome, { recursive: true });
  writeEvidence();

  const expectedHead = requiredEnv("WARDIAN_EXPECTED_HEAD_SHA");
  const actualHead = commandResult("/usr/bin/git", ["rev-parse", "HEAD"]).stdout.trim();
  if (!/^[0-9a-f]{40}$/i.test(expectedHead) || actualHead !== expectedHead) {
    fail(`Checkout identity mismatch: expected ${expectedHead}, observed ${actualHead}`);
  }
  evidence.source = { expectedHead, actualHead };

  const config = JSON.parse(readFileSync(path.join(workspacePath, "src-tauri", "tauri.conf.json"), "utf8"));
  const bundleIdentifier = config.identifier;
  if (typeof bundleIdentifier !== "string" || !/^[A-Za-z0-9.-]+$/.test(bundleIdentifier)) {
    fail("The configured Tauri bundle identifier is missing or unsafe");
  }

  const appBundle = resolveExistingPath(requiredEnv("WARDIAN_NATIVE_APP"), "WARDIAN_NATIVE_APP");
  if (!appBundle.endsWith(".app") || !statSync(appBundle).isDirectory() || (!isInside(appBundle, workspacePath) && !isInside(appBundle, runnerTempPath))) {
    fail("WARDIAN_NATIVE_APP must be the single checked-out or runner-temp .app bundle");
  }
  const plistPath = path.join(appBundle, "Contents", "Info.plist");
  const runtimeBundleIdentifier = readPlistValue(plistPath, "CFBundleIdentifier");
  if (runtimeBundleIdentifier !== bundleIdentifier) {
    fail(`Bundle identity mismatch: configured ${bundleIdentifier}, runtime ${runtimeBundleIdentifier}`);
  }
  const executableName = readPlistValue(plistPath, "CFBundleExecutable");
  if (!/^[A-Za-z0-9._-]+$/.test(executableName)) {
    fail("The app bundle executable name is unsafe");
  }
  appExecutable = path.join(appBundle, "Contents", "MacOS", executableName);
  if (!existsSync(appExecutable) || !statSync(appExecutable).isFile()) {
    fail("The app bundle executable is missing");
  }
  const artifactSha256 = sha256File(appExecutable);
  evidence.app = {
    artifactSha256,
    bundleIdentifier,
    executablePath: displayPath(appExecutable, workspacePath, runnerTempPath),
    plistBundleIdentifier: runtimeBundleIdentifier,
  };
  writeEvidence();

  const child = spawn(appExecutable, [], {
    cwd: workspacePath,
    detached: true,
    env: {
      HOME: isolatedHome,
      PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
      TMPDIR: runnerTempPath,
      WARDIAN_E2E_NATIVE_HOME: isolatedHome,
      WARDIAN_HOME: isolatedHome,
    },
    stdio: "ignore",
  });
  childPid = child.pid;
  if (!childPid) {
    fail("The debug app did not provide a process identity");
  }
  child.unref();
  evidence.app.pid = childPid;
  evidence.app.processGroupCleanup = "owned process group only";
  const psCommand = commandResult("/bin/ps", ["-p", String(childPid), "-o", "command="]).stdout.trim();
  if (!psCommand.includes(appExecutable)) {
    fail("The launched PID is not the exact selected app executable");
  }
  ownedGroup = captureOwnedProcessGroup(childPid, appExecutable);
  evidence.app.processGroup = {
    leaderPid: ownedGroup.leaderPid,
    pgid: ownedGroup.pgid,
    scope: "the detached group created for the selected app leader",
  };
  writeEvidence();

  const initial = await waitForSnapshot(bundleIdentifier, childPid, "launch", (snapshot) => snapshot.window.bundleProcessId === childPid);
  recordVisibleState("launched", initial, assertNativeState(initial, "launched", childPid));
  const desktopPng = path.join(qaDir, "desktop-before.png");
  evidence.screenshots.desktopBefore = { file: "desktop-before.png", ...capturePng(desktopPng) };
  const likelyMaximized = initial.window.width >= evidence.screenshots.desktopBefore.width * 0.9 && initial.window.height >= evidence.screenshots.desktopBefore.height * 0.8;
  let baseline = initial;
  if (likelyMaximized) {
    const normalizeAction = runAppleScript(bundleIdentifier, "press-zoom", childPid);
    evidence.actions.push(actionRecord(normalizeAction, "startup-restore", "AXZoomButton", childPid));
    baseline = await waitForSnapshot(bundleIdentifier, childPid, "startup restore", (snapshot) => snapshot.window.bundleProcessId === childPid && !snapshot.window.minimized && snapshot.window.width < initial.window.width * 0.9 && snapshot.window.height < initial.window.height * 0.9);
    recordVisibleState("startup-restored", baseline, assertNativeState(baseline, "startup-restored", childPid));
  }

  const baselineAssertion = assertNativeState(baseline, "baseline", childPid);
  recordVisibleState("baseline", baseline, baselineAssertion);
  evidence.screenshots.baseline = {
    file: "baseline.png",
    ...capturePng(path.join(qaDir, "baseline.png"), baselineAssertion.frame),
  };

  const zoomedAction = runAppleScript(bundleIdentifier, "press-zoom", childPid);
  evidence.actions.push(actionRecord(zoomedAction, "zoom", "AXZoomButton", childPid));
  const zoomed = await waitForSnapshot(bundleIdentifier, childPid, "zoom", (snapshot) => snapshot.window.bundleProcessId === childPid && !snapshot.window.minimized && (snapshot.window.width > baseline.window.width * 1.1 || snapshot.window.height > baseline.window.height * 1.1));
  const zoomedAssertion = assertNativeState(zoomed, "zoom", childPid);
  recordVisibleState("zoomed", zoomed, zoomedAssertion);
  evidence.screenshots.zoomed = {
    file: "zoomed.png",
    ...capturePng(path.join(qaDir, "zoomed.png"), zoomedAssertion.frame),
  };

  const restoreAction = runAppleScript(bundleIdentifier, "press-zoom", childPid);
  evidence.actions.push(actionRecord(restoreAction, "restore", "AXZoomButton", childPid));
  const restored = await waitForSnapshot(bundleIdentifier, childPid, "restore", (snapshot) => snapshot.window.bundleProcessId === childPid && !snapshot.window.minimized && Math.abs(snapshot.window.width - baseline.window.width) <= 12 && Math.abs(snapshot.window.height - baseline.window.height) <= 12 && Math.abs(snapshot.window.x - baseline.window.x) <= 12 && Math.abs(snapshot.window.y - baseline.window.y) <= 12);
  const restoredAssertion = assertNativeState(restored, "restore", childPid);
  recordVisibleState("restored", restored, restoredAssertion);
  evidence.screenshots.restored = {
    file: "restored.png",
    ...capturePng(path.join(qaDir, "restored.png"), restoredAssertion.frame),
  };

  const minimizeAction = runAppleScript(bundleIdentifier, "press-minimize", childPid);
  evidence.actions.push(actionRecord(minimizeAction, "minimize", "AXMinimizeButton", childPid));
  const minimized = await waitForSnapshot(bundleIdentifier, childPid, "minimize", (snapshot) => snapshot.window.bundleProcessId === childPid && snapshot.window.minimized);
  if (minimized.window.fullScreen !== false) {
    fail("minimize: AX fullscreen state became unknown or true");
  }
  recordState("minimized", minimized, {
    fullscreen: minimized.window.fullScreen,
    minimized: minimized.window.minimized,
    pid: minimized.window.bundleProcessId,
  });

  const raiseAction = runAppleScript(bundleIdentifier, "raise", childPid);
  evidence.actions.push(actionRecord(raiseAction, "deminimize-and-raise", "AXWindow", childPid));
  const restoredAfterMinimize = await waitForSnapshot(bundleIdentifier, childPid, "restore after minimize", (snapshot) => snapshot.window.bundleProcessId === childPid && !snapshot.window.minimized);
  recordVisibleState("restored-after-minimize", restoredAfterMinimize, assertNativeState(restoredAfterMinimize, "restore-after-minimize", childPid));

  const closeAction = runAppleScript(bundleIdentifier, "press-close", childPid);
  evidence.actions.push(actionRecord(closeAction, "close-owned-app", "AXCloseButton", childPid));
  const closeDeadline = Date.now() + 12_000;
  let closeOutcome;
  let closeStateError;
  while (Date.now() < closeDeadline) {
    if (!processIsAlive(childPid)) {
      closeOutcome = "closed-by-AXPress";
      break;
    }
    try {
      const processState = runAppleScript(bundleIdentifier, "process-state", childPid);
      if (processState.processId !== childPid) {
        fail("close-owned-app: process identity changed while observing close policy");
      }
      if (!processState.visible || processState.windowCount === 0) {
        closeOutcome = "hidden-by-AXPress";
        break;
      }
    } catch (error) {
      closeStateError = error;
      if (error.fatal) {
        throw error;
      }
    }
    await sleep(250);
  }
  if (!closeOutcome) {
    throw new Error(`close-owned-app: AXPress did not close or hide the owned app${closeStateError ? ` (${closeStateError.message})` : ""}`);
  }
  evidence.app.closeOutcome = closeOutcome;
  acceptancePassed = true;
} catch (error) {
  evidence.status = "failed";
  evidence.error = error instanceof Error ? error.message : String(error);
} finally {
  if (qaDir) {
    try {
      if (ownedGroup) {
        evidence.cleanup = await terminateOwnedProcessGroup(ownedGroup);
      } else if (childPid) {
        evidence.cleanup = { groupQuiescent: false, leaderPid: childPid, signals: [], status: "ownership-not-established" };
      }
      if (acceptancePassed) {
        if (evidence.cleanup?.status === "quiescent" && evidence.cleanup.leaderQuiescent && evidence.cleanup.groupQuiescent) {
          evidence.status = "passed";
        } else {
          evidence.status = "failed";
          evidence.error = evidence.error ?? "Acceptance could not confirm the owned app leader and process group were quiescent";
        }
      }
      writeEvidence();
    } catch (error) {
      evidence.status = "failed";
      evidence.error = evidence.error ?? (error instanceof Error ? error.message : String(error));
      try {
        writeEvidence();
      } catch {
        // Preserve the original failure when the runner itself cannot write evidence.
      }
    }
  }
}

if (evidence.status !== "passed") {
  process.exitCode = 1;
}
