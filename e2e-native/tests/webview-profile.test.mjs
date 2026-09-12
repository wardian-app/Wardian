// @tier ci — Offline environment/ownership regressions; no app or browser.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { acquireHomeLock } from "../lib/sessionHome.mjs";
import { nativeSessionEnvironment, nativeSessionOptions } from "../lib/webviewProfile.mjs";

function fixture(t) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-e2e-native-profile-unit-"));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  return home;
}

test("Windows sessions replace inherited profiles only in the child and never reuse a profile", (t) => {
  const home = fixture(t);
  acquireHomeLock({ home, runId: "owned" });
  const env = { WEBVIEW2_USER_DATA_FOLDER: "default-profile", webview2_user_data_folder: "other-profile", KEEP: "unchanged" };
  const options = { home, runId: "owned", env, platform: "win32" };
  const first = nativeSessionEnvironment(options);
  fs.writeFileSync(path.join(first.WEBVIEW2_USER_DATA_FOLDER, "keep"), "existing session");
  const second = nativeSessionEnvironment(options);
  assert.notEqual(first.WEBVIEW2_USER_DATA_FOLDER, second.WEBVIEW2_USER_DATA_FOLDER);
  for (const child of [first, second]) {
    assert.equal(path.dirname(child.WEBVIEW2_USER_DATA_FOLDER), fs.realpathSync(home));
    assert.ok(fs.statSync(child.WEBVIEW2_USER_DATA_FOLDER).isDirectory());
    assert.equal(child.webview2_user_data_folder, undefined);
    assert.equal(child.KEEP, "unchanged");
  }
  assert.equal(env.WEBVIEW2_USER_DATA_FOLDER, "default-profile");
  assert.equal(env.webview2_user_data_folder, "other-profile");
  assert.equal(fs.readFileSync(path.join(first.WEBVIEW2_USER_DATA_FOLDER, "keep"), "utf8"), "existing session");
});

test("missing or foreign home ownership fails before creating profiles", (t) => {
  const home = fixture(t);
  const options = { home, runId: "mine", platform: "win32", env: {} };
  assert.throws(() => nativeSessionEnvironment(options), /live owned/);
  assert.deepEqual(fs.readdirSync(home), []);
  acquireHomeLock({ home, runId: "other" });
  assert.throws(() => nativeSessionEnvironment(options), /live owned/);
  assert.deepEqual(fs.readdirSync(home), [".native-e2e-lock"]);
});

test("non-Windows sessions preserve environment without profile filesystem access", () => {
  const env = { WEBVIEW2_USER_DATA_FOLDER: "inherited" };
  const child = nativeSessionEnvironment({ home: "not-created", runId: "test", env, platform: "linux" });
  assert.deepEqual(child, { ...env, WARDIAN_HOME: "not-created", WARDIAN_E2E_NATIVE_HOME: "not-created" });
  assert.notEqual(child, env);
});

test("EdgeDriver capability and child environment select the same owned profile", (t) => {
  const home = fixture(t);
  acquireHomeLock({ home, runId: "owned" });
  const options = nativeSessionOptions({ home, runId: "owned", appPath: "app.exe", env: {}, platform: "win32" });
  assert.deepEqual(options.tauriOptions, {
    application: "app.exe",
    webviewOptions: { userDataFolder: options.env.WEBVIEW2_USER_DATA_FOLDER },
  });
  assert.equal(path.dirname(options.tauriOptions.webviewOptions.userDataFolder), fs.realpathSync(home));
  assert.deepEqual(nativeSessionOptions({ home: "unused", appPath: "app", env: {}, platform: "linux" }).tauriOptions, { application: "app" });
});
