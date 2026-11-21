// @tier nightly — Opt-in frozen-artifact benchmark; never build or time a mocked backend.
// WARDIAN_E2E_CHAT_PREVIEW_TIMING=1 WARDIAN_CHAT_PREVIEW_SEED_EXE=<compiled-libtest-exe> \
// WARDIAN_NATIVE_APP=<frozen-app> WARDIAN_NATIVE_CLI=<matching-frozen-cli> \
// npm run test:e2e:native:fast -- e2e-native/tests/chat-preview-identity-native.test.mjs
// Optional module override: export async function seedStoppedChatPreviewSource({ harness, fixture, mode }).
// It must use the backend acquisition owner, NOT write invented capture/projection DTOs.
// An optional beforeColdOpen({ harness, fixture, session }) can release a backend-owned fixture barrier.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { By } from "selenium-webdriver";
import { chromium } from "@playwright/test";
import { createNativeHarness, assertNativePreflight, prepareIsolatedHome, startNativeSession, waitForAppShell, invokeTauri } from "../lib/harness.mjs";
import { closeConformanceSession } from "../lib/conformance-cleanup.mjs";
import { releaseHomeLock } from "../lib/sessionHome.mjs";
import { startRemoteClientTlsFixture } from "../lib/remoteClientTlsFixture.mjs";
import {
  CHAT_PREVIEW_BUDGET_MS, CHAT_PREVIEW_SEED_TIMEOUT_MS, chatPreviewWatchdogMs,
  STOPPED_SOURCE_CAPABILITY, stageSyntheticStoppedSource, verifySyntheticSource, pendingCaptureWitness, seedNativePreviewSource,
  trustedClickWithPendingCapture,
  chatTimingReceipt, chatTimingCandidateDetails, chatPreviewObserverScript, assertChatTimingSample, recordChatTimingCase, writeChatTimingReport,
  recordRejectedChatScope,
} from "../lib/chatPreviewTiming.mjs";

const enabled = process.env.WARDIAN_E2E_CHAT_PREVIEW_TIMING === "1";
const diagnostic = process.env.WARDIAN_CHAT_PREVIEW_DIAGNOSTIC === "1";
const requestedFixtureModes = (process.env.WARDIAN_CHAT_PREVIEW_FIXTURES ?? "pending,qualified").split(",");
// Invalid selectors retain the maximum watchdog until the existing preflight reports them.
const watchdogMs = chatPreviewWatchdogMs(requestedFixtureModes.length === 1 ? 1 : 2);
const probe = "synthetic-unsent-timing-probe";

function endpointResult(snapshot, options) {
  const result = assertChatTimingSample(snapshot, options);
  // Concurrent-build diagnostics prove the ruler and product path, not a budget.
  return diagnostic ? { diagnostic_only: true, readable: true, usable: true, usability_probe: result.usability_probe,
    evidence_level: result.evidence_level, renderer_at_readable: result.renderer_at_readable, transition: result.transition } : result;
}

async function eventually(operation, timeoutMs = 15_000) {
  const end = performance.now() + timeoutMs;
  while (performance.now() < end) {
    const value = await operation();
    if (value) return value;
    await delay(25);
  }
  throw Object.assign(new Error("Owned fixture readiness deadline expired"), { code: "fixture_readiness_timeout" });
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const { port } = server.address();
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

async function startOwnedSession(harness, fixture) {
  const overrides = { CODEX_HOME: path.join(fixture.providerHome, ".codex"), CLAUDE_CONFIG_DIR: path.join(fixture.providerHome, ".claude"),
    WARDIAN_NATIVE_SESSION_START_ATTEMPTS: "1" };
  fs.mkdirSync(overrides.CODEX_HOME); fs.mkdirSync(overrides.CLAUDE_CONFIG_DIR);
  const previous = Object.fromEntries(Object.keys(overrides).map((key) => [key, process.env[key]]));
  Object.assign(process.env, overrides);
  try { return await startNativeSession(harness); }
  finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  }
}

async function desktopEndpoint(driver, field, timeoutMs) {
  const result = await driver.executeAsyncScript((name, timeout, done) => {
    window.__chatPreviewTiming.wait(name, timeout).then(() => done({ ok: true }),
      (error) => done({ ok: false, error: String(error.message) }));
  }, field, timeoutMs);
  assert.ok(result.ok, result.error);
}

async function closeDesktopChat(driver, fixture) {
  const close = await driver.executeScript((owner) => {
    const candidates = [...document.querySelectorAll('[data-agent-grid-card-id], [data-testid="agent-session-surface"]')]
      .filter((node) => (node.getAttribute("data-agent-grid-card-id") === owner || node.getAttribute("data-resource-key") === owner)
        && node.getBoundingClientRect().width > 0);
    if (candidates.length !== 1) return null;
    const panel = candidates[0].closest('[data-testid="surface-panel"]');
    const host = panel?.closest('[data-testid="workbench-host"]');
    const tabs = [...(host?.querySelectorAll('[role="tab"][aria-selected="true"]') ?? [])]
      .filter((tab) => tab.getAttribute("data-surface-id") === panel.getAttribute("data-surface-id"));
    return tabs.length === 1 ? tabs[0].querySelector("[data-tab-close]") : null;
  }, fixture.sessionId);
  assert.ok(close, "Cannot resolve the actual owned Chat parent tab for normal warm reopen");
  await close.click();
}

async function measureDesktop(driver, fixture, label, report, save, options, afterUsable) {
  await driver.executeScript((name) => window.__chatPreviewTiming.arm(name), label);
  return recordChatTimingCase(report, label, async () => {
    const roster = await driver.findElement(By.css(`[aria-label="Agent ${fixture.name}"]`));
    const pending = report.cases.at(-1).capture = {};
    await trustedClickWithPendingCapture(options.pendingFixture, () => roster.click(), pending);
    process.stdout.write(`[chat-preview-timing] trusted click active: ${label}\n`);
    const mode = await driver.wait(() => driver.executeScript(() => window.__chatPreviewTiming.modeControl()), 5_000);
    report.cases.at(-1).activation = { normal_chat_toggle_required: mode.needsToggle, clock_origin: "trusted_roster_click" };
    if (mode.needsToggle) await mode.node.click();
    // Normal mode activation remains inside the original trusted Roster clock.
    await desktopEndpoint(driver, "readableAt", 5_000);
    const plan = await driver.wait(() => driver.executeScript(() => window.__chatPreviewTiming.prepareInteraction()),
      5_000, "No safe owned control appeared before the interaction deadline", 25);
    assert.ok(plan?.node, "No safe owned control for the original painted body");
    if (plan.kind === "input") await plan.node.sendKeys(probe);
    else {
      assert.equal(plan.kind, "wheel");
      await driver.actions({ async: true }).scroll(0, 0, 0, plan.delta, plan.node).perform();
    }
    await desktopEndpoint(driver, "usableAt", 5_000);
    await afterUsable?.();
    if (options.requireQualified) await desktopEndpoint(driver, "qualifiedAt", 30_000);
    return endpointResult(await driver.executeScript(() => window.__chatPreviewTiming.snapshot()), options);
  }, () => driver.executeScript(() => window.__chatPreviewTiming.snapshot()), save);
}

async function pairPwa(page, context, session, origin, report) {
  const offer = await invokeTauri(session.driver, "create_remote_pairing_offer");
  assert.equal(offer.gateway_origin, origin);
  const params = new URLSearchParams({ pairing_offer_id: offer.pairing_offer_id, nonce: offer.nonce,
    server_fingerprint: offer.server_identity_fingerprint });
  const submitted = page.waitForResponse((response) => response.url() === `${origin}/remote/api/pairing/submit` && response.status() === 202);
  await page.goto(`${origin}/remote?${params}`, { waitUntil: "domcontentloaded" });
  const pending = await (await submitted).json();
  const requests = await invokeTauri(session.driver, "list_pending_remote_pairing_requests");
  const request = requests.find((item) => item.request_id === pending.pairing_request_id);
  assert.ok(request && request.canonical_origin === origin && request.credential_kind === "pwa" && request.audience === "wardian_remote_pwa"
    && request.public_key_fingerprint === pending.public_key_fingerprint && request.client_origin === null,
  "PWA pairing must match the actual browser-submitted key and native pending request");
  await invokeTauri(session.driver, "approve_remote_pairing_request", { requestId: request.request_id });
  await page.getByTestId("remote-watchlist-view").waitFor({ state: "visible", timeout: 20_000 });
  const cookie = (await context.cookies(origin)).find((item) => item.name === "__Host-wardian_remote_session");
  assert.ok(cookie?.secure && cookie.httpOnly && cookie.sameSite === "Strict", "Real PWA cookie session was not established");
  report.pwa_pairing = { browser_key_generated_by_product: true, cookie_session: true, secure: true,
    http_only: true, same_site: cookie.sameSite, bearer_used: false };
}

async function measureRemote(page, fixture, label, report, save, options, afterUsable) {
  await page.evaluate((name) => window.__chatPreviewTiming.arm(name), label);
  return recordChatTimingCase(report, label, async () => {
    const pending = report.cases.at(-1).capture = {};
    await trustedClickWithPendingCapture(options.pendingFixture,
      () => page.getByRole("button", { name: `Open ${fixture.name} details`, exact: true }).click(), pending);
    process.stdout.write(`[chat-preview-timing] trusted click active: ${label}\n`);
    await page.evaluate(() => window.__chatPreviewTiming.wait("readableAt", 5_000));
    const handle = await page.waitForFunction(() => window.__chatPreviewTiming.prepareInteraction(), null, { timeout: 5_000, polling: "raf" });
    try {
      const kind = await (await handle.getProperty("kind")).jsonValue();
      const node = (await handle.getProperty("node")).asElement();
      assert.ok(node, "No safe owned PWA control for original painted body");
      if (kind === "input") await node.type(probe);
      else {
        assert.equal(kind, "wheel");
        const bounds = await node.boundingBox();
        assert.ok(bounds);
        const delta = await (await handle.getProperty("delta")).jsonValue();
        await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
        await page.mouse.wheel(0, delta);
      }
    } finally { await handle.dispose(); }
    await page.evaluate(() => window.__chatPreviewTiming.wait("usableAt", 5_000));
    await afterUsable?.();
    if (options.requireQualified) await page.evaluate(() => window.__chatPreviewTiming.wait("qualifiedAt", 30_000));
    return endpointResult(await page.evaluate(() => window.__chatPreviewTiming.snapshot()), options);
  }, () => page.evaluate(() => window.__chatPreviewTiming.snapshot()), save);
}

test("real desktop and native-gateway PWA preview-to-qualified opening timing", { skip: !enabled, timeout: watchdogMs }, async () => {
  assert.equal(process.env.WARDIAN_NATIVE_SKIP_BUILD, "1", "Use the prebuilt fast runner; this benchmark never builds");
  assert.ok(process.env.WARDIAN_NATIVE_APP && process.env.WARDIAN_NATIVE_CLI, "Explicit frozen app and paired CLI are required");
  const base = await createNativeHarness();
  prepareIsolatedHome(base);
  const file = path.join(base.isolatedHome, "artifacts", "chat-preview-identity", "result.json");
  process.stdout.write(`[chat-preview-timing] evidence: ${file}\n`);
  const report = { schema_version: 1, status: "RUNNING", cases: [], surfaces: [],
    deadlines: { seed_setup_ms: CHAT_PREVIEW_SEED_TIMEOUT_MS, outer_watchdog_ms: watchdogMs,
      opening_budget_ms: CHAT_PREVIEW_BUDGET_MS, qualified_wait_ms: 30_000 },
    timing_qualification: diagnostic ? "DIAGNOSTIC_ONLY_NOT_BENCHMARK" : "ISOLATED_BENCHMARK_ATTEMPT",
    artifact: { app_sha256: base.frozenArtifacts.app.sha256, cli_sha256: base.frozenArtifacts.cli.sha256 },
    cache: "Independent backend process/home and browser profile per surface; OS filesystem cache warmed by fixture generation/hash/read. Remote pairing warms shell/SW, not Chat detail.",
    limits: ["two-animation-frame DOM witnesses, not physical display scanout", "native supervisor closure is not a full caller birth/EOF/Job-empty qualification receipt",
      "page-loader request observation begins after the desktop shell; earlier startup IPC is unobserved",
      "Missing data-chat-page-scope leaves consumer admission unproven; qualified renderer DOM still requires actual source-backed turn and response identity"],
  };
  const save = () => writeChatTimingReport(file, report);
  const failures = [];
  let taskFailure;
  try {
    assertNativePreflight(base);
    report.driver_artifacts = { native_webdriver_sha256: createHash("sha256").update(fs.readFileSync(base.nativeDriverPath)).digest("hex"),
      tauri_driver_sha256: createHash("sha256").update(fs.readFileSync(base.tauriDriverPath)).digest("hex") };
    const manifestPath = process.env.WARDIAN_CHAT_PREVIEW_ARTIFACT_MANIFEST;
    if (manifestPath) {
      assert.ok(path.isAbsolute(manifestPath) && fs.statSync(manifestPath).size <= 64 * 1_024, "Bounded absolute artifact manifest required");
      const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
      assert.equal(manifest.frozen.app.sha256, report.artifact.app_sha256, "Runtime app does not match the frozen source manifest");
      assert.equal(manifest.frozen.cli.sha256, report.artifact.cli_sha256, "Runtime CLI does not match the frozen source manifest");
      report.artifact.source_commit = manifest.sourceCommit;
      report.artifact.profile = manifest.profile;
    }
    report.harness_source_sha256 = createHash("sha256").update(fs.readFileSync(new URL(import.meta.url)))
      .update(fs.readFileSync(new URL("../lib/chatPreviewTiming.mjs", import.meta.url))).digest("hex");
    const adapterPath = process.env.WARDIAN_CHAT_PREVIEW_SEED_MODULE;
    const seedExe = process.env.WARDIAN_CHAT_PREVIEW_SEED_EXE;
    let adapter;
    if (adapterPath) {
      assert.ok(path.isAbsolute(adapterPath), "Seed adapter path must be absolute");
      assert.ok(fs.statSync(adapterPath).size <= 4 * 1024 * 1024, "Seed adapter exceeds bounded source size");
      report.seed_adapter_sha256 = createHash("sha256").update(fs.readFileSync(adapterPath)).digest("hex");
      adapter = await import(pathToFileURL(adapterPath).href);
    } else if (seedExe) {
      adapter = { seedStoppedChatPreviewSource: ({ harness, fixture, mode }) => seedNativePreviewSource({ harness, fixture, mode,
        executable: mode === "qualified" ? process.env.WARDIAN_CHAT_PREVIEW_QUALIFY_SEED_EXE ?? seedExe : seedExe }) };
    } else throw Object.assign(new Error(STOPPED_SOURCE_CAPABILITY), { code: "missing_stopped_source_capability" });
    assert.equal(typeof adapter.seedStoppedChatPreviewSource, "function", "Seed module must export seedStoppedChatPreviewSource");
    const modes = requestedFixtureModes;
    assert.ok(modes.length <= 2 && new Set(modes).size === modes.length && modes.every((mode) => ["pending", "qualified"].includes(mode)), "Unknown or duplicate fixture mode");
    const pendingTurns = Number(process.env.WARDIAN_CHAT_PREVIEW_PENDING_TURNS ?? 12_000);
    const pendingPadding = Number(process.env.WARDIAN_CHAT_PREVIEW_PENDING_PADDING ?? 1_024);
    assert.ok(Number.isSafeInteger(pendingTurns) && pendingTurns >= 12_000 && pendingTurns <= 20_000, "Pending history must contain 12000..20000 turns");
    report.fixture_modes = modes;
    for (const { surface, mode } of modes.flatMap((mode) => ["desktop", "remote"].map((surface) => ({ mode, surface })))) {
      const harness = { ...base, isolatedHome: path.join(base.isolatedHome, `wardian-e2e-native-${surface}-${mode}`), homeLock: undefined,
        appPath: base.frozenArtifacts.app.path, sharedCliPath: base.frozenArtifacts.cli.path, ownsGeneratedHome: false };
      const detail = { surface, fixture_mode: mode, status: "RUNNING", cleanup: {}, websocket: { status_frames_received: 0 }, errors: [] };
      report.surfaces.push(detail);
      let session; let browser; let page; let tls; let startupAttempted = false; let fixture;
      const pendingObservations = new Set();
      let observing = true;
      try {
        prepareIsolatedHome(harness);
        fixture = stageSyntheticStoppedSource(harness, mode === "qualified" ? { turns: 128, padding: 128 }
          : { turns: pendingTurns, padding: pendingPadding });
        assert.ok(mode !== "pending" || fixture.sourceBytes >= 4 * 1024 * 1024, "Pending fixture must retain a genuine large source");
        detail.fixture = { history_turns: mode === "qualified" ? 128 : pendingTurns, padding: mode === "qualified" ? 128 : pendingPadding,
          records: fixture.records, source_bytes: fixture.sourceBytes };
        detail.source = verifySyntheticSource(fixture);
        detail.seed_receipt = await adapter.seedStoppedChatPreviewSource({ harness, fixture, mode });
        if (mode === "pending") detail.seed_capture = pendingCaptureWitness(fixture);
        let gatewayPort;
        if (surface === "remote") {
          const sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: base.repoRoot, encoding: "utf8" }).trim();
          const diff = execFileSync("git", ["diff", "--binary", "HEAD"], { cwd: base.repoRoot });
          const sourceDeltaDigest = createHash("sha256").update(diff).digest("hex");
          tls = await startRemoteClientTlsFixture({ sourceCommit, sourceDeltaDigest, allowPwaWebSocketUpgrades: true });
          gatewayPort = await freePort();
          fs.mkdirSync(path.join(harness.isolatedHome, "remote-access"));
          fs.writeFileSync(path.join(harness.isolatedHome, "remote-access", "config.json"), JSON.stringify({ schema_version: 1,
            enabled: true, canonical_origin: tls.ready.gateway_origin, loopback_host: "127.0.0.1", loopback_port: gatewayPort,
            gateway_identity_public_key: "synthetic-fixture-public-key", gateway_identity_fingerprint: "synthetic-fixture-fingerprint" }));
          tls.setUpstream({ host: "127.0.0.1", port: gatewayPort });
          detail.tls = { certificate_sha256: tls.ready.certificate_sha256, pwa_websocket_forwarding: true };
        }
        startupAttempted = true;
        session = await startOwnedSession(harness, fixture);
        await waitForAppShell(session.driver, 30_000);
        // Leave transport headroom for the observer's 30-second qualification deadline.
        await session.driver.manage().setTimeouts({ script: 40_000 });
        const agent = (await invokeTauri(session.driver, "list_agents")).find((item) => item.session_id === fixture.sessionId);
        detail.restored_agent = agent ? { provider: agent.provider, is_off: agent.is_off, session_name: agent.session_name, synthetic: true } : null;
        assert.ok(agent?.provider === "codex" && agent.is_off === true && agent.session_name === fixture.name,
          "The real native roster must restore the owned stopped Codex agent");
        const observerConfig = { surface, sessionId: fixture.sessionId, provider: fixture.provider, name: fixture.name, body: fixture.body, probe,
          synthetic: fixture.synthetic, sourceKey: fixture.sourceKey, targetTurn: fixture.targetTurn,
          targetMessageId: fixture.targetMessageId, captureDetails: diagnostic };
        if (surface === "desktop") {
          await session.driver.executeScript(chatPreviewObserverScript(observerConfig));
        } else {
          await eventually(async () => {
            try { return (await fetch(`http://127.0.0.1:${gatewayPort}/remote/api/health`, { signal: AbortSignal.timeout(500) })).ok; }
            catch { return false; }
          });
          browser = await chromium.launchPersistentContext(path.join(harness.isolatedHome, "pwa-profile"), {
            headless: true, viewport: { width: 430, height: 920 },
            args: [`--ignore-certificate-errors-spki-list=${tls.ready.certificate_spki_sha256_base64}`],
          });
          await browser.addInitScript({ content: chatPreviewObserverScript(observerConfig) });
          // This is the product's persisted view preference; it does not pre-open or prefetch Chat.
          await browser.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
          page = await browser.newPage();
          const observe = (operation) => {
            const task = operation().catch((error) => { if (observing) detail.errors.push(String(error.message).slice(0, 256)); })
              .finally(() => pendingObservations.delete(task));
            pendingObservations.add(task);
          };
          const chatPath = `/remote/api/agents/${fixture.sessionId}/chat`;
          page.on("request", (request) => {
            const url = new URL(request.url());
            if (url.pathname === chatPath) observe(() => page.evaluate((details) => window.__chatPreviewTiming.noteRequest(details),
              { method: request.method(), path: url.pathname, query: url.search }));
          });
          page.on("response", (response) => {
            if (new URL(response.url()).pathname === chatPath && response.ok()) observe(async () => {
              const dto = await response.json();
              const details = diagnostic ? chatTimingCandidateDetails(dto, fixture) : null;
              let receipt; let receiptFailure;
              try { receipt = chatTimingReceipt(dto, fixture); }
              catch (error) {
                receiptFailure = error;
                recordRejectedChatScope(detail, error, { at: performance.now(), clock_basis: "node:performance.now" });
              }
              if (receiptFailure) {
                let diagnosticFailure;
                try { if (details) await page.evaluate((value) => window.__chatPreviewTiming.noteCandidateDetails(value), details); }
                catch (error) { diagnosticFailure = error; }
                if (diagnosticFailure) throw new AggregateError([receiptFailure, diagnosticFailure],
                  "API receipt rejection and diagnostic retention failed", { cause: receiptFailure });
                throw receiptFailure;
              }
              await page.evaluate((value) => window.__chatPreviewTiming.receive(value.receipt, value.details), { receipt, details });
            });
          });
          page.on("websocket", (socket) => {
            if (new URL(socket.url()).pathname === "/remote/api/status-stream") socket.on("framereceived", () => detail.websocket.status_frames_received++);
          });
          await pairPwa(page, browser, session, tls.ready.gateway_origin, detail);
        }
        await adapter.beforeColdOpen?.({ harness, fixture, session, mode });
        if (mode === "pending") detail.cold_open_capture = pendingCaptureWitness(fixture);
        const measure = (label, cold) => {
          const options = { requireQualified: mode === "qualified", requireAdmitted: true,
            pendingFixture: mode === "pending" ? fixture : null,
            ...(diagnostic ? { budgetMs: Number.MAX_SAFE_INTEGER } : {}),
            requireTransition: !diagnostic && mode === "qualified" && cold && process.env.WARDIAN_CHAT_PREVIEW_REQUIRE_TRANSITION === "1" };
          const afterUsable = () => adapter.afterUsable?.({ harness, fixture, session, mode, surface, label });
          return surface === "desktop" ? measureDesktop(session.driver, fixture, label, report, save, options, afterUsable)
            : measureRemote(page, fixture, label, report, save, options, afterUsable);
        };
        const openings = diagnostic ? ["diagnostic-first-chat", ...(process.env.WARDIAN_CHAT_PREVIEW_DIAGNOSTIC_WARM === "1"
          ? ["diagnostic-same-process-warm-reopen"] : [])] : ["cold-process-first-chat", "same-process-warm-reopen"];
        for (const [index, temperature] of openings.entries()) {
          if (index === 1) {
            if (surface === "desktop") await closeDesktopChat(session.driver, fixture);
            else await page.getByRole("button", { name: "Back to remote agents", exact: true }).click();
          }
          try { await measure(`${surface}-${mode}-${temperature}`, index === 0); }
          catch (error) { failures.push(error); detail.errors.push({ code: error.code ?? "error", message: String(error.message).slice(0, 512) }); }
        }
        if (surface === "remote") assert.ok(detail.websocket.status_frames_received > 0, "No real forwarded PWA status-stream frame observed");
        assert.deepEqual(verifySyntheticSource(fixture), detail.source, "Stopped source changed during opening measurements");
        await Promise.allSettled([...pendingObservations]);
        assert.equal(detail.errors.length, 0, "A timing or passive response observation failed; see retained partials");
        detail.status = "OBSERVED_UI_ENDPOINTS";
      } catch (error) {
        detail.status = "FAILED"; detail.errors.push({ code: error.code ?? "error", message: String(error.message).slice(0, 1_024) }); failures.push(error);
      } finally {
        observing = false;
        await Promise.allSettled([...pendingObservations]);
        for (const [label, operation] of [
          ["observer", async () => page ? page.evaluate(() => window.__chatPreviewTiming?.dispose())
            : session?.driver.executeScript(() => window.__chatPreviewTiming?.dispose())],
          ["browser", async () => browser?.close()],
          ["session", async () => { if (session) await closeConformanceSession(session); else assert.ok(!startupAttempted, "Unreturned native startup remains supervisor-owned"); }],
          ["tls", async () => tls?.close()],
        ]) {
          try { detail.cleanup[label] = await operation() ?? "closed_or_not_started"; }
          catch (error) { detail.cleanup[label] = "UNPROVEN"; failures.push(error); }
        }
        if (Object.values(detail.cleanup).some((value) => value === "UNPROVEN")) detail.status = "FAILED_CLEANUP";
        if (Object.values(detail.cleanup).every((value) => value !== "UNPROVEN")) releaseHomeLock({ home: harness.isolatedHome, runId: harness.runId });
        await save();
      }
    }
    report.status = failures.length ? "FAILED" : modes.includes("qualified")
      ? "OBSERVED_UI_ENDPOINTS_CLOSURE_LIMITED" : "OBSERVED_OPENING_ONLY_QUALIFICATION_UNRUN";
  } catch (error) {
    taskFailure = error;
    report.status = "FAILED";
    report.error = { code: error.code ?? "error", message: String(error.message).slice(0, 2_048) };
  }
  // A surface's cleanup/report error must not replace failures collected before it.
  const retainedFailures = [...new Set([...failures, ...(taskFailure ? [taskFailure] : [])])];
  if (retainedFailures.length) {
    report.status = "FAILED";
    report.error ??= { code: "chat_preview_failed", message: "Chat preview measurements failed; inspect retained result.json partials" };
  }
  try { await save(); }
  catch (error) { retainedFailures.push(error); }
  if (retainedFailures.length === 1) throw retainedFailures[0];
  if (retainedFailures.length > 1) throw new AggregateError(retainedFailures,
    "Chat preview measurements or failure-report persistence failed", { cause: retainedFailures[0] });
});
