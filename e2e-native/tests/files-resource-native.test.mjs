import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { By, until } from "selenium-webdriver";

import { fileResourceUrlConversion } from "../../src/features/files/resourceTicketUrl.mjs";
import { closeWorkbenchSurface } from "../lib/workbench.mjs";

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
  const editableFile = path.join(primary, "editable.txt");
  const mismatchFile = path.join(primary, "mismatch.txt");
  const recoveryCleanFile = path.join(primary, "recovery-clean.txt");
  const recoveryConflictFile = path.join(primary, "recovery-conflict.txt");
  const pickerFile = path.join(external, "selected.txt");
  const pickerSibling = path.join(external, "sibling.txt");
  const pdfFile = path.join(primary, "artifact.pdf");
  const imageFile = path.join(primary, "artifact.png");
  const pdfBytes = Buffer.from(
    "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<<>>\n%%EOF\n",
    "utf8",
  );

  fs.writeFileSync(primaryFile, "revision one\n");
  fs.writeFileSync(additionalFile, "additional root\n");
  fs.writeFileSync(editableFile, "save base\n");
  fs.writeFileSync(mismatchFile, "mismatch target\n");
  fs.writeFileSync(recoveryCleanFile, "one\ntwo\nthree\n");
  fs.writeFileSync(recoveryConflictFile, "shared line\n");
  fs.writeFileSync(pickerFile, "selected only\n");
  fs.writeFileSync(pickerSibling, "must stay denied\n");
  fs.writeFileSync(pdfFile, pdfBytes);
  fs.writeFileSync(imageFile, Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAF/gL+Xf6iAAAAAElFTkSuQmCC",
    "base64",
  ));

  return {
    fixtureRoot,
    primary,
    additional,
    external,
    primaryFile,
    additionalFile,
    editableFile,
    mismatchFile,
    recoveryCleanFile,
    recoveryConflictFile,
    pickerFile,
    pickerSibling,
    pdfFile,
    pdfBytes,
    imageFile,
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

async function waitForFileRuntimeStats(driver, resourceId, predicate, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let latest = null;
  while (Date.now() < deadline) {
    latest = await invokeTauri(driver, "debug_file_resource_stats", {
      resourceId,
    });
    if (predicate(latest)) return latest;
    await sleep(100);
  }
  assert.fail(`file runtime stats did not settle: ${JSON.stringify(latest)}`);
}

async function fetchResource(driver, url, { method = "GET", range = null } = {}) {
  const conversion = fileResourceUrlConversion(url);
  return driver.executeAsyncScript((resourceUrl, resourceConversion, resourceMethod, resourceRange, done) => {
    const fetchUrl = resourceConversion
      ? window.__TAURI_INTERNALS__.convertFileSrc(
          resourceConversion.path,
          resourceConversion.protocol,
        )
      : resourceUrl;
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
  }, url, conversion, method, range);
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

test("native Files resources enforce roots, revisions, ranges, and cleanup", { timeout: 360_000 }, async (t) => {
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
  t.after(async () => {
    if (session) await session.close();
  });

  let { driver } = session;
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

  const editableCapture = await startTauriEventCapture(driver, FILE_REVISION_EVENT);
  const editable = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.editableFile, { agentId: SESSION_ID }),
  );
  const mismatch = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.mismatchFile, { agentId: SESSION_ID }),
  );
  const savedText = "saved through retained authorization\n";
  const saved = await invokeTauri(driver, "save_file_resource_text", {
    request: {
      resource_id: editable.resource_id,
      subscription_id: editable.subscription_id,
      expected_revision: editable.revision,
      buffer_base_hash: editable.descriptor.content_hash,
      text: savedText,
      recovery_cleanup: null,
    },
  });
  assert.equal(saved.status, "saved");
  assert.equal(saved.revision, editable.revision + 1);
  assert.equal(saved.content_hash.startsWith("sha256:"), true);
  assert.notEqual(saved.content_hash, editable.descriptor.content_hash);
  assert.equal(fs.readFileSync(fixtures.editableFile, "utf8"), savedText);
  const savedEvent = await waitForTauriEvent(
    driver,
    editableCapture,
    (event) => event.resource_id === editable.resource_id && event.revision === saved.revision,
    15_000,
  );
  assert.equal(savedEvent.descriptor.content_hash, saved.content_hash);
  const savedReadback = await invokeTauri(driver, "read_file_resource_text", {
    request: {
      resource_id: editable.resource_id,
      subscription_id: editable.subscription_id,
      revision: saved.revision,
    },
  });
  assert.equal(savedReadback.text, savedText);
  await sleep(450);
  const savedEvents = (await readTauriEventCapture(driver, editableCapture))
    .filter((event) => event.resource_id === editable.resource_id);
  assert.equal(
    savedEvents.length,
    1,
    `guarded save emitted duplicate events: ${JSON.stringify(savedEvents)}`,
  );
  await stopTauriEventCapture(driver, editableCapture);

  await expectFileError(driver, "save_file_resource_text", {
    request: {
      resource_id: editable.resource_id,
      subscription_id: mismatch.subscription_id,
      expected_revision: saved.revision,
      buffer_base_hash: saved.content_hash,
      text: "must not cross resource ownership\n",
      recovery_cleanup: null,
    },
  }, "unauthorized_resource");
  assert.equal(fs.readFileSync(fixtures.editableFile, "utf8"), savedText);
  assert.equal(fs.readFileSync(fixtures.mismatchFile, "utf8"), "mismatch target\n");

  const staleCapture = await startTauriEventCapture(driver, FILE_REVISION_EVENT);
  const externalText = "external edit wins\n";
  fs.writeFileSync(fixtures.editableFile, externalText);
  const stale = await invokeTauri(driver, "save_file_resource_text", {
    request: {
      resource_id: editable.resource_id,
      subscription_id: editable.subscription_id,
      expected_revision: saved.revision,
      buffer_base_hash: saved.content_hash,
      text: "must not overwrite the external edit\n",
      recovery_cleanup: null,
    },
  });
  assert.equal(stale.status, "stale_conflict");
  assert.ok(stale.revision > saved.revision);
  assert.notEqual(stale.content_hash, saved.content_hash);
  assert.equal(fs.readFileSync(fixtures.editableFile, "utf8"), externalText);
  const staleEvent = await waitForTauriEvent(
    driver,
    staleCapture,
    (event) => event.resource_id === editable.resource_id && event.revision === stale.revision,
    15_000,
  );
  assert.equal(staleEvent.descriptor.content_hash, stale.content_hash);
  await stopTauriEventCapture(driver, staleCapture);

  const revocablePrimary = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.primaryFile, { agentId: SESSION_ID }),
  );
  const revocableAdditional = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.additionalFile, { agentId: SESSION_ID }),
  );
  const restoredAgentConfig = {
    ...agent,
    folder: fixtures.primary,
    include_directories: [fixtures.additional],
  };
  await invokeTauri(driver, "update_agent_config", {
    newConfig: {
      ...restoredAgentConfig,
      folder: fixtures.external,
      include_directories: [],
    },
  });
  for (const opened of [revocablePrimary, revocableAdditional]) {
    await expectFileError(driver, "read_file_resource_text", {
      request: {
        resource_id: opened.resource_id,
        subscription_id: opened.subscription_id,
        revision: opened.revision,
      },
    }, "unauthorized_path");
    await expectFileError(driver, "save_file_resource_text", {
      request: {
        resource_id: opened.resource_id,
        subscription_id: opened.subscription_id,
        expected_revision: opened.revision,
        buffer_base_hash: opened.descriptor.content_hash,
        text: "revoked roots must not write\n",
        recovery_cleanup: null,
      },
    }, "unauthorized_path");
  }
  assert.equal(fs.readFileSync(fixtures.primaryFile, "utf8"), currentText);
  assert.equal(fs.readFileSync(fixtures.additionalFile, "utf8"), "additional root\n");
  await invokeTauri(driver, "update_agent_config", {
    newConfig: restoredAgentConfig,
  });
  const restoredPrimary = await invokeTauri(driver, "read_file_resource_text", {
    request: {
      resource_id: revocablePrimary.resource_id,
      subscription_id: revocablePrimary.subscription_id,
      revision: revocablePrimary.revision,
    },
  });
  const restoredAdditional = await invokeTauri(driver, "read_file_resource_text", {
    request: {
      resource_id: revocableAdditional.resource_id,
      subscription_id: revocableAdditional.subscription_id,
      revision: revocableAdditional.revision,
    },
  });
  assert.equal(restoredPrimary.text, currentText);
  assert.equal(restoredAdditional.text, "additional root\n");

  const recoveryClean = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.recoveryCleanFile, { agentId: SESSION_ID }),
  );
  const recoveryConflict = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.recoveryConflictFile, { agentId: SESSION_ID }),
  );
  await expectFileError(driver, "checkpoint_file_recovery", {
    request: {
      recovery_id: null,
      expected_recovery_revision: null,
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryConflict.subscription_id,
      base_content_hash: recoveryClean.descriptor.content_hash,
      base: "one\ntwo\nthree\n",
      resource_key: recoveryClean.resource_id,
      buffer: "ONE\ntwo\nthree\n",
    },
  }, "unauthorized_resource");
  let cleanCheckpoint = await invokeTauri(driver, "checkpoint_file_recovery", {
    request: {
      recovery_id: null,
      expected_recovery_revision: null,
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryClean.subscription_id,
      base_content_hash: recoveryClean.descriptor.content_hash,
      base: "one\ntwo\nthree\n",
      resource_key: recoveryClean.resource_id,
      buffer: "ONE\ntwo\nthree\n",
    },
  });
  const conflictCheckpoint = await invokeTauri(driver, "checkpoint_file_recovery", {
    request: {
      recovery_id: null,
      expected_recovery_revision: null,
      resource_id: recoveryConflict.resource_id,
      subscription_id: recoveryConflict.subscription_id,
      base_content_hash: recoveryConflict.descriptor.content_hash,
      base: "shared line\n",
      resource_key: recoveryConflict.resource_id,
      buffer: "buffer line\n",
    },
  });
  assert.equal(cleanCheckpoint.recovery_revision, 1);
  assert.equal(conflictCheckpoint.recovery_revision, 1);

  await invokeTauri(driver, "update_agent_config", {
    newConfig: {
      ...restoredAgentConfig,
      folder: fixtures.external,
      include_directories: [],
    },
  });
  cleanCheckpoint = await invokeTauri(driver, "checkpoint_file_recovery", {
    request: {
      recovery_id: cleanCheckpoint.recovery_id,
      expected_recovery_revision: cleanCheckpoint.recovery_revision,
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryClean.subscription_id,
      base_content_hash: recoveryClean.descriptor.content_hash,
      base: "one\ntwo\nthree\n",
      resource_key: recoveryClean.resource_id,
      buffer: "ONE\ntwo\nthree\n",
    },
  });
  assert.equal(cleanCheckpoint.recovery_revision, 2);
  await expectFileError(driver, "checkpoint_file_recovery", {
    request: {
      recovery_id: null,
      expected_recovery_revision: null,
      resource_id: recoveryConflict.resource_id,
      subscription_id: recoveryConflict.subscription_id,
      base_content_hash: recoveryConflict.descriptor.content_hash,
      base: "shared line\n",
      resource_key: recoveryConflict.resource_id,
      buffer: "new recovery must require live file authority\n",
    },
  }, "unauthorized_path");
  await expectFileError(driver, "read_file_resource_text", {
    request: {
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryClean.subscription_id,
      revision: recoveryClean.revision,
    },
  }, "unauthorized_path");
  await expectFileError(driver, "save_file_resource_text", {
    request: {
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryClean.subscription_id,
      expected_revision: recoveryClean.revision,
      buffer_base_hash: recoveryClean.descriptor.content_hash,
      text: "recovery must not authorize this write\n",
      recovery_cleanup: null,
    },
  }, "unauthorized_path");
  await expectFileError(driver, "merge_file_recovery", {
    request: {
      recovery_id: cleanCheckpoint.recovery_id,
      expected_recovery_revision: cleanCheckpoint.recovery_revision,
      resource_key: recoveryClean.resource_id,
      resource_id: recoveryClean.resource_id,
      subscription_id: recoveryClean.subscription_id,
    },
  }, "unauthorized_path");
  assert.equal(fs.readFileSync(fixtures.recoveryCleanFile, "utf8"), "one\ntwo\nthree\n");
  await invokeTauri(driver, "update_agent_config", {
    newConfig: restoredAgentConfig,
  });

  const acceptanceResources = [
    editable,
    mismatch,
    revocablePrimary,
    revocableAdditional,
    recoveryClean,
    recoveryConflict,
  ];
  for (const opened of acceptanceResources) {
    await closeResource(driver, opened.subscription_id);
  }
  for (const opened of acceptanceResources) {
    await waitForFileRuntimeStats(
      driver,
      opened.resource_id,
      (stats) => stats.subscriber_count === 0,
    );
  }
  await waitForFileRuntimeStats(
    driver,
    null,
    (stats) => stats.watcher_count === 0
      && stats.ticket_count === 0
      && stats.renderer_lease_count === 0,
  );

  await session.close();
  session = null;
  session = await startNativeSession(harness);
  driver = session.driver;
  await waitForAppShell(driver, 20_000);

  const listedCleanRecoveries = await invokeTauri(driver, "list_file_recoveries", {
    request: { resource_key: recoveryClean.resource_id },
  });
  const listedConflictRecoveries = await invokeTauri(driver, "list_file_recoveries", {
    request: { resource_key: recoveryConflict.resource_id },
  });
  assert.deepEqual(
    listedCleanRecoveries.map((recovery) => recovery.recovery_id),
    [cleanCheckpoint.recovery_id],
  );
  assert.deepEqual(
    listedConflictRecoveries.map((recovery) => recovery.recovery_id),
    [conflictCheckpoint.recovery_id],
  );
  const restoredCleanRecovery = await invokeTauri(driver, "get_file_recovery", {
    request: {
      recovery_id: cleanCheckpoint.recovery_id,
      resource_key: recoveryClean.resource_id,
    },
  });
  const restoredConflictRecovery = await invokeTauri(driver, "get_file_recovery", {
    request: {
      recovery_id: conflictCheckpoint.recovery_id,
      resource_key: recoveryConflict.resource_id,
    },
  });
  assert.equal(restoredCleanRecovery.base, "one\ntwo\nthree\n");
  assert.equal(restoredCleanRecovery.buffer, "ONE\ntwo\nthree\n");
  assert.equal(restoredConflictRecovery.base, "shared line\n");
  assert.equal(restoredConflictRecovery.buffer, "buffer line\n");

  fs.writeFileSync(fixtures.recoveryCleanFile, "one\ntwo\nTHREE\n");
  fs.writeFileSync(fixtures.recoveryConflictFile, "disk line\n");
  const reopenedClean = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.recoveryCleanFile, { agentId: SESSION_ID }),
  );
  const reopenedConflict = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.recoveryConflictFile, { agentId: SESSION_ID }),
  );
  const cleanMerge = await invokeTauri(driver, "merge_file_recovery", {
    request: {
      recovery_id: cleanCheckpoint.recovery_id,
      expected_recovery_revision: cleanCheckpoint.recovery_revision,
      resource_key: recoveryClean.resource_id,
      resource_id: reopenedClean.resource_id,
      subscription_id: reopenedClean.subscription_id,
    },
  });
  assert.equal(cleanMerge.status, "clean");
  assert.equal(cleanMerge.disk_changed, true);
  assert.equal(cleanMerge.merged_text, "ONE\ntwo\nTHREE\n");
  assert.equal(fs.readFileSync(fixtures.recoveryCleanFile, "utf8"), "one\ntwo\nTHREE\n");

  const conflictedMerge = await invokeTauri(driver, "merge_file_recovery", {
    request: {
      recovery_id: conflictCheckpoint.recovery_id,
      expected_recovery_revision: conflictCheckpoint.recovery_revision,
      resource_key: recoveryConflict.resource_id,
      resource_id: reopenedConflict.resource_id,
      subscription_id: reopenedConflict.subscription_id,
    },
  });
  assert.equal(conflictedMerge.status, "conflicted");
  assert.equal(conflictedMerge.disk_changed, true);
  assert.match(
    conflictedMerge.merged_text,
    /<<<<<<<[\s\S]*buffer line[\s\S]*=======[\s\S]*disk line[\s\S]*>>>>>>>/,
  );
  assert.equal(fs.readFileSync(fixtures.recoveryConflictFile, "utf8"), "disk line\n");

  for (const checkpoint of [cleanCheckpoint, conflictCheckpoint]) {
    const resourceKey = checkpoint.recovery_id === cleanCheckpoint.recovery_id
      ? recoveryClean.resource_id
      : recoveryConflict.resource_id;
    await invokeTauri(driver, "discard_file_recovery", {
      request: {
        recovery_id: checkpoint.recovery_id,
        expected_recovery_revision: checkpoint.recovery_revision,
        resource_key: resourceKey,
      },
    });
    assert.deepEqual(
      await invokeTauri(driver, "list_file_recoveries", {
        request: { resource_key: resourceKey },
      }),
      [],
    );
  }
  await closeResource(driver, reopenedClean.subscription_id);
  await closeResource(driver, reopenedConflict.subscription_id);
  for (const opened of [reopenedClean, reopenedConflict]) {
    await waitForFileRuntimeStats(
      driver,
      opened.resource_id,
      (stats) => stats.subscriber_count === 0,
    );
  }
  await waitForFileRuntimeStats(
    driver,
    null,
    (stats) => stats.watcher_count === 0
      && stats.ticket_count === 0
      && stats.renderer_lease_count === 0,
  );

  const imageIdentity = await invokeTauri(
    driver,
    "open_file_resource",
    request(fixtures.imageFile, { agentId: SESSION_ID }),
  );
  await closeResource(driver, imageIdentity.subscription_id);

  // Persist and reload a real Files surface so this native layer crosses the
  // production FileResourceClient -> ImageRenderer URL conversion boundary.
  // Removing that client conversion makes the image decode fail and this
  // element never reaches a nonzero natural size.
  const loadedWorkbench = await invokeTauri(driver, "load_workbench_state");
  assert.ok(loadedWorkbench.document);
  assert.equal(typeof loadedWorkbench.durable_token, "string");
  const nextRevision = loadedWorkbench.durable_revision + 1;
  const surfaceId = `native-files-image-${RUN_ID}`;
  const groupId = `native-files-group-${RUN_ID}`;
  const savedWorkbench = await invokeTauri(driver, "save_workbench_state", {
    document: {
      schema_version: 1,
      revision: nextRevision,
      saved_at: new Date().toISOString(),
      root: { kind: "group", group_id: groupId },
      groups: {
        [groupId]: {
          group_id: groupId,
          surface_ids: [surfaceId],
          active_surface_id: surfaceId,
        },
      },
      surfaces: {
        [surfaceId]: {
          surface_id: surfaceId,
          surface_type: "files",
          resource_key: imageIdentity.resource_id,
          state_schema_version: 1,
          state: {
            resource_kind: "file",
            mode: "preview",
            transient_preview: false,
            review_drawer_open: false,
            selected_version_id: null,
            optional_checkpoint_id: null,
          },
        },
      },
      active_group_id: groupId,
      recently_closed: [],
      shell: {
        ...loadedWorkbench.document.shell,
        left_sidebar_collapsed: true,
        right_sidebar_collapsed: true,
        bottom_terminal_open: false,
      },
    },
    expected_revision: loadedWorkbench.durable_revision,
    expected_token: loadedWorkbench.durable_token,
    request_id: `native-files-renderer-${RUN_ID}`,
  });
  assert.equal(savedWorkbench.outcome, "saved");
  await driver.navigate().refresh();
  await waitForAppShell(driver, 20_000);
  const renderedImage = await driver.wait(
    until.elementLocated(By.css('img[alt="artifact.png"]')),
    20_000,
  );
  await driver.wait(async () => driver.executeScript(
    "return arguments[0].complete && arguments[0].naturalWidth > 0;",
    renderedImage,
  ), 20_000);
  const renderedSource = await renderedImage.getAttribute("src");
  assert.equal(renderedSource.startsWith("wardian-resource://"), false);
  await closeWorkbenchSurface(driver, "files", imageIdentity.resource_id);
  const finalImageStats = await waitForFileRuntimeStats(
    driver,
    imageIdentity.resource_id,
    (stats) => stats.watcher_count === 0
      && stats.subscriber_count === 0
      && stats.ticket_count === 0
      && stats.renderer_lease_count === 0,
  );
  assert.equal(finalImageStats.user_grant_count, 0);
  const finalTitle = await driver.getTitle();
  assert.equal(typeof finalTitle, "string");
  t.diagnostic("native Files runtime remained live through final cleanup assertion");
});
