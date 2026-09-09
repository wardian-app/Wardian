// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { By } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  invokeTauri,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { waitForWorkbenchReady } from "../lib/workbench.mjs";

/**
 * Proves that `wardian browser open` with no URL lands on the workspace's
 * dev server.
 *
 * The ranking and the scan are unit-tested against real sockets already. What
 * only this layer can show is the whole path: a CLI invocation with no address,
 * a control-plane hop, a port read out of a file on disk, a probe against a
 * server that is actually running, and a surface that comes up on the page
 * rather than on `about:blank`.
 */

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const SCREENSHOT_DATE = "2026-08-09";
const TIMEOUT_MS = 60_000;

const FIXTURE = `<!doctype html>
<html><head><title>Workspace dev server</title>
<style>
  body { font-family: system-ui, sans-serif; padding: 2rem; background: #0d1117; color: #e6edf3; }
  code { color: #7ee787; }
</style></head>
<body>
  <h1 id="marker">Workspace dev server</h1>
  <p>Opened by <code>wardian browser open</code> with no address.</p>
</body></html>`;

/** Serves the fixture on an ephemeral loopback port. */
async function serveFixture(t) {
  const server = http.createServer((_request, response) => {
    response.writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      "Content-Length": Buffer.byteLength(FIXTURE),
    });
    response.end(FIXTURE);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  return server.address().port;
}

/** Builds `wardian-cli` and returns its path, matching the other native tests. */
function buildCli(harness) {
  const build = spawnSync("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(build.status, 0, `cargo build -p wardian-cli failed\n${build.stderr}`);
  return freezeBuiltCliForRun(harness);
}

/**
 * Runs the CLI against the same isolated home the app is using.
 *
 * Async on purpose: this process also serves the fixture the detection probes,
 * so a blocking `spawnSync` would deadlock the CLI against its own test server.
 */
function runCli(cliPath, harness, args) {
  const env = { ...process.env, WARDIAN_HOME: harness.isolatedHome };
  delete env.WARDIAN_SESSION_ID;
  return new Promise((resolve, reject) => {
    const child = spawn(cliPath, args, { cwd: harness.repoRoot, env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout, stderr }));
  });
}

async function runCliOk(cliPath, harness, args) {
  const result = await runCli(cliPath, harness, args);
  assert.equal(
    result.status,
    0,
    `wardian ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result.stdout;
}

test("opening a browser with no address lands on the workspace's dev server", async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);

  prepareIsolatedHome(harness);
  const onboarding = path.join(harness.isolatedHome, "settings", "onboarding.json");
  fs.mkdirSync(path.dirname(onboarding), { recursive: true });
  fs.writeFileSync(
    onboarding,
    JSON.stringify({
      dismissed_hint_ids: [],
      contextual_tips_enabled: false,
      guided_tour_state: "skipped",
    }),
  );

  const port = await serveFixture(t);
  // A workspace that declares the port its server is on, which is the signal
  // detection reads before it falls back to the conventional list.
  const workspace = path.join(harness.isolatedHome, "declared-workspace");
  fs.mkdirSync(workspace, { recursive: true });
  fs.writeFileSync(path.join(workspace, ".env"), `PORT=${port}\n`);

  const session = await startNativeSession(harness);
  t.after(async () => { await session?.close(); });
  await waitForAppShell(session.driver, 30_000);
  await waitForWorkbenchReady(session.driver, 30_000);

  const engine = await invokeTauri(session.driver, "browser_engine_status");
  assert.equal(engine.available, true, `no Chromium on this host: ${engine.detail ?? "unknown"}`);

  const cliPath = buildCli(harness);

  // No URL. The address has to come from the workspace.
  const opened = JSON.parse(
    await runCliOk(cliPath, harness, [
      "browser", "--json", "open", "--workspace", workspace,
    ]),
  ).session;
  assert.equal(
    opened.url,
    `http://localhost:${port}/`,
    `expected the declared port, got ${JSON.stringify(opened)}`,
  );

  // `--blank` is how a caller says the blank page was the point.
  const blank = JSON.parse(
    await runCliOk(cliPath, harness, [
      "browser", "--json", "open", "--workspace", workspace, "--blank", "--detached",
    ]),
  ).session;
  assert.notEqual(blank.url, `http://localhost:${port}/`);
  assert.ok(
    blank.url === "" || blank.url.startsWith("about:"),
    `--blank should not detect an address, got ${JSON.stringify(blank.url)}`,
  );

  // The non-detached open surfaces itself, and that surface shows the page.
  const surface = await session.driver.wait(async () => {
    const found = await session.driver.findElements(By.css('[data-testid="browser-surface"]'));
    for (const candidate of found) {
      if (await candidate.isDisplayed()) return candidate;
    }
    return false;
  }, TIMEOUT_MS, "the CLI open never reached a workbench surface");
  assert.equal(await surface.getAttribute("data-resource-key"), opened.browser_id);

  await session.driver.wait(async () => {
    const state = await session.driver.findElement(
      By.css('[data-testid="browser-surface-load-state"]'),
    );
    return (await state.getText()) === "Ready";
  }, TIMEOUT_MS, "the detected page never reported a completed load");

  const address = await session.driver.findElement(
    By.css('[data-testid="browser-surface-address"]'),
  );
  assert.equal(await address.getAttribute("value"), `http://localhost:${port}/`);

  const screenshotDir = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "browser-default-address",
    SCREENSHOT_DATE,
  );
  fs.mkdirSync(screenshotDir, { recursive: true });
  fs.writeFileSync(
    path.join(screenshotDir, "opened-on-dev-server.png"),
    await session.driver.takeScreenshot(),
    "base64",
  );
});
