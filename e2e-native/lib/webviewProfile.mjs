import fs from "node:fs";
import path from "node:path";
import { lockHolderAlive, readHomeLock } from "./sessionHome.mjs";

/**
 * Give each Windows driver/app session a fresh WebView2 profile inside its
 * claimed home. The child-only override replaces Tauri's shared LocalData
 * default without changing the parent environment or deleting any profile.
 */
export function nativeSessionEnvironment({ home, runId, env = process.env, platform = process.platform }) {
  const childEnv = { ...env, WARDIAN_HOME: home, WARDIAN_E2E_NATIVE_HOME: home };
  if (platform !== "win32") return childEnv;

  const owner = readHomeLock(home);
  if (!runId || owner?.runId !== runId || !lockHolderAlive(owner.pid)) {
    throw new Error("Refusing to create a WebView2 profile without a live owned native home lock");
  }
  const realHome = fs.realpathSync(home);
  const profile = fs.mkdtempSync(path.join(realHome, ".webview2-"));
  // Windows environment names are case-insensitive; never leave a differently
  // cased inherited override competing with the session's owned profile.
  for (const key of Object.keys(childEnv)) {
    if (key.toUpperCase() === "WEBVIEW2_USER_DATA_FOLDER") delete childEnv[key];
  }
  childEnv.WEBVIEW2_USER_DATA_FOLDER = profile;
  return childEnv;
}

/** EdgeDriver selects its own temporary profile unless this capability is set. */
export function nativeSessionOptions({ appPath, platform = process.platform, ...options }) {
  const env = nativeSessionEnvironment({ ...options, platform });
  return {
    env,
    tauriOptions: {
      application: appPath,
      ...(platform === "win32"
        ? { webviewOptions: { userDataFolder: env.WEBVIEW2_USER_DATA_FOLDER } }
        : {}),
    },
  };
}
