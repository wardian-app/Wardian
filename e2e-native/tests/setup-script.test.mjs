// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";

import {
  edgeDriverDownloadUrl,
  msEdgeDriverToolInstallArgs,
  nativeDriverCandidates,
  nativeDriverGuidance,
  parseArgs,
  resolveCommand,
  validateNativeSetupArtifact,
  validateNativeSetupDependencies,
  webview2RuntimeRoots,
} from "../../scripts/setup-native-e2e.mjs";

test("native setup parses skip aliases", () => {
  assert.deepEqual(parseArgs(["--skip-tauri-driver", "--skip-edge-driver"]), {
    skipTauriDriver: true,
    skipNativeDriver: true,
    help: false,
  });
  assert.equal(parseArgs(["--skip-webdriver"]).skipNativeDriver, true);
});

test("native setup rejects unknown options", () => {
  assert.throws(() => parseArgs(["--unknown"]), /Unknown option/);
});

test("native setup resolves commands from PATH entries", () => {
  const delimiter = process.platform === "win32" ? ";" : ":";
  const env = { PATH: [process.cwd(), "C:\\missing"].join(delimiter) };
  assert.equal(resolveCommand("definitely-missing-command", env), null);
});

test("native setup provides driver candidates and guidance for supported platforms", () => {
  assert.ok(nativeDriverCandidates("win32").some((candidate) => candidate.includes("msedgedriver")));
  assert.ok(nativeDriverCandidates("linux").includes("chromedriver"));
  assert.match(nativeDriverGuidance("darwin"), /chromedriver|geckodriver/);
});

test("native setup pins git-sourced msedgedriver helper", () => {
  const args = msEdgeDriverToolInstallArgs();

  assert.deepEqual(args.slice(0, 3), [
    "install",
    "--git",
    "https://github.com/chippers/msedgedriver-tool",
  ]);
  assert.ok(args.includes("--rev"));
  assert.match(args[args.indexOf("--rev") + 1], /^[0-9a-f]{40}$/);
  assert.ok(args.includes("--locked"));
});

test("native setup derives the Edge WebDriver URL from the runtime version", () => {
  assert.equal(
    edgeDriverDownloadUrl("150.0.4078.105", "x64"),
    "https://msedgedriver.microsoft.com/150.0.4078.105/edgedriver_win64.zip",
  );
  assert.equal(
    edgeDriverDownloadUrl("150.0.4078.105", "arm64"),
    "https://msedgedriver.microsoft.com/150.0.4078.105/edgedriver_arm64.zip",
  );
});

test("native setup searches the installed WebView2 roots", () => {
  assert.deepEqual(webview2RuntimeRoots({
    ProgramFiles: "C:\\Program Files",
    "ProgramFiles(x86)": "C:\\Program Files (x86)",
    LOCALAPPDATA: "C:\\Users\\runneradmin\\AppData\\Local",
  }), [
    "C:\\Program Files\\Microsoft\\EdgeWebView\\Application",
    "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application",
    "C:\\Users\\runneradmin\\AppData\\Local\\Microsoft\\EdgeWebView\\Application",
  ]);
});

test("native setup validates package-local Tauri and Selenium dependencies", () => {
  const packagePaths = {
    "@tauri-apps/cli/tauri.js": "C:/fixture/node_modules/@tauri-apps/cli/tauri.js",
    "selenium-webdriver": "C:/fixture/node_modules/selenium-webdriver/index.js",
  };
  const resolved = validateNativeSetupDependencies({
    root: "C:/fixture",
    requireResolve: (packageName) => packagePaths[packageName],
  });
  assert.deepEqual(resolved, packagePaths);
});

test("native setup reports missing package dependencies before driver setup", () => {
  assert.throws(
    () => validateNativeSetupDependencies({
      root: "C:/fixture",
      requireResolve: () => { throw new Error("not installed"); },
    }),
    /Required Node package is unavailable: @tauri-apps\/cli\/tauri\.js/,
  );
});

test("native setup resolves relative app overrides and rejects missing overrides", () => {
  assert.equal(
    validateNativeSetupArtifact({
      root: process.cwd(),
      env: { WARDIAN_NATIVE_APP: "package.json" },
    }),
    path.resolve(process.cwd(), "package.json"),
  );
  assert.throws(
    () => validateNativeSetupArtifact({
      root: process.cwd(),
      env: { WARDIAN_NATIVE_APP: "missing-native-artifact.exe" },
    }),
    /WARDIAN_NATIVE_APP does not exist/,
  );
});
