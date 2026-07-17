import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  invokeTauri,
  invokeTauriResult,
  prepareIsolatedHome,
  readTauriEventCapture,
  startNativeSession,
  startTauriEventCapture,
  stopTauriEventCapture,
  waitForAppShell,
  waitForTauriEvent,
} from "../lib/harness.mjs";

const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const FILE_REVISION_EVENT = "file-resource://revision";
const RUN_ID = `${process.pid}-${Date.now()}`;
const SESSION_ID = `e2e-files-${RUN_ID}`;
const SESSION_NAME = `E2E-Files-${RUN_ID}`;

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isPathInside(candidate, parent) {
  const relative = path.relative(path.resolve(parent), path.resolve(candidate));
  return relative === "" || (!!relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

function createFixtures(harness) {
  const fixtureRoot = path.resolve(harness.isolatedHome, "files-resource-fixture");
  assert.ok(isPathInside(fixtureRoot, harness.isolatedHome), "fixture root must stay in isolated home");

  const primary = path.join(fixtureRoot, "primary");
  const additional = path.join(fixtureRoot, "additional");
  const external = path.join(fixtureRoot, "picker-external");
  for (const directory of [primary, additional, external]) {
    fs.mkdirSync(directory, { recursive: true });
  }

  const primaryFile = path.join(primary, "report.txt");
  const additionalFile = path.join(additional, "notes.txt");
  const pickerFile = path.join(external, "selected.txt");
  const pickerSibling = path.join(external, "sibling.txt");
  const pdfFile = path.join(primary, "artifact.pdf");
  const pdfBytes = Buffer.from(
    "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<<>>\n%%EOF\n",
    "utf8",
  );

  fs.writeFileSync(primaryFile, "revision one\n");
  fs.writeFileSync(additionalFile, "additional root\n");
  fs.writeFileSync(pickerFile, "selected only\n");
  fs.writeFileSync(pickerSibling, "must stay denied\n");
  fs.writeFileSync(pdfFile, pdfBytes);

  return {
    fixtureRoot,
    primary,
    additional,
    external,
    primaryFile,
    additionalFile,
    pickerFile,
    pickerSibling,
    pdfFile,
    pdfBytes,
  };
}

async function createOffMockAgent(driver, fixtures) {
  return invokeTauri(driver, "spawn_agent", {
    req: {
      sessionName: SESSION_NAME,
      agentClass: "TestClass",
      folder: fixtures.primary,
      resumeSession: SESSION_ID,
      isOff: true,
      configOverride: {
        provider: "mock",
        include_directories: [fixtures.additional],
      },
    },
  });
}

function request(pathValue, { agentId = null, capabilityId = null } = {}) {
  return {
    request: {
      path: pathValue,
      agent_id: agentId,
      user_file_capability_id: capabilityId,
    },
  };
}

async function closeResource(driver, subscriptionId) {
  await invokeTauri(driver, "close_file_resource", {
    request: { subscription_id: subscriptionId },
  });
}

async function expectFileError(driver, command, args, code) {
  const result = await invokeTauriResult(driver, command, args);
  assert.equal(result.ok, false, `${command} unexpectedly succeeded`);
  assert.equal(result.error?.code, code, JSON.stringify(result.error));
  return result.error;
}

async function fetchResource(driver, url, { method = "GET", range = null } = {}) {
  return driver.executeAsyncScript((resourceUrl, resourceMethod, resourceRange, done) => {
    let fetchUrl = resourceUrl;
    try {
      const parsed = new URL(resourceUrl);
      if (parsed.protocol === "wardian-resource:") {
        fetchUrl = window.__TAURI_INTERNALS__.convertFileSrc(
          decodeURIComponent(parsed.pathname).replace(/^\/+/, ""),
          "wardian-resource",
        );
      }
    } catch (error) {
      done({ ok: false, error: `resource URL conversion failed: ${String(error)}` });
      return;
    }
    const headers = resourceRange ? { Range: resourceRange } : undefined;
    fetch(fetchUrl, { method: resourceMethod, headers }).then(async (response) => {
      const bytes = new Uint8Array(await response.arrayBuffer());
      done({
        ok: true,
        fetch_url: fetchUrl,
        status: response.status,
        headers: Object.fromEntries(response.headers.entries()),
        body_base64: bytes.length
          ? btoa(String.fromCharCode(...bytes))
          : "",
      });
    }, (error) => done({ ok: false, fetch_url: fetchUrl, error: String(error) }));
  }, url, method, range);
}

function tryCreateEscapeLink(t, fixtures) {
  try {
    if (process.platform === "win32") {
      const linkDirectory = path.join(fixtures.primary, "escape-link");
      fs.symlinkSync(fixtures.external, linkDirectory, "junction");
      return path.join(linkDirectory, path.basename(fixtures.pickerFile));
    }
    const linkFile = path.join(fixtures.primary, "escape-link.txt");
    fs.symlinkSync(fixtures.pickerFile, linkFile, "file");
    return linkFile;
  } catch (error) {
    if (error?.code === "EPERM" || error?.code === "EACCES") {
      t.diagnostic(`link escape assertion skipped: OS denied link creation (${error.code})`);
      return null;
    }
    throw error;
  }
}

test("native Files resources enforce roots, revisions, ranges, and cleanup", { timeout: 240_000 }, async (t) => {
  const harness = await createNativeHarness();
  assert.ok(harness.appPath);

  try {
    if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }

  prepareIsolatedHome(harness);
  const fixtures = createFixtures(harness);

  let session;
  try {
    session = await startNativeSession(harness);
  } catch (error) {
    t.skip(String(error));
    return;
  }
  t.after(async () => session.close());

  const { driver } = session;
  await waitForAppShell(driver, 20_000);
  const agent = await createOffMockAgent(driver, fixtures);
  assert.equal(agent.session_id, SESSION_ID);
  assert.deepEqual(agent.include_directories, [fixtures.additional]);

  const systemRoot = agent.system_include_directories
    ?.map((directory) => path.resolve(directory))
    .find((directory) => isPathInside(directory, harness.isolatedHome));
  assert.ok(systemRoot, "mock agent must expose an isolated system include root");
  fs.mkdirSync(systemRoot, { recursive: true });
  const systemFile = path.join(systemRoot, "internal-context.txt");
  fs.writeFileSync(systemFile, "system context must stay private\n");

  const escapeFile = tryCreateEscapeLink(t, fixtures);
  const capture = await startTauriEventCapture(driver, FILE_REVISION_EVENT);

  const primaryTrusted = await invokeTauri(driver, "open_file_resource", request(fixtures.primaryFile));
  const primaryExplicit = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.primaryFile, { agentId: SESSION_ID }),
  );
  assert.equal(primaryTrusted.resource_id, primaryExplicit.resource_id);
  assert.equal(primaryTrusted.revision, 1);
  assert.notEqual(primaryTrusted.subscription_id, primaryExplicit.subscription_id);

  const sharedStats = await invokeTauri(driver, "debug_file_resource_stats", {
    resourceId: primaryTrusted.resource_id,
  });
  assert.equal(sharedStats.watcher_count, 1);
  assert.equal(sharedStats.subscriber_count, 2);

  for (const authorization of [{}, { agentId: SESSION_ID }]) {
    const additional = await invokeTauri(
      driver,
      "open_file_resource",
      request(fixtures.additionalFile, authorization),
    );
    await closeResource(driver, additional.subscription_id);
  }

  for (const authorization of [{}, { agentId: SESSION_ID }]) {
    await expectFileError(
      driver,
      "open_file_resource",
      request(systemFile, authorization),
      "unauthorized_path",
    );
    if (escapeFile) {
      await expectFileError(
        driver,
        "open_file_resource",
        request(escapeFile, authorization),
        "unauthorized_path",
      );
    }
  }

  const grant = await invokeTauri(driver, "debug_grant_file_resource_for_e2e", {
    path: fixtures.pickerFile,
  });
  const picked = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.pickerFile, { capabilityId: grant.capability_id }),
  );
  const restoredPicked = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.pickerFile),
  );
  assert.equal(picked.resource_id, restoredPicked.resource_id);
  await expectFileError(
    driver,
    "open_file_resource",
    request(fixtures.pickerSibling, { capabilityId: grant.capability_id }),
    "unauthorized_path",
  );
  await expectFileError(
    driver,
    "open_file_resource",
    request(fixtures.pickerSibling),
    "unauthorized_path",
  );
  await closeResource(driver, picked.subscription_id);
  await closeResource(driver, restoredPicked.subscription_id);

  fs.writeFileSync(fixtures.primaryFile, "revision two, first write\n");
  await sleep(40);
  fs.writeFileSync(fixtures.primaryFile, "revision two, second write\n");
  await sleep(40);
  const currentText = "revision two, stable final\n";
  fs.writeFileSync(fixtures.primaryFile, currentText);

  const revision = await waitForTauriEvent(
    driver,
    capture,
    (event) => event.resource_id === primaryTrusted.resource_id && event.revision === 2,
    15_000,
  );
  assert.equal(revision.descriptor.content_hash.startsWith("sha256:"), true);
  await sleep(450);
  let primaryEvents = (await readTauriEventCapture(driver, capture))
    .filter((event) => event.resource_id === primaryTrusted.resource_id);
  assert.equal(primaryEvents.length, 1, JSON.stringify(primaryEvents));

  fs.writeFileSync(fixtures.primaryFile, currentText);
  await sleep(450);
  primaryEvents = (await readTauriEventCapture(driver, capture))
    .filter((event) => event.resource_id === primaryTrusted.resource_id);
  assert.equal(primaryEvents.length, 1, "unchanged write emitted a revision");
  await stopTauriEventCapture(driver, capture);

  await expectFileError(driver, "read_file_resource_text", {
    request: {
      resource_id: primaryExplicit.resource_id,
      subscription_id: primaryExplicit.subscription_id,
      revision: 1,
    },
  }, "stale_revision");
  const text = await invokeTauri(driver, "read_file_resource_text", {
    request: {
      resource_id: primaryExplicit.resource_id,
      subscription_id: primaryExplicit.subscription_id,
      revision: 2,
    },
  });
  assert.equal(text.text, currentText);

  const pdf = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.pdfFile, { agentId: SESSION_ID }),
  );
  assert.equal(pdf.descriptor.renderer_kind, "pdf");
  const rendererLeaseId = `native-pdf-${RUN_ID}`;
  const ticket = await invokeTauri(driver, "issue_file_resource_ticket", {
    request: {
      resource_id: pdf.resource_id,
      subscription_id: pdf.subscription_id,
      revision: pdf.revision,
      renderer_lease_id: rendererLeaseId,
    },
  });

  const full = await fetchResource(driver, ticket.url);
  assert.equal(full.ok, true, JSON.stringify(full));
  assert.deepEqual(
    { status: full.status, body_base64: full.body_base64 },
    { status: 200, body_base64: fixtures.pdfBytes.toString("base64") },
    JSON.stringify(full),
  );
  if (full.headers["accept-ranges"] !== undefined) {
    assert.equal(full.headers["accept-ranges"], "bytes");
  } else {
    t.diagnostic("WebView2 did not expose custom-protocol response headers to fetch");
  }

  const head = await fetchResource(driver, ticket.url, { method: "HEAD" });
  assert.equal(head.ok, true, head.error);
  assert.equal(head.status, 200);
  assert.equal(head.body_base64, "");
  if (head.headers["content-length"] !== undefined) {
    assert.equal(Number(head.headers["content-length"]), fixtures.pdfBytes.length);
  }
  t.diagnostic("custom protocol HEAD completed");

  const rangeStart = 5;
  const rangeEnd = 13;
  const ranged = await fetchResource(driver, ticket.url, {
    range: `bytes=${rangeStart}-${rangeEnd}`,
  });
  assert.equal(ranged.ok, true, ranged.error);
  assert.equal(ranged.status, 206);
  assert.equal(
    ranged.body_base64,
    fixtures.pdfBytes.subarray(rangeStart, rangeEnd + 1).toString("base64"),
  );
  if (ranged.headers["content-range"] !== undefined) {
    assert.equal(
      ranged.headers["content-range"],
      `bytes ${rangeStart}-${rangeEnd}/${fixtures.pdfBytes.length}`,
    );
  }
  t.diagnostic("custom protocol range GET completed");

  const invalidRange = await fetchResource(driver, ticket.url, {
    range: `bytes=${fixtures.pdfBytes.length + 20}-`,
  });
  assert.equal(invalidRange.ok, true, invalidRange.error);
  assert.equal(invalidRange.status, 416);
  if (invalidRange.headers["content-range"] !== undefined) {
    assert.equal(invalidRange.headers["content-range"], `bytes */${fixtures.pdfBytes.length}`);
  }
  t.diagnostic("custom protocol 416 completed");

  await invokeTauri(driver, "close_file_renderer_lease", {
    request: {
      resource_id: pdf.resource_id,
      subscription_id: pdf.subscription_id,
      renderer_lease_id: rendererLeaseId,
    },
  });
  const leaseClosedStats = await invokeTauri(driver, "debug_file_resource_stats", {
    resourceId: pdf.resource_id,
  });
  assert.equal(leaseClosedStats.ticket_count, 0);
  assert.equal(leaseClosedStats.renderer_lease_count, 0);
  assert.equal(leaseClosedStats.subscriber_count, 1);

  await closeResource(driver, pdf.subscription_id);
  await closeResource(driver, primaryTrusted.subscription_id);
  let retainedStats = await invokeTauri(driver, "debug_file_resource_stats", {
    resourceId: primaryExplicit.resource_id,
  });
  assert.equal(retainedStats.watcher_count, 1);
  assert.equal(retainedStats.subscriber_count, 1);

  await closeResource(driver, primaryExplicit.subscription_id);
  retainedStats = await invokeTauri(driver, "debug_file_resource_stats", {
    resourceId: primaryExplicit.resource_id,
  });
  assert.equal(retainedStats.watcher_count, 0);
  assert.equal(retainedStats.subscriber_count, 0);
  assert.equal(retainedStats.ticket_count, 0);
  assert.equal(retainedStats.renderer_lease_count, 0);
  assert.equal(retainedStats.user_grant_count, 1);
  const finalTitle = await driver.getTitle();
  assert.equal(typeof finalTitle, "string");
  t.diagnostic("native Files runtime remained live through final cleanup assertion");
});
