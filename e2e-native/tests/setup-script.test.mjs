import test from "node:test";
import assert from "node:assert/strict";

import {
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
