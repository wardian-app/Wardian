import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";

const providerCommands = {
  claude: "claude",
  codex: "codex",
  gemini: "gemini",
  antigravity: "agy",
  opencode: "opencode",
};

function fakeCommandName(command) {
  if (process.platform === "win32") {
    return `${command}.exe`;
  }
  return command;
}

function seedProviderPath(harness) {
  const binDir = path.join(harness.isolatedHome, "fake-provider-bin");
  fs.mkdirSync(binDir, { recursive: true });
  for (const command of Object.values(providerCommands)) {
    const executable = path.join(binDir, fakeCommandName(command));
    fs.writeFileSync(executable, process.platform === "win32" ? "" : "#!/bin/sh\nexit 0\n", "utf8");
    if (process.platform !== "win32") {
      fs.chmodSync(executable, 0o755);
    }
  }
  return binDir;
}

async function invokeTauri(driver, command, args = {}) {
  const result = await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);

  assert.equal(result.ok, true, `${command} failed: ${result.error}`);
  return result.value;
}

test("native provider readiness reports user-facing provider commands from the app environment", { timeout: 180000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) {
      ensureNativeAppBuilt(harness);
    }
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const providerBin = seedProviderPath(harness);
  const previousPath = process.env.PATH;
  const previousPathExt = process.env.PATHEXT;
  process.env.PATH = [providerBin, previousPath].filter(Boolean).join(path.delimiter);
  if (process.platform === "win32") {
    process.env.PATHEXT = ".EXE;.CMD;.BAT";
  }

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  } finally {
    if (previousPath === undefined) {
      delete process.env.PATH;
    } else {
      process.env.PATH = previousPath;
    }
    if (previousPathExt === undefined) {
      delete process.env.PATHEXT;
    } else {
      process.env.PATHEXT = previousPathExt;
    }
  }

  t.after(async () => {
    await session.close();
  });

  await waitForAppShell(session.driver, 20000);

  const readiness = await invokeTauri(session.driver, "list_provider_readiness");
  const byProvider = new Map(readiness.map((entry) => [entry.provider, entry]));

  assert.deepEqual(
    [...byProvider.keys()],
    ["claude", "codex", "gemini", "antigravity", "opencode"],
  );
  for (const [provider, command] of Object.entries(providerCommands)) {
    const entry = byProvider.get(provider);
    assert.equal(entry.available, true, `${provider} should be available: ${JSON.stringify(entry)}`);
    assert.equal(entry.reason, null);
    assert.ok(
      typeof entry.executable === "string" && entry.executable.length > 0,
      `${provider} should report a resolved executable for ${command}: ${JSON.stringify(entry)}`,
    );
  }
});
