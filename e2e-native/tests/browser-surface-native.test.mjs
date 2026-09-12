// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { By, Key, until } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  freezeBuiltCliForRun,
  invokeTauri,
  invokeTauriResult,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";
import { openWorkbenchSurface, waitForWorkbenchReady } from "../lib/workbench.mjs";

/**
 * Exercises the browser surface against a real Chromium.
 *
 * This has to be native. Browser E2E can mock the session client, but it
 * cannot prove that a Chromium starts, that CDP drives it, that a screencast
 * frame reaches the surface, or that a ref taken before a navigation is
 * refused afterwards. Those are the claims the feature rests on.
 */

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const SCREENSHOT_DATE = "2026-08-09";
const INPUT_SCREENSHOT_DATE = "2026-08-24";

const FIXTURE = `<!doctype html>
<html><head><title>Wardian Browser Surface</title>
<style>
  body { font-family: system-ui, sans-serif; padding: 2rem; background: #0d1117; color: #e6edf3; }
  input, button { font-size: 1rem; padding: .5rem .75rem; margin-right: .5rem; }
  #out { margin-top: 1rem; color: #7ee787; }
</style></head>
<body>
  <h1 id="heading">Agent browser surface</h1>
  <p>A page an agent drives through <code>wardian browser</code>.</p>
  <input id="q" placeholder="Search" />
  <button id="go" onclick="document.getElementById('out').textContent = 'searched for ' + document.getElementById('q').value">Go</button>
  <p id="out"></p>
  <p><a id="next" href="/second">Second page</a></p>
  <!--
    A window this page opens, and a dialog that stops it. Both used to be
    dead ends: the popup ran in a target nothing was attached to, and the
    dialog held the renderer with nobody able to answer it.
  -->
  <p><a id="popup" href="/second" target="_blank">Open in a new window</a></p>
  <p><button id="ask" onclick="document.getElementById('out').textContent = 'confirmed ' + window.confirm('Proceed?')">Ask</button></p>
  <!--
    One request that 404s, so the ledger has a failure to report and the
    surface footer has something to count. Introspection is the point of the
    phase this fixture now also covers.
  -->
  <script>fetch('/api/missing').catch(() => {});</script>
</body></html>`;

const SECOND = `<!doctype html>
<html><head><title>Second page</title></head>
<body style="font-family: system-ui; background:#0d1117; color:#e6edf3; padding:2rem">
<h1 id="marker">Second page</h1></body></html>`;

/** Serves the fixture on an ephemeral loopback port. */
async function serveFixture(t) {
  const server = http.createServer((request, response) => {
    if (request.url?.startsWith("/api/missing")) {
      const body = JSON.stringify({ error: "nope" });
      response.writeHead(404, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(body),
      });
      response.end(body);
      return;
    }
    const body = request.url?.startsWith("/second") ? SECOND : FIXTURE;
    response.writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      "Content-Length": Buffer.byteLength(body),
    });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  return `http://127.0.0.1:${server.address().port}/`;
}

async function requireInvoke(driver, command, args = {}) {
  return await invokeTauri(driver, command, args);
}

/** Builds `wardian-cli` and returns its path, matching the CLI shared-state test. */
function buildCli(harness) {
  const build = spawnSync("cargo", ["build", "-p", "wardian-cli", "--bin", "wardian-cli"], {
    cwd: harness.repoRoot,
    encoding: "utf8",
  });
  assert.equal(build.status, 0, `cargo build -p wardian-cli failed
${build.stderr}`);
  return freezeBuiltCliForRun(harness);
}

/**
 * Runs the CLI against the same isolated home the app is using.
 *
 * Deliberately async: `spawnSync` would block this process's event loop, and
 * this process also serves the fixture the page navigates to. Blocking here
 * would deadlock the CLI against the server it is waiting on.
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
    `wardian ${args.join(" ")} failed
stdout:
${result.stdout}
stderr:
${result.stderr}`,
  );
  return result.stdout;
}

test("browser surface drives a real page and refuses stale refs", async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);

  prepareIsolatedHome(harness);
  // A fresh home shows the guided tour, whose backdrop would intercept every
  // workbench click this test makes.
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
  const baseUrl = await serveFixture(t);

  const session = await startNativeSession(harness);
  t.after(async () => { await session?.close(); });
  await waitForAppShell(session.driver, 30_000);
  const { driver } = session;

  const engine = await requireInvoke(driver, "browser_engine_status");
  assert.equal(
    engine.available,
    true,
    `no Chromium on this host: ${engine.detail ?? "unknown"}`,
  );

  // Open through the launcher so the provisioning path is what is under test:
  // the contribution has to create its own session before the surface opens.
  await waitForWorkbenchReady(driver, 30_000);
  await openWorkbenchSurface(driver, "browser", undefined, { timeoutMs: 40_000 });

  const surface = await driver.wait(async () => {
    const found = await driver.findElements(By.css('[data-testid="browser-surface"]'));
    for (const candidate of found) if (await candidate.isDisplayed()) return candidate;
    return false;
  }, 60_000, "the browser surface never rendered");

  const browserId = await surface.getAttribute("data-resource-key");
  assert.ok(browserId, "the surface must carry the session it presents");

  // Navigate through the CLI rather than the Tauri command: surface-originated
  // mutations require the drive lease the surface itself holds, and the
  // control-plane path is the one an agent actually uses.
  const cliPath = buildCli(harness);
  await runCliOk(cliPath, harness, ["browser", browserId, "navigate", baseUrl]);

  // A frame proves the screencast path end to end: CDP event, backend
  // broadcast, Tauri event, and the surface's own render.
  await driver.wait(async () => {
    const frames = await driver.findElements(By.css('[data-testid="browser-surface-frame"]'));
    return frames.length > 0 && await frames[0].isDisplayed();
  }, 60_000, "no screencast frame reached the surface");

  await driver.wait(async () => {
    const state = await driver.findElement(By.css('[data-testid="browser-surface-load-state"]'));
    return (await state.getText()) === "Ready";
  }, 60_000, "the page never reported a completed load");

  const address = await driver.findElement(By.css('[data-testid="browser-surface-address"]'));
  assert.match(await address.getAttribute("value"), /127\.0\.0\.1/);

  const shortRef = await driver.findElement(By.css('[data-testid="browser-surface-short-ref"]'));
  assert.match(await shortRef.getText(), /^browser:\d+$/);

  // The page is rendered at the size of the pane showing it. Without this the
  // browser stays at its 1280x800 default and every frame is a rescaled
  // picture of a layout nobody chose.
  const pane = await driver.findElement(By.css('[data-testid="browser-surface-viewport"]'));
  const [paneWidth, paneHeight] = await driver.executeScript(
    "const rect = arguments[0].getBoundingClientRect();"
    + " return [Math.round(rect.width), Math.round(rect.height)];",
    pane,
  );
  assert.ok(paneWidth > 0 && paneHeight > 0, "the pane had no size to match");
  await driver.wait(async () => {
    const reported = await driver.findElement(
      By.css('[data-testid="browser-surface-viewport-size"]'),
    );
    return (await reported.getText()) === `${paneWidth}×${paneHeight}`;
  }, 30_000, `the page was never resized to the pane (${paneWidth}x${paneHeight})`);

  // Keystrokes forwarded by the surface, including the editing keys that
  // carry no text. Only this layer covers the whole path: a real DOM key
  // event, the surface's forwarder, the Tauri command, and CDP's own
  // synthesis inside a real renderer. The click is not decoration — it is
  // what gives the pane DOM focus, and keys reach the forwarder only then.
  await driver
    .actions()
    .move({
      origin: pane,
      x: -Math.round(paneWidth / 3),
      y: -Math.round(paneHeight / 3),
    })
    .click()
    .perform();
  await runCliOk(cliPath, harness, [
    "browser", browserId, "eval", "document.getElementById('q').focus()",
  ]);
  await driver.actions().sendKeys("wardiann", Key.BACK_SPACE).perform();
  await driver.wait(async () => {
    const typed = await runCliOk(cliPath, harness, [
      "browser", browserId, "eval", "document.getElementById('q').value",
    ]);
    // The trailing character is the one Backspace has to remove; a surface
    // that forwards keys without a virtual key code leaves it there.
    return typed.includes("wardian") && !typed.includes("wardiann");
  }, 20_000, "typing and Backspace did not reach the page through the surface");

  // PR evidence for the input and fidelity fixes: live controls, a page laid
  // out at the pane's own size, and a field holding what was typed *after* a
  // correction.
  const inputEvidenceDir = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "browser-surface-input",
    INPUT_SCREENSHOT_DATE,
  );
  fs.mkdirSync(inputEvidenceDir, { recursive: true });
  fs.writeFileSync(
    path.join(inputEvidenceDir, "editing-keys-and-pane-sized-viewport.png"),
    await driver.takeScreenshot(),
    "base64",
  );

  // A window the page opens. Only this layer can show what used to happen:
  // the popup ran in a target nothing was attached to, so the surface kept
  // showing an opener that would never change — which is every OAuth flow.
  const popupSnapshot = JSON.parse(
    await runCliOk(cliPath, harness, ["browser", "--json", browserId, "snapshot", "--interactive"]),
  ).snapshot;
  const popupLink = popupSnapshot.elements.find(
    (element) => element.name.trim() === "Open in a new window",
  );
  assert.ok(popupLink, `the popup link was not in the snapshot: ${JSON.stringify(popupSnapshot.elements)}`);
  await runCliOk(cliPath, harness, ["browser", browserId, "click", popupLink.element_ref]);

  const popupChip = await driver.wait(
    until.elementLocated(By.css('[data-testid="browser-surface-popup"]')),
    30_000,
    "the popup was never presented on the surface",
  );
  assert.match(await popupChip.getText(), /popup/i);
  await driver.wait(async () => {
    const address = await driver.findElement(By.css('[data-testid="browser-surface-address"]'));
    return (await address.getAttribute("value")).includes("/second");
  }, 30_000, "the address bar never followed the popup");

  const popupEvidenceDir = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "browser-surface-dialogs-and-popups",
    INPUT_SCREENSHOT_DATE,
  );
  fs.mkdirSync(popupEvidenceDir, { recursive: true });
  fs.writeFileSync(
    path.join(popupEvidenceDir, "popup-presented-over-its-opener.png"),
    await driver.takeScreenshot(),
    "base64",
  );

  // The popup's own history has no entry for its opener, so this button is
  // the only way back.
  await driver.findElement(By.css('[aria-label="Close popup"]')).click();
  await driver.wait(async () => {
    const chips = await driver.findElements(By.css('[data-testid="browser-surface-popup"]'));
    return chips.length === 0;
  }, 30_000, "the surface never returned to the page behind the popup");

  // A dialog stops the renderer. Before it was surfaced, the page simply
  // stopped answering and there was nothing to click.
  await runCliOk(cliPath, harness, [
    "browser", browserId, "eval", "setTimeout(() => document.getElementById('ask').click(), 0)",
  ]);
  const dialogMessage = await driver.wait(
    until.elementLocated(By.css('[data-testid="browser-surface-dialog-message"]')),
    30_000,
    "the dialog never reached the surface",
  );
  assert.match(await dialogMessage.getText(), /Proceed\?/);

  fs.writeFileSync(
    path.join(popupEvidenceDir, "page-dialog-waiting-for-an-answer.png"),
    await driver.takeScreenshot(),
    "base64",
  );

  await driver.findElement(By.css('[data-testid="browser-surface-dialog-accept"]')).click();
  await driver.wait(async () => {
    const answered = await runCliOk(cliPath, harness, [
      "browser", browserId, "eval", "document.getElementById('out').textContent",
    ]);
    // The page resumed *with the answer it was given*, which is the whole
    // point of holding the dialog rather than dismissing it unseen.
    return answered.includes("confirmed true");
  }, 30_000, "the page never resumed after the dialog was answered");

  // The surface holds the lease, so its controls are live rather than inert.
  const reload = await driver.findElement(By.css('[aria-label="Reload"]'));
  assert.equal(
    await reload.getAttribute("disabled"),
    null,
    "the only attached surface must hold the drive lease",
  );

  // A mutation without the lease token is refused by the backend, not merely
  // hidden by the frontend.
  const forged = await invokeTauriResult(driver, "navigate_browser_session", {
    browserId,
    action: "about:blank",
    leaseToken: "not-a-real-lease",
  });
  assert.equal(forged.ok, false, "a forged lease token must not navigate the page");
  assert.match(
    String(forged.error?.message ?? forged.error),
    /read-only|drive lease/i,
    `expected a lease refusal, got: ${JSON.stringify(forged.error)}`,
  );

  // The agent path: the same CLI a human uses, against the app's own runtime.
  const listed = await runCliOk(cliPath, harness, ["browser", "list"]);
  assert.match(listed, /browser:\d+/, `browser list did not show the session:
${listed}`);

  const snapshotJson = JSON.parse(
    await runCliOk(cliPath, harness, ["browser", "--json", browserId, "snapshot", "--interactive"]),
  );
  const elements = snapshotJson.snapshot.elements;
  const searchBox = elements.find((element) => element.name === "Search");
  const goButton = elements.find((element) => element.name.trim() === "Go");
  assert.ok(searchBox, `the search box was not in the snapshot: ${JSON.stringify(elements)}`);
  assert.ok(goButton, `the button was not in the snapshot: ${JSON.stringify(elements)}`);
  assert.equal(
    Object.hasOwn(searchBox, "checked"),
    false,
    "a text input must not report a checked state",
  );

  await runCliOk(cliPath, harness, ["browser", browserId, "fill", searchBox.element_ref, "wardian"]);
  await runCliOk(cliPath, harness, ["browser", browserId, "click", goButton.element_ref]);
  const outcome = await runCliOk(cliPath, harness, ["browser", browserId, "get", "text", "#out"]);
  assert.match(outcome, /searched for wardian/, `the page handler did not run:
${outcome}`);

  // A ref minted before a navigation must be refused, not applied to whatever
  // now occupies that position.
  const staleRef = goButton.element_ref;
  await runCliOk(cliPath, harness, ["browser", browserId, "navigate", `${baseUrl}second`]);
  await runCliOk(cliPath, harness, [
    "browser", browserId, "wait", "--url-contains", "/second", "--timeout-ms", "10000",
  ]);
  const stale = await runCli(cliPath, harness, [
    "browser", "--json", browserId, "click", staleRef,
  ]);
  assert.notEqual(stale.status, 0, "a stale ref must not succeed");
  assert.equal(
    JSON.parse(stale.stdout || stale.stderr).error.code,
    "snapshot_stale",
    `expected snapshot_stale, got:
${stale.stdout}
${stale.stderr}`,
  );

  await runCliOk(cliPath, harness, ["browser", browserId, "navigate", baseUrl]);
  await driver.wait(async () => {
    const state = await driver.findElement(By.css('[data-testid="browser-surface-load-state"]'));
    return (await state.getText()) === "Ready";
  }, 30_000, "the page never reloaded after the stale-ref check");

  // Introspection over real IPC. The engine-backed Rust tests already prove
  // the runtime; what only this layer can prove is that the new control-plane
  // variants round-trip between the CLI and the app, where a serde tag
  // mismatch would compile cleanly and fail at runtime.
  let failed = [];
  await driver.wait(async () => {
    const listed = await runCliOk(cliPath, harness, [
      "browser", "--json", browserId, "network", "--failed",
    ]);
    failed = JSON.parse(listed).network.entries ?? [];
    return failed.length > 0;
  }, 30_000, "the network ledger never recorded the failing request");
  assert.equal(failed[0].status, 404, `expected the 404, got ${JSON.stringify(failed[0])}`);

  const detail = JSON.parse(
    await runCliOk(cliPath, harness, [
      "browser", "--json", browserId, "network", failed[0].request_id, "--body",
    ]),
  ).network.detail;
  assert.equal(detail.entry.status, 404);
  assert.equal(detail.response_headers["content-type"], "application/json");

  await runCliOk(cliPath, harness, ["browser", browserId, "cookies", "set", "sid", "abc"]);
  const cookies = JSON.parse(
    await runCliOk(cliPath, harness, ["browser", "--json", browserId, "cookies"]),
  ).cookies;
  assert.equal(
    cookies.find((cookie) => cookie.name === "sid")?.value,
    "abc",
    `the cookie did not round-trip: ${JSON.stringify(cookies)}`,
  );

  await runCliOk(cliPath, harness, [
    "browser", browserId, "storage", "local", "set", "theme", "dark",
  ]);
  const stored = JSON.parse(
    await runCliOk(cliPath, harness, ["browser", "--json", browserId, "storage", "local", "theme"]),
  ).storage;
  assert.equal(stored.value, "dark", `web storage did not round-trip: ${JSON.stringify(stored)}`);

  // The surface reads the count off the session summary, so this also proves
  // the ledger reached the frontend rather than only the CLI.
  const failures = await driver.wait(
    until.elementLocated(By.css('[data-testid="browser-surface-network-failures"]')),
    30_000,
    "the surface never reported the failed request",
  );
  assert.match(await failures.getText(), /failed request/);

  const screenshotDir = path.join(
    harness.repoRoot,
    "e2e",
    "screenshots",
    "agent-browser-surface",
    SCREENSHOT_DATE,
  );
  fs.mkdirSync(screenshotDir, { recursive: true });
  fs.writeFileSync(
    path.join(screenshotDir, "browser-surface.png"),
    await driver.takeScreenshot(),
    "base64",
  );
});
