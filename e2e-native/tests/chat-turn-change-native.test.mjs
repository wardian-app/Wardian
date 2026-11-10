// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { By, Key } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  invokeTauri,
} from "../lib/harness.mjs";

const { chatAcceptanceFixture } = createRequire(import.meta.url)("../../scripts/mock-agent.cjs");

/**
 * Passive WebView instrumentation. It makes no IPC calls and never dispatches
 * input. Keep this named boundary executable by the app-free harness tests.
 */
function installChatObservation(config) {
  const state = { owner: null, phase: null, phases: [], calls: [], receipts: [], presentations: [], errors: [], intents: [], captureCostMs: 0 };
  const bounded = (value) => typeof value === "string" && value.length <= 1024 ? value : null;
  const surface = () => Array.from(document.querySelectorAll('[data-testid="agent-session-surface"]'))
    .find((node) => node.getAttribute("data-resource-key") === state.owner);
  const now = () => performance.now();
  let inputEvidence = null;
  let wheelEvidence = null;
  const fail = (error) => { if (state.errors.length < 8) state.errors.push(String(error).slice(0, 160)); };
  const record = (command, args, dto, returnedAt) => {
    if (command !== "load_agent_chat_page" || !dto || !Array.isArray(dto.events)) return;
    if (state.receipts.length >= 128 || dto.events.length > 384) throw new Error("passive receipt bound exceeded");
    if (dto.recent_admission != null && (typeof dto.recent_admission !== "string"
      || !dto.recent_admission || dto.recent_admission.length > 1024)) throw new Error("invalid recent admission");
    state.receipts.push({
      at: returnedAt, session: bounded(dto.session_id), conversation: bounded(dto.conversation_id),
      generation: bounded(dto.generation), epoch: bounded(dto.source_epoch), revision: bounded(dto.revision),
      admission: dto.recent_admission == null ? null : bounded(dto.recent_admission),
      progress: bounded(dto.progress), older: dto.older_page, reset: dto.reset,
      before: bounded(args?.cursor), next: bounded(dto.next_before), detail: Boolean(dto.detail),
      rows: dto.events.map((row) => ({
        id: bounded(row.id), key: bounded(row.metadata?.chat_display_key) ?? bounded(row.id),
        session: bounded(row.session_id), provider: bounded(row.provider), role: row.role, kind: row.kind,
        turn: bounded(row.turn_id), text: typeof row.text === "string" ? row.text.slice(0, 96) : null,
        length: typeof row.text === "string" ? row.text.length : null,
        target: row.kind === "message" && row.role === "assistant" && row.provider === "mock"
          && row.text === config.body,
      })),
    });
  };
  const original = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = function (...args) {
    const result = Reflect.apply(original, this, args);
    const command = args[0];
    if (command === "load_agent_chat_page" || command === "load_agent_chat_transcript") {
      if (state.calls.length >= 128) fail("passive call bound exceeded");
      else state.calls.push({ command, at: now(), session: bounded(args[1]?.sessionId),
        before: bounded(args[1]?.cursor), detail: bounded(args[1]?.detailRef),
        revalidate: Array.isArray(args[1]?.revalidateIds) });
      // Attach directly to the original Promise. Do not return a chained Promise,
      // await it, clone a result, or turn a rejection into a successful value.
      try { result.then((dto) => {
        const started = now();
        try { record(command, args[1], dto, started); } catch (error) { fail(error); }
        finally { state.captureCostMs += now() - started; }
      },
        () => {}); } catch (error) { fail(error); }
    }
    return result;
  };
  const visibleBody = () => {
    const root = surface();
    const scroll = root?.querySelector('[data-testid="agent-chat-scroll-region"]');
    if (!scroll) return null;
    const region = scroll.getBoundingClientRect();
    const contents = Array.from(root.querySelectorAll('article[aria-label="assistant message"] .chat-message-content'));
    for (const content of contents) {
      if (content.textContent !== config.body) continue;
      const key = content.closest("[data-chat-row-key]")?.getAttribute("data-chat-row-key");
      const receipt = state.receipts.findLast((item) => item.session === state.owner
        && item.revision && item.generation && item.rows.some((row) => row.target
          && row.session === state.owner && row.key === key));
      if (!receipt || !key) continue;
      // Measure the actual body text, not the row/header of a tall message.
      const range = document.createRange();
      range.selectNodeContents(content);
      const rects = Array.from(range.getClientRects());
      if (!rects.some((rect) => rect.width > 0 && rect.height > 0
        && rect.bottom > Math.max(0, region.top) && rect.top < Math.min(innerHeight, region.bottom)
        && rect.right > Math.max(0, region.left) && rect.left < Math.min(innerWidth, region.right))) continue;
      return { key, receiptAt: receipt.at, revision: receipt.revision, generation: receipt.generation,
        epoch: receipt.epoch, conversation: receipt.conversation, admission: receipt.admission,
        matchingBodySlots: contents.filter((node) => node.textContent === config.body).length };
    }
    return null;
  };
  const checkUsable = (phase) => {
    if (!phase || phase.chatAt === null || phase.usableAt !== null || phase.usableChecking || !visibleBody()) return;
    const valid = () => (inputEvidence?.phase === phase
      && inputEvidence.node.value === config.probe && document.activeElement === inputEvidence.node)
      || (wheelEvidence?.phase === phase && wheelEvidence.node.scrollTop !== wheelEvidence.before);
    if (!valid()) return;
    phase.usableChecking = true;
    requestAnimationFrame(() => requestAnimationFrame(() => {
      phase.usableChecking = false;
      if (state.phase === phase && valid() && visibleBody()) phase.usableAt = now();
    }));
  };
  const sample = () => {
    const presented = visibleBody();
    if (presented && state.presentations.length < 64
      && state.presentations.at(-1)?.revision !== presented.revision) {
      state.presentations.push({ at: now(), ...presented });
    }
    const phase = state.phase;
    checkUsable(phase);
    if (!phase || phase.chatAt === null || phase.paintAt !== null || phase.checking) return;
    const body = presented;
    if (!body || body.receiptAt > now()) return;
    phase.checking = true;
    const commitAt = now();
    requestAnimationFrame(() => requestAnimationFrame(() => {
      phase.checking = false;
      if (state.phase !== phase) return;
      const current = visibleBody();
      if (current && current.key === body.key && current.revision === body.revision) {
        phase.commitAt = commitAt;
        phase.paintAt = now();
        phase.body = current;
      }
    }));
  };
  document.addEventListener("click", (event) => {
    if (!event.isTrusted || !state.phase) return;
    const phase = state.phase;
    const row = event.target.closest(`[aria-label="Agent ${config.name}"]`);
    if (row?.getAttribute("aria-label") === `Agent ${config.name}` && phase.rosterAt === null) phase.rosterAt = now();
    const root = surface();
    const button = event.target.closest('button[title="Switch to Chat"]');
    if (button && root?.contains(button) && phase.chatAt === null) {
      phase.chatAt = now();
      sample();
    }
    const intent = event.target.closest("button");
    if (intent && root?.contains(intent) && state.intents.length < 64 && /Load older transcript|Show full details|Read more/.test(intent.textContent)) {
      state.intents.push({ kind: /older/.test(intent.textContent) ? "older" : "detail", at: now() });
    }
  }, true);
  document.addEventListener("input", (event) => {
    const phase = state.phase;
    if (!event.isTrusted || !phase || phase.chatAt === null || phase.usableAt !== null
      || !surface()?.contains(event.target) || event.target.getAttribute("aria-label") !== "Message agent"
      || event.target.value !== config.probe || document.activeElement !== event.target) return;
    inputEvidence = { phase, node: event.target };
    checkUsable(phase);
  }, true);
  document.addEventListener("wheel", (event) => {
    if (event.isTrusted && surface()?.contains(event.target) && state.intents.length < 64) {
      state.intents.push({ kind: event.deltaY < 0 ? "older" : "wheel", at: now() });
      const phase = state.phase;
      const scroll = surface()?.querySelector('[data-testid="agent-chat-scroll-region"]');
      const before = scroll?.scrollTop;
      if (phase && phase.chatAt !== null && scroll && phase.usableAt === null) {
        wheelEvidence = { phase, node: scroll, before };
        requestAnimationFrame(() => checkUsable(phase));
      }
    }
  }, true);
  const observer = new MutationObserver(sample);
  observer.observe(document.documentElement, { childList: true, subtree: true, characterData: true });
  window.__chatAcceptance = {
    state,
    bind(owner) { if (state.owner && state.owner !== owner) throw new Error("owner cannot change"); state.owner = owner; },
    arm(label) {
      if (state.phases.length >= 3) throw new Error("measurement phase bound exceeded");
      inputEvidence = null;
      wheelEvidence = null;
      state.phase = { label, rosterAt: null, chatAt: null, commitAt: null, paintAt: null, usableAt: null, body: null, checking: false };
      state.phases.push(state.phase);
    },
    sample,
  };
}

async function readObservation(driver) {
  return driver.executeScript(() => window.__chatAcceptance.state);
}

/** Maintained normal-flow case; app-free checks never import this module. */
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;

function withEnv(t, key, value) {
  const previous = process.env[key];
  process.env[key] = value;
  t.after(() => {
    if (previous === undefined) delete process.env[key];
    else process.env[key] = previous;
  });
}

async function visible(driver, selector, within = driver) {
  return driver.wait(async () => {
    for (const element of await within.findElements(By.css(selector))) {
      if (await element.isDisplayed()) return element;
    }
    return false;
  }, 10_000, `owned visible element missing: ${selector}`);
}

async function ownedSurface(driver, sessionId) {
  return visible(driver, `[data-testid="agent-session-surface"][data-resource-key="${sessionId}"]`);
}

/** Wait only for the real raw producer, never a transcript/index/admission IPC. */
async function waitForProducer(driver, harness, sessionId, fixture) {
  const log = path.join(harness.isolatedHome, "agents", sessionId, "mock-transcript.jsonl");
  return driver.wait(() => {
    if (!fs.existsSync(log)) return false;
    const fd = fs.openSync(log, "r");
    try {
      const stat = fs.fstatSync(fd, { bigint: true });
      assert.ok(stat.isFile() && stat.size <= 1024n * 1024n, "bounded plain Mock log required");
      const raw = fs.readFileSync(fd, "utf8");
      if (!raw.endsWith("\n")) return false;
      const frames = raw.trimEnd().split("\n").map((line) => JSON.parse(line));
      if (!frames.some((frame) => frame.type === "model" && frame.turn_id === fixture.latestTurn
        && frame.content === fixture.latestBody)) return false;
      if (!frames.some((frame) => frame.type === "result" && frame.turn_id === fixture.latestTurn
        && frame.status === "success")) return false;
      assert.ok(frames.length <= fixture.maxRawFrames);
      return { path: log, dev: String(stat.dev), ino: String(stat.ino), bytes: String(stat.size),
        sha256: createHash("sha256").update(raw).digest("hex"), frames: frames.length };
    } finally { fs.closeSync(fd); }
  }, 30_000, "independently declared completed Mock body/result never published");
}

async function measureChat(driver, sessionId, label, name, openRoster, keyboard = true) {
  await driver.executeScript((phase) => window.__chatAcceptance.arm(phase), label);
  if (openRoster) await (await visible(driver, `[aria-label="Agent ${name}"]`)).click();
  const surface = await ownedSurface(driver, sessionId);
  const toggle = await visible(driver, 'button[title="Switch to Chat"]', surface);
  await toggle.click();
  const input = keyboard ? await visible(driver, 'textarea[aria-label="Message agent"]', surface) : null;
  if (input) await input.sendKeys("native-input-probe");
  else await driver.actions({ async: true }).scroll(0, 0, 0, -20,
    await visible(driver, '[data-testid="agent-chat-scroll-region"]', surface)).perform();
  await driver.wait(async () => {
    const observation = await readObservation(driver);
    assert.deepEqual(observation.errors, [], "passive observer failed");
    return observation.phase.paintAt !== null && observation.phase.usableAt !== null;
  }, 5_000, `${label}: no source-bound readable body and trusted usable input`);
  const state = await readObservation(driver);
  const phase = state.phase;
  assert.ok(Number.isFinite(phase.chatAt), "trusted owned Chat selection was not observed");
  assert.ok(phase.commitAt >= phase.chatAt && phase.paintAt >= phase.commitAt);
  assert.ok(phase.usableAt >= phase.chatAt);
  assert.ok(phase.paintAt - phase.chatAt < 1000, `${label}: readable Chat exceeded 1000ms`);
  assert.ok(phase.usableAt - phase.chatAt < 1000, `${label}: usable Chat exceeded 1000ms`);
  if (openRoster) assert.ok(phase.rosterAt !== null && phase.rosterAt <= phase.chatAt);
  assert.ok(state.calls.every((call) => call.at >= state.phases[0].chatAt), "Chat was prewarmed before selection");
  assert.ok(state.calls.every((call) => call.command !== "load_agent_chat_transcript"), "legacy transcript prewarm");
  if (input) await input.sendKeys(Key.chord(Key.CONTROL, "a"), Key.BACK_SPACE);
  return phase;
}

function canonicalHistoryReceipts(receipts, sessionId) {
  let scope = null;
  let canonical = [];
  for (const receipt of receipts) {
    if (receipt.session !== sessionId) continue;
    if (receipt.reset === true) { canonical = []; scope = null; }
    // Revision is opaque: Older changes its seed. Explicit scope and reset
    // delimit this controlled current cohort, not parsed token components.
    if (typeof receipt.generation !== "string" || !receipt.generation
      || typeof receipt.revision !== "string" || !receipt.revision
      || typeof receipt.conversation !== "string" || !receipt.conversation
      || !(receipt.epoch === null || (typeof receipt.epoch === "string" && receipt.epoch))) continue;
    const next = JSON.stringify([receipt.conversation, receipt.generation, receipt.epoch]);
    if (scope !== null && scope !== next) canonical = [];
    scope = next;
    if (receipt.admission === null && receipt.progress === "ready" && !receipt.detail) canonical.push(receipt);
  }
  return canonical;
}

/** Real scroll/click demand only; opaque cursors are compared, never decoded. */
async function exerciseHistory(driver, sessionId, fixture) {
  const surface = await ownedSurface(driver, sessionId);
  const scroll = await visible(driver, '[data-testid="agent-chat-scroll-region"]', surface);
  await driver.wait(async () => canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
    .some((receipt) => receipt.next),
  30_000, "normal canonical page/cursor never arrived");
  const before = await readObservation(driver);
  assert.equal(before.calls.some((call) => call.before || call.detail), false, "lazy work before user demand");
  const used = new Set();
  const bound = Math.ceil(fixture.maxRawFrames / 80) + 2;
  for (let index = 0; index < bound; index += 1) {
    const state = await readObservation(driver);
    const cohort = canonicalHistoryReceipts(state.receipts, sessionId);
    const page = cohort.findLast((receipt) => !receipt.older);
    const older = cohort.filter((receipt) => receipt.older === true);
    const cursor = older.length ? older.at(-1).next : page?.next;
    if (!cursor) break;
    assert.equal(used.has(cursor), false, "opaque Older cursor repeated");
    used.add(cursor);
    const count = older.length;
    await driver.actions({ async: true }).scroll(0, 0, 0, -10000, scroll).perform();
    const buttons = await surface.findElements(By.xpath('.//button[contains(., "Load older transcript")]'));
    if (buttons.length && await buttons[0].isDisplayed()) await buttons[0].click();
    await driver.wait(async () => canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
      .filter((receipt) => receipt.older === true).length > count,
    10_000, "trusted Older demand produced no normal page");
  }
  const state = await readObservation(driver);
  const pages = canonicalHistoryReceipts(state.receipts, sessionId).filter((receipt) => receipt.older === true);
  assert.ok(pages.length > 0 && pages.at(-1).next === null, "Older did not exhaust within declared frame bound");
  assert.ok(state.intents.some((intent) => intent.kind === "older"), "no trusted Older input");
  for (const call of state.calls.filter((call) => call.before)) {
    assert.ok(state.intents.some((intent) => intent.kind === "older" && intent.at <= call.at));
  }
  const rows = new Map(canonicalHistoryReceipts(state.receipts, sessionId)
    .flatMap((receipt) => receipt.rows).map((row) => [row.key, row]));
  assert.ok(rows.size > 80, "fixture did not exercise more than one normal header cohort");
  for (let index = 0; index < fixture.historyTurns; index += 1) {
    assert.ok([...rows.values()].some((row) => row.role === "assistant"
      && row.text === `Controlled assistant history ${index} is complete.`), `history ${index} skipped`);
  }
  const repeats = [...rows.values()].filter((row) => row.role === "assistant" && row.text === fixture.repeatBody);
  assert.equal(repeats.length, 2, "equal text must retain two assistant identities");
  assert.equal(new Set(repeats.map((row) => row.id)).size, 2);
  assert.equal(new Set(repeats.map((row) => row.turn)).size, 2);
  const long = [...rows.values()].find((row) => row.role === "assistant" && row.turn === "chat-acceptance-long");
  assert.ok(long, "independent full-body owner absent from actual pages");
  const longRow = await visible(driver, `[data-chat-row-key="${long.key}"]`, surface);
  const detailButtons = await longRow.findElements(By.xpath('.//button[normalize-space()="Show full details"]'));
  assert.equal(detailButtons.length, 1, "long body did not retain lazy details");
  await detailButtons[0].click();
  const detailBound = Math.ceil(Buffer.byteLength(fixture.longBody) / 65536) + 1;
  for (let index = 0; index < detailBound; index += 1) {
    const pre = await visible(driver, "pre", longRow);
    if (await pre.getText() === fixture.longBody) break;
    const more = await longRow.findElements(By.xpath('.//button[normalize-space()="Read more"]'));
    assert.equal(more.length, 1, "full-body window incomplete without continuation");
    await more[0].click();
  }
  assert.equal(await (await visible(driver, "pre", longRow)).getText(), fixture.longBody);
  const completed = await readObservation(driver);
  assert.ok(completed.calls.some((call) => call.detail && completed.intents
    .some((intent) => intent.kind === "detail" && intent.at <= call.at)), "detail did not follow trusted intent");
  return { olderPages: pages.length, distinctHeaders: rows.size, detailBytes: Buffer.byteLength(fixture.longBody) };
}

function redactObservation(value) {
  if (Array.isArray(value)) return value.map(redactObservation);
  if (!value || typeof value !== "object") return value;
  const result = {};
  for (const [key, item] of Object.entries(value)) {
    result[key] = typeof item === "string" && !["label", "command", "kind", "provider", "role", "progress"].includes(key)
      ? createHash("sha256").update(item).digest("hex") : redactObservation(item);
  }
  return result;
}

// Native execution is intentionally separate from the app-free observer tests.
// Startup/cleanup errors fail this case; no catch-to-skip or synthetic UI input.
test("chat transcript renders the per-turn change surface", { timeout: 600_000 }, async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  assert.ok(harness.appPath);
  const fixture = chatAcceptanceFixture();
  const name = `Chat-Changes-${RUN_ID}`;
  withEnv(t, "WARDIAN_MOCK_SCENARIO", "chat_acceptance");
  withEnv(t, "WARDIAN_MOCK_DELAY_MS", "5");
  withEnv(t, "WARDIAN_NATIVE_SESSION_START_ATTEMPTS", "1");
  prepareIsolatedHome(harness);
  const workspace = path.join(harness.isolatedHome, `chat-workspace-${RUN_ID}`);
  fs.mkdirSync(workspace, { recursive: true });
  const sessions = [];
  const cleanupOwner = { sessionId: null };
  let paused = false;
  const evidence = { timings: [], history: null, endpoints: "UNRUN", preselectionIndex: "UNOBSERVED",
    cacheState: "cold process only; raw producer inspection warms filesystem", transition: "UNRUN" };
  t.after(async () => {
    const errors = [];
    for (const entry of sessions.toReversed()) {
      if (entry.closed) continue;
      try { evidence.lastObservation = redactObservation(await readObservation(entry.session.driver)); }
      catch (error) { evidence.observationReadError = String(error).slice(0, 160); }
      try {
        if (cleanupOwner.sessionId && !paused) {
          await invokeTauri(entry.session.driver, "pause_agent", { sessionId: cleanupOwner.sessionId });
          paused = true;
        }
      } catch (error) { errors.push(error); }
      try { await entry.session.close(); entry.closed = true; } catch (error) { errors.push(error); }
    }
    const evidenceDir = path.join(harness.isolatedHome, "artifacts", "chat-normal-flow");
    fs.mkdirSync(evidenceDir, { recursive: true });
    fs.writeFileSync(path.join(evidenceDir, "result.json"), JSON.stringify(evidence, null, 2));
    if (errors.length) throw new AggregateError(errors, "owned native cleanup failed");
  });
  const open = async () => {
    const session = await startNativeSession(harness);
    const entry = { session, closed: false };
    sessions.push(entry);
    await waitForAppShell(session.driver, 30_000);
    await session.driver.executeScript(installChatObservation, { name, body: fixture.latestBody, probe: "native-input-probe" });
    return entry;
  };
  const first = await open();
  const driver = first.session.driver;
  const agent = await invokeTauri(driver, "spawn_agent", { req: { sessionName: name, agentClass: "Coder", folder: workspace,
    isOff: false, configOverride: { provider: "mock", conversation_logging: "enabled" } } });
  const sessionId = agent.session_id ?? agent.sessionId;
  cleanupOwner.sessionId = sessionId;
  assert.ok(sessionId);
  await driver.executeScript((id) => window.__chatAcceptance.bind(id), sessionId);
  const producer = await waitForProducer(driver, harness, sessionId, fixture);
  evidence.timings.push(await measureChat(driver, sessionId, "cold-process-first-chat", name, true));
  const surface = await ownedSurface(driver, sessionId);
  await (await visible(driver, 'button[title="Switch to Terminal"]', surface)).click();
  evidence.timings.push(await measureChat(driver, sessionId, "same-process-warm-chat", name, false));
  evidence.history = await exerciseHistory(driver, sessionId, fixture);

  // Preserve the original two tool-turn contracts, after all timing and lazy UI
  // claims. This legacy read is explicitly forbidden before the first Chat.
  const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
  const edits = events.filter((event) => event?.metadata?.tool_name === "Edit");
  assert.ok(edits.length >= 2);
  assert.equal(edits[0].metadata.tool_input.old_string, "const WORK_GROUP_MIN_ENTRIES = 4;");
  assert.equal(edits[0].metadata.files_written[0], "src/features/chat/chatPresentation.ts");
  const write = events.find((event) => event?.metadata?.tool_name === "Write");
  assert.ok(write.metadata.tool_input.content.includes("Mock spec"));
  const card = await visible(driver, '[data-testid="turn-change-card"]', surface);
  const cardText = await card.getText();
  assert.match(cardText, /changed files?/);
  assert.match(cardText, /chatPresentation\.ts/);
  assert.doesNotMatch(cardText, /Created/);
  assert.ok((await surface.findElements(By.css('[data-testid="chat-work-group"]'))).length >= 1);
  const panel = await visible(driver, '[data-testid="tool-structured-edit"]', surface);
  assert.match(await panel.getText(), /Before\/after|New contents/);
  evidence.live = redactObservation(await readObservation(driver));
  const presentations = (await readObservation(driver)).presentations;
  const preview = presentations.find((item) => item.admission);
  const canonical = preview && presentations.find((item) => item.at > preview.at && item.admission === null);
  if (canonical) {
    assert.equal(preview.matchingBodySlots, 1);
    assert.equal(canonical.matchingBodySlots, 1, "preview was not retired to one canonical target slot");
    // Preview and canonical IDs deliberately differ; no alias is invented.
    // Each observation independently binds its actual response row to the DOM.
    const current = canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
      .find((receipt) => receipt.rows.some((row) => row.target && row.key === canonical.key));
    assert.ok(current, "visible canonical target must belong to the current durable cohort");
    assert.equal(canonical.generation, current.generation);
    assert.equal(canonical.conversation, current.conversation);
    assert.equal(canonical.epoch, current.epoch);
    evidence.transition = "observed visible preview retirement to one actual canonical target slot; IDs not equated";
  } else evidence.transition = "UNRUN: canonical-first or no observed visible handoff";

  // Save Terminal mode through normal input before the same-home restart. The
  // restart does not reset the home, reseed an archive, or resume the provider.
  await (await visible(driver, 'button[title="Switch to Terminal"]', surface)).click();
  await invokeTauri(driver, "pause_agent", { sessionId });
  paused = true;
  await first.session.close();
  first.closed = true;
  assert.ok(first.session.tauriDriver.exitCode !== null || first.session.tauriDriver.signalCode !== null,
    "original driver child did not join before same-home restart");
  const saved = await open();
  await saved.session.driver.executeScript((id) => window.__chatAcceptance.bind(id), sessionId);
  evidence.timings.push(await measureChat(saved.session.driver, sessionId, "saved-off-restart", name, true, false));
  const agents = await invokeTauri(saved.session.driver, "list_agents");
  assert.equal(agents.find((entry) => entry.session_id === sessionId)?.is_off, true, "saved session resumed");
  const after = await waitForProducer(saved.session.driver, harness, sessionId, fixture);
  assert.deepEqual(after, producer, "saved-Off changed the original provider inode/content");
  evidence.saved = redactObservation(await readObservation(saved.session.driver));
  evidence.timings = redactObservation(evidence.timings);
  evidence.endpoints = "PASS: owned Mock normal UI only; no real-provider/OS-cold/PWA/whole-suite claim";
});
