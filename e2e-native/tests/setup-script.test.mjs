import test from "node:test";
import assert from "node:assert/strict";

import {
  msEdgeDriverToolInstallArgs,
  nativeDriverCandidates,
  nativeDriverGuidance,
  parseArgs,
  resolveCommand,
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
