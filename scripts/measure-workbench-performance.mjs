import fsSync from "node:fs";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { gzipSync } from "node:zlib";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const fixturePath = path.join(repoRoot, "scripts", "fixtures", "workbench-performance-v1.json");
const baselinePath = path.join(
  repoRoot,
  "docs",
  "research",
  "workbench-navigation",
  "workbench-performance-baseline.json",
);
const refusal = "Refusing to benchmark without an explicit isolated WARDIAN_HOME.";
const gates = Object.freeze({
  restore_p95_ms: { limit: 1500, unit: "ms" },
  tab_switch_p95_ms: { limit: 100, unit: "ms" },
  group_focus_p95_ms: { limit: 75, unit: "ms" },
  terminal_output_commit_p95_ms: { limit: 50, unit: "ms" },
  stream_gap_count: { limit: 0, unit: "gaps" },
  overview_settle_p95_ms: { limit: 300, unit: "ms" },
  heavy_surface_resume_p95_ms: { limit: 500, unit: "ms" },
  react_commit_max_ms: { limit: 50, unit: "ms" },
  bundle_delta_gzip_bytes: { limit: 250 * 1024, unit: "bytes" },
  xterm_renderer_peak: { limit: 24, unit: "renderers" },
  webgl_context_peak: { limit: 12, unit: "contexts" },
});

function samePath(left, right) {
  const normalizedLeft = path.normalize(left);
  const normalizedRight = path.normalize(right);
  return process.platform === "win32"
    ? normalizedLeft.toLowerCase() === normalizedRight.toLowerCase()
    : normalizedLeft === normalizedRight;
}

function inside(parent, candidate) {
  const relative = path.relative(parent, candidate);
  return relative !== "" && relative !== ".." && !relative.startsWith(`..${path.sep}`)
    && !path.isAbsolute(relative);
}

function canonicalize(candidate) {
  const missing = [];
  let ancestor = path.resolve(candidate);
  while (!fsSync.existsSync(ancestor)) {
    const parent = path.dirname(ancestor);
    if (samePath(parent, ancestor)) throw new Error(refusal);
    missing.unshift(path.basename(ancestor));
    ancestor = parent;
  }
  return path.resolve(fsSync.realpathSync.native(ancestor), ...missing);
}

function isolatedWardianHome(rawValue) {
  if (typeof rawValue !== "string" || rawValue.trim() === "") throw new Error(refusal);
  const trimmed = rawValue.trim();
  if (!path.isAbsolute(trimmed)) throw new Error(refusal);
  const resolved = canonicalize(trimmed);
  const profile = canonicalize(os.homedir());
  const production = canonicalize(path.join(os.homedir(), ".wardian"));
  const workspace = canonicalize(repoRoot);
  if (samePath(resolved, profile) || samePath(resolved, production) || samePath(resolved, workspace)) {
    throw new Error(refusal);
  }
  const workspacePerfRoot = canonicalize(path.join(repoRoot, ".tmp", "workbench-performance"));
  const tempRoot = canonicalize(os.tmpdir());
  const allowed = inside(workspacePerfRoot, resolved)
    || (inside(tempRoot, resolved) && path.basename(resolved).startsWith("wardian-workbench-performance-"));
  if (!allowed) throw new Error(refusal);
  return resolved;
}

function assertInsideHome(home, candidate) {
  if (!inside(home, candidate)) throw new Error(`Refusing filesystem mutation outside ${home}: ${candidate}`);
}

function round(value) {
  return Number(value.toFixed(2));
}

function summarize(values, label) {
  if (!Array.isArray(values) || values.length === 0) throw new Error(`${label} requires observed samples`);
  if (values.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error(`${label} contains a non-finite or negative observation`);
  }
  const samples = values.map(round).sort((left, right) => left - right);
  const at = (ratio) => samples[Math.min(samples.length - 1, Math.ceil(samples.length * ratio) - 1)];
  return { samples, median: at(0.5), p95: at(0.95), max: samples.at(-1) };
}

function fixtureErrors(fixture) {
  const errors = [];
  const document = fixture?.workbench;
  const surfaces = Object.values(document?.surfaces ?? {});
  const presentations = fixture?.terminal_presentations ?? [];
  const required = ["agents-overview", "graph", "garden", "queue", "library", "workflows"];
  const singletonTypes = new Set([
    "agents-overview", "dashboard", "queue", "graph", "garden", "library", "workflows",
  ]);
  if (fixture?.schema_version !== 1) errors.push("fixture schema_version must be 1");
  if (Object.keys(document?.groups ?? {}).length !== 4) errors.push("fixture must contain four groups");
  if (surfaces.length !== 20) errors.push("fixture must contain 20 tabs");
  if ((fixture?.agents ?? []).length !== 20) errors.push("fixture must contain 20 agents");
  const heavyGrace = fixture?.benchmark?.heavy_surface_hidden_grace_ms;
  if (!Number.isSafeInteger(heavyGrace) || heavyGrace < 1 || heavyGrace > 300_000) {
    errors.push("fixture heavy_surface_hidden_grace_ms must be an integer from 1 through 300000");
  }
  for (const type of required) {
    if (!surfaces.some((surface) => surface.surface_type === type)) errors.push(`fixture lacks ${type}`);
  }
  for (const type of singletonTypes) {
    if (surfaces.filter((surface) => surface.surface_type === type).length > 1) {
      errors.push(`fixture duplicates singleton surface ${type}`);
    }
  }
  if (presentations.filter((entry) => entry.mode === "owner").length !== 1) {
    errors.push("fixture must contain one terminal owner");
  }
  if (presentations.filter((entry) => entry.mode === "mirror").length !== 3) {
    errors.push("fixture must contain three terminal mirrors");
  }
  if (new Set(presentations.map((entry) => entry.session_id)).size !== 1) {
    errors.push("owner and mirrors must share one runtime");
  }
  return errors;
}

function gateObservations(baseline) {
  return {
    restore_p95_ms: baseline.runtime?.startup_restore_ms?.p95,
    tab_switch_p95_ms: baseline.runtime?.tab_switch_ms?.p95,
    group_focus_p95_ms: baseline.runtime?.group_focus_ms?.p95,
    terminal_output_commit_p95_ms: baseline.runtime?.terminal_output_commit_ms?.p95,
    stream_gap_count: baseline.runtime?.stream_gap_count,
    overview_settle_p95_ms: baseline.runtime?.overview_settle_ms?.p95,
    heavy_surface_resume_p95_ms: baseline.runtime?.heavy_surface_resume_ms?.p95,
    react_commit_max_ms: baseline.runtime?.react_commit_max_ms,
    bundle_delta_gzip_bytes: baseline.bundle?.production_delta_gzip_bytes,
    xterm_renderer_peak: baseline.runtime?.renderer_peaks?.xterm,
    webgl_context_peak: baseline.runtime?.renderer_peaks?.webgl,
  };
}

function evaluateGates(baseline) {
  const observed = gateObservations(baseline);
  for (const [metric, value] of Object.entries(observed)) {
    if (!Number.isFinite(value) || value < 0) throw new Error(`Missing observed metric: ${metric}`);
  }
  const checks = Object.entries(gates).map(([metric, gate]) => ({
    metric,
    observed: observed[metric],
    operator: "<=",
    limit: gate.limit,
    unit: gate.unit,
    passed: observed[metric] <= gate.limit,
  }));
  const result = { schema_version: 1, passed: checks.every((check) => check.passed), checks };
  if (!result.passed) throw new Error(`Workbench performance gate failure:\n${JSON.stringify(result, null, 2)}`);
  return result;
}

async function seedHome(home, fixture) {
  const settings = path.join(home, "settings");
  const workbench = path.join(settings, "workbench.json");
  const copiedFixture = path.join(settings, "workbench-performance-fixture.json");
  for (const target of [settings, workbench, copiedFixture]) assertInsideHome(home, target);
  await fs.mkdir(settings, { recursive: true });
  await fs.writeFile(workbench, JSON.stringify(fixture.workbench), "utf8");
  await fs.writeFile(copiedFixture, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
  return path.relative(home, copiedFixture).replaceAll(path.sep, "/");
}

async function filesUnder(directory) {
  return (await Promise.all((await fs.readdir(directory, { withFileTypes: true })).map(async (entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? filesUnder(target) : [target];
  }))).flat();
}

async function gzipBytes(directory) {
  let total = 0;
  for (const file of (await filesUnder(directory)).filter((entry) => /\.(?:js|css)$/i.test(entry))) {
    total += gzipSync(await fs.readFile(file)).byteLength;
  }
  return total;
}

async function productionBundleDelta(home) {
  const { build } = await import("vite");
  const root = path.join(home, "bundle-measurement");
  const candidate = path.join(root, "canonical");
  for (const target of [root, candidate]) assertInsideHome(home, target);
  await fs.mkdir(root, { recursive: true });
  const reference = JSON.parse(await fs.readFile(baselinePath, "utf8"));
  const baseGzip = reference?.bundle?.base_gzip_bytes;
  if (!Number.isSafeInteger(baseGzip) || baseGzip <= 0) {
    throw new Error(
      `Refusing to measure bundle delta without a positive integer bundle.base_gzip_bytes in ${baselinePath}.`,
    );
  }
  const previousPerf = process.env.VITE_WARDIAN_WORKBENCH_PERF;
  const previousGrace = process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS;
  try {
    delete process.env.VITE_WARDIAN_WORKBENCH_PERF;
    delete process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS;
    await build({ root: repoRoot, logLevel: "warn", build: { outDir: candidate, emptyOutDir: true } });
  } finally {
    if (previousPerf === undefined) delete process.env.VITE_WARDIAN_WORKBENCH_PERF;
    else process.env.VITE_WARDIAN_WORKBENCH_PERF = previousPerf;
    if (previousGrace === undefined) delete process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS;
    else process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS = previousGrace;
  }
  const candidateGzip = await gzipBytes(candidate);
  return {
    base_gzip_bytes: baseGzip,
    candidate_gzip_bytes: candidateGzip,
    production_delta_gzip_bytes: Math.max(0, candidateGzip - baseGzip),
  };
}

function benchmarkValidationPlugin() {
  return {
    name: "wardian-workbench-benchmark-validation",
    enforce: "pre",
    transform(code, id) {
      if (!id.replaceAll("\\", "/").endsWith("/src/main.tsx")) return null;
      return {
        code: `import { validateWorkbenchDocument as __wardianValidateWorkbenchDocument } from "./features/workbench/workbenchModel";\n`
          + `globalThis.__WARDIAN_VALIDATE_WORKBENCH__ = __wardianValidateWorkbenchDocument;\n${code}`,
        map: null,
      };
    },
  };
}

async function buildProductionRuntime(home, fixture) {
  const { build } = await import("vite");
  const outDir = path.join(home, "runtime-build");
  assertInsideHome(home, outDir);
  const previousPerf = process.env.VITE_WARDIAN_WORKBENCH_PERF;
  const previousGrace = process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS;
  try {
    process.env.VITE_WARDIAN_WORKBENCH_PERF = "1";
    process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS = String(
      fixture.benchmark.heavy_surface_hidden_grace_ms,
    );
    await build({
      root: repoRoot,
      logLevel: "warn",
      plugins: [benchmarkValidationPlugin()],
      resolve: { alias: { "react-dom/client": "react-dom/profiling" } },
      build: { outDir, emptyOutDir: true },
    });
  } finally {
    if (previousPerf === undefined) delete process.env.VITE_WARDIAN_WORKBENCH_PERF;
    else process.env.VITE_WARDIAN_WORKBENCH_PERF = previousPerf;
    if (previousGrace === undefined) delete process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS;
    else process.env.VITE_WARDIAN_HEAVY_SURFACE_GRACE_MS = previousGrace;
  }
  return outDir;
}

function browserFixture(fixture) {
  localStorage.setItem("wardian-settings", JSON.stringify({
    state: { gridCardDisplayMode: "chat" },
    version: 2,
  }));
  const clone = (value) => structuredClone(value);
  const callbacks = new Map();
  const listeners = new Map();
  let callbackId = 1;
  let eventId = 1;
  const trackedRuntime = fixture.terminal_presentations[0].session_id;
  const trackedOwner = fixture.terminal_presentations.find((entry) => entry.mode === "owner").presentation_id;
  const runtime = {
    document: clone(fixture.workbench),
    events: [],
    last_ack: 0,
    stream_gap_count: 0,
    react_commits: [],
    xterm_peak: 0,
    xterm_live: 0,
    webgl_peak: 0,
    webgl_live: 0,
    terminal_burst_started_at: null,
    terminal_burst_last_sequence: null,
    terminal_last_commit_ms: null,
  };
  const brokerState = (sessionId = trackedRuntime, presentationId = null) => ({
    session_id: sessionId, runtime_generation: 1, lease_epoch: 1,
    stream_sequence: runtime.events.length, interaction_sequence: 1,
    geometry: { cols: 80, rows: 24 },
    owner_presentation_id: sessionId === trackedRuntime ? trackedOwner : presentationId,
    pending_activation: null, runtime_state: "live",
  });
  const snapshot = (sessionId = trackedRuntime) => ({
    snapshot_id: `perf-snapshot-${sessionId}`, session_id: sessionId, runtime_generation: 1,
    sequence_barrier: 0, geometry: { cols: 80, rows: 24 }, terminal_state_base64: "",
    visible_grid: "", scrollback: [],
  });
  const presentation = (request) => ({
    presentation_id: request.presentation_id, client_kind: "desktop",
    desired_geometry: request.desired_geometry ?? { cols: 80, rows: 24 },
    visibility: request.visibility, render_state: request.render_state,
    interaction_capability: request.requested_interaction, interaction_sequence: 1,
    requires_resync: false,
  });
  const registration = (request) => ({
    presentation: presentation(request),
    broker_state: brokerState(request.session_id, request.presentation_id),
    initial_snapshot: snapshot(request.session_id),
  });
  const emit = (name, payload) => {
    for (const listener of listeners.get(name) ?? []) {
      callbacks.get(listener.callback_id)?.({ event: name, id: listener.event_id, payload: clone(payload) });
    }
  };
  runtime.emit_terminal_burst = (lineCount) => {
    runtime.terminal_burst_started_at = performance.now();
    runtime.terminal_last_commit_ms = null;
    const first = runtime.events.length + 1;
    for (let index = 0; index < lineCount; index += 1) {
      const sequence = runtime.events.length + 1;
      runtime.events.push({
        sequence, runtime_generation: 1, type: "output",
        bytes: [...new TextEncoder().encode(`perf-${sequence}\r\n`)],
      });
    }
    emit("terminal-session-events-ready", {
      session_id: trackedRuntime, runtime_generation: 1,
      latest_sequence: runtime.events.length,
    });
    runtime.terminal_burst_last_sequence = runtime.events.length;
    return { first, last: runtime.events.length };
  };
  window.__WARDIAN_WORKBENCH_PERF__ = runtime;
  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
    supportsFiber: true,
    inject: () => 1,
    onCommitFiberRoot: (_id, root) => {
      const duration = root?.current?.actualDuration;
      if (Number.isFinite(duration) && duration >= 0) runtime.react_commits.push(duration);
    },
    onCommitFiberUnmount: () => undefined,
  };
  const originalContext = HTMLCanvasElement.prototype.getContext;
  const liveWebglContexts = new Set();
  const contextsByCanvas = new WeakMap();
  const loseContextWrapped = new WeakSet();
  const wrappedLoseExtensions = new WeakSet();
  const refreshWebglLiveCount = () => {
    for (const entry of [...liveWebglContexts]) {
      if (!entry.canvas.isConnected || entry.context.isContextLost?.()) {
        liveWebglContexts.delete(entry);
      }
    }
    runtime.webgl_live = liveWebglContexts.size;
    runtime.webgl_peak = Math.max(runtime.webgl_peak, runtime.webgl_live);
  };
  const releaseCanvasContexts = (canvasContexts) => {
    for (const entry of canvasContexts) liveWebglContexts.delete(entry);
    canvasContexts.clear();
    refreshWebglLiveCount();
  };
  HTMLCanvasElement.prototype.getContext = function getContext(type, ...args) {
    const context = originalContext.call(this, type, ...args);
    if (context && (type === "webgl" || type === "webgl2")) {
      let canvasContexts = contextsByCanvas.get(this);
      if (!canvasContexts) {
        canvasContexts = new Set();
        contextsByCanvas.set(this, canvasContexts);
        this.addEventListener("webglcontextlost", () => releaseCanvasContexts(canvasContexts));
      }
      if (![...canvasContexts].some((entry) => entry.context === context)) {
        const entry = { canvas: this, context };
        canvasContexts.add(entry);
        liveWebglContexts.add(entry);
      }
      if (!loseContextWrapped.has(context)) {
        loseContextWrapped.add(context);
        const originalGetExtension = context.getExtension.bind(context);
        context.getExtension = (name) => {
          const extension = originalGetExtension(name);
          if (
            name === "WEBGL_lose_context"
            && extension?.loseContext
            && !wrappedLoseExtensions.has(extension)
          ) {
            wrappedLoseExtensions.add(extension);
            const originalLoseContext = extension.loseContext.bind(extension);
            extension.loseContext = () => {
              releaseCanvasContexts(canvasContexts);
              return originalLoseContext();
            };
          }
          return extension;
        };
      }
      refreshWebglLiveCount();
    }
    return context;
  };
  const observePeaks = () => {
    runtime.xterm_live = document.querySelectorAll(".xterm").length;
    runtime.xterm_peak = Math.max(runtime.xterm_peak, runtime.xterm_live);
    refreshWebglLiveCount();
  };
  const installPeakObserver = () => {
    if (!document.documentElement) return;
    new MutationObserver(observePeaks).observe(
      document.documentElement,
      { childList: true, subtree: true },
    );
    observePeaks();
  };
  if (document.documentElement) installPeakObserver();
  else window.addEventListener("DOMContentLoaded", installPeakObserver, { once: true });
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  const emptyLibrarySection = (stubbed = false) => ({
    tree: { path: "", name: "Root", children: [] },
    stubbed,
  });
  const defaults = {
    list_agent_classes: [], list_provider_readiness: [], load_watchlists: [], load_watchlist_prefs: null,
    load_agent_interactions: {}, load_queue_items: [], load_queue_preferences: {},
    load_onboarding_hints: { dismissed_hint_ids: ["spawn-agent-first-run:v1"] },
    list_workflows: [], list_scheduled_runs: [], load_workflow_library: { folders: [], rootWorkflowIds: [] },
    workflow_list_blueprints: [], workflow_list_runs: [], get_topology: { edges: [], ignored_pairs: [], fallback_groups: [] },
    workflow_validate: { ok: true, diagnostics: [] },
    workflow_write: { written: true, diagnostics: [] },
    get_pair_activity: [], load_app_settings: null, load_shell_settings: null, list_available_shells: [],
    sync_provider_theme_settings: null, library_watch: null, library_unwatch: null,
    get_library_index: {
      sections: {
        skills: emptyLibrarySection(),
        prompts: emptyLibrarySection(),
        workflows: emptyLibrarySection(),
        classes: emptyLibrarySection(),
        mcps: emptyLibrarySection(true),
      },
      deployments: {},
      orphans: [],
    },
  };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    transformCallback: (callback) => { const id = callbackId++; callbacks.set(id, callback); return id; },
    unregisterCallback: (id) => callbacks.delete(id), convertFileSrc: (value) => value,
    invoke: async (command, args = {}) => {
      if (command === "get_workbench_boot_config") return { safe_mode: false };
      if (command === "load_workbench_state") return {
        source: "primary", document: clone(runtime.document), notice: null,
        durable_revision: runtime.document.revision, durable_token: `perf-${runtime.document.revision}`,
      };
      if (command === "save_workbench_state") {
        runtime.document = clone(args.document);
        return { outcome: "saved", durable_revision: args.document.revision, durable_token: `perf-${args.document.revision}`, request_id: args.request_id };
      }
      if (command === "list_agents") return clone(fixture.agents);
      if (command === "register_terminal_presentation") return registration(args.request);
      if (command === "update_terminal_presentation") return registration(args.request);
      if (command === "report_terminal_presentation_viewport") return presentation(args.request);
      if (command === "subscribe_terminal_events") return {
        broker_state: brokerState(args.request.session_id),
        initial_snapshot: snapshot(args.request.session_id),
      };
      if (command === "read_terminal_events") {
        const after = args.request.after_sequence;
        const events = runtime.events.filter((event) => event.sequence > after).slice(0, args.request.max_events);
        if (events[0] && events[0].sequence !== after + 1) runtime.stream_gap_count += 1;
        return { status: "events", runtime_generation: 1, events,
          next_sequence: events.at(-1)?.sequence ?? after, available_from_sequence: 1,
          latest_sequence: runtime.events.length, recovery_snapshot: null };
      }
      if (command === "ack_terminal_events") {
        if (args.request.session_id === trackedRuntime) {
          runtime.last_ack = args.request.applied_sequence;
          if (
            runtime.terminal_burst_started_at !== null
            && runtime.terminal_burst_last_sequence !== null
            && runtime.last_ack >= runtime.terminal_burst_last_sequence
            && runtime.terminal_last_commit_ms === null
          ) {
            runtime.terminal_last_commit_ms = performance.now() - runtime.terminal_burst_started_at;
          }
        }
        return { runtime_generation: 1, acknowledged_sequence: args.request.applied_sequence };
      }
      if (["unsubscribe_terminal_events", "unregister_terminal_presentation"].includes(command)) {
        return brokerState(args.request.session_id, args.request.presentation_id);
      }
      if (command === "plugin:event|listen") {
        const id = eventId++; const list = listeners.get(args.event) ?? [];
        list.push({ callback_id: args.handler, event_id: id }); listeners.set(args.event, list); return id;
      }
      if (command === "plugin:event|unlisten") return null;
      return clone(defaults[command] ?? null);
    },
  };
}

async function preparedPage(browser, baseUrl, fixture, errors) {
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
  page.on("console", (message) => { if (message.type() === "error") errors.push(message.text()); });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(browserFixture, fixture);
  const started = performance.now();
  await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
  try {
    await page.locator('[data-testid="workbench-host"]').waitFor({ timeout: 20_000 });
  } catch (error) {
    const diagnostic = await page.evaluate(() => ({
      app_shell: document.querySelector('[data-testid="app-shell"]') !== null,
      body: document.body?.innerText.slice(0, 2_000) ?? "",
      tauri: typeof window.__TAURI_INTERNALS__?.invoke,
    }));
    throw new Error(`Production workbench host did not mount: ${JSON.stringify({ diagnostic, errors })}\n${error}`);
  }
  const validation = await page.evaluate((document) => {
    if (typeof window.__WARDIAN_VALIDATE_WORKBENCH__ !== "function") {
      throw new Error("Canonical workbench validator is unavailable in the production benchmark build");
    }
    return window.__WARDIAN_VALIDATE_WORKBENCH__(document);
  }, fixture.workbench);
  if (!validation.valid) {
    throw new Error(`Canonical workbench validation failed: ${JSON.stringify(validation.errors)}`);
  }
  try {
    await page.waitForFunction(() => new Set(
      [...document.querySelectorAll('[role="tab"][data-surface-id]')]
        .map((tab) => tab.getAttribute("data-surface-id")),
    ).size === 20, undefined, { timeout: 10_000 });
  } catch (error) {
    const diagnostic = await page.evaluate(() => ({
      tabs: [...document.querySelectorAll('[role="tab"][data-surface-id]')]
        .map((tab) => tab.getAttribute("data-surface-id")),
      groups: [...document.querySelectorAll('[data-testid="workbench-group"]')]
        .map((group) => group.getAttribute("data-group-id")),
      panels: [...document.querySelectorAll('[data-testid="surface-panel"][data-surface-id]')]
        .map((panel) => panel.getAttribute("data-surface-id")),
      notice: document.querySelector('[data-testid="workbench-persistence-notice"]')?.textContent ?? null,
    }));
    throw new Error(`Production restore did not expose 20 unique tabs: ${JSON.stringify({ diagnostic, errors })}\n${error}`);
  }
  return { page, restore_ms: performance.now() - started };
}

async function twoFrames(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function clickSurfaceTabFromUser(page, surfaceId) {
  await page.evaluate(() => {
    const runtime = window.__WARDIAN_WORKBENCH_PERF__;
    runtime.tab_activation_started_at = null;
    window.addEventListener("pointerdown", () => {
      runtime.tab_activation_started_at = performance.now();
    }, { capture: true, once: true });
  });
  await page.locator(
    `[role="tab"][data-surface-id=${JSON.stringify(surfaceId)}]`,
  ).filter({ visible: true }).first().click();
}

async function measureTabActivation(page, surfaceId) {
  await clickSurfaceTabFromUser(page, surfaceId);
  return await page.evaluate(async (id) => {
    const selector = `[role="tab"][data-surface-id=${JSON.stringify(id)}]`;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    if (![...document.querySelectorAll(selector)].some(
      (candidate) => candidate.getAttribute("aria-selected") === "true",
    )) {
      throw new Error(`Surface tab ${id} did not activate`);
    }
    const started = window.__WARDIAN_WORKBENCH_PERF__.tab_activation_started_at;
    if (!Number.isFinite(started)) throw new Error(`Surface tab ${id} did not receive user input`);
    return performance.now() - started;
  }, surfaceId);
}

async function waitForHeavyRendererReleased(page, surfaceId, timeoutMs) {
  await page.waitForFunction((id) => {
    const panel = document.querySelector(`[data-testid="surface-panel"][data-surface-id=${JSON.stringify(id)}]`);
    return panel?.querySelector('[data-heavy-renderer-state="released"]') !== null;
  }, surfaceId, { timeout: timeoutMs });
}

async function measureHeavySurfaceActivation(page, surfaceId) {
  await clickSurfaceTabFromUser(page, surfaceId);
  return await page.evaluate(async (id) => {
    const tabSelector = `[role="tab"][data-surface-id=${JSON.stringify(id)}]`;
    const panelSelector = `[data-testid="surface-panel"][data-surface-id=${JSON.stringify(id)}]`;
    const panel = document.querySelector(panelSelector);
    if (!(panel instanceof HTMLElement)) {
      throw new Error(`Heavy surface ${id} is unavailable`);
    }
    const started = window.__WARDIAN_WORKBENCH_PERF__.tab_activation_started_at;
    if (!Number.isFinite(started)) throw new Error(`Heavy surface ${id} did not receive user input`);
    const deadline = started + 20_000;
    while (performance.now() < deadline) {
      const currentPanel = document.querySelector(panelSelector);
      const readySurface = id === "perf-graph"
        ? currentPanel?.querySelector('[data-testid="graph-view"]')
        : currentPanel?.querySelector("canvas");
      const rect = readySurface?.getBoundingClientRect();
      const mounted = currentPanel?.querySelector('[data-heavy-renderer-state="mounted"]') !== null;
      const selected = [...document.querySelectorAll(tabSelector)].some(
        (candidate) => candidate.getAttribute("aria-selected") === "true",
      );
      if (
        selected
        && mounted
        && rect
        && rect.width > 0
        && rect.height > 0
      ) {
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        return performance.now() - started;
      }
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    const finalPanel = document.querySelector(panelSelector);
    const finalReady = id === "perf-graph"
      ? finalPanel?.querySelector('[data-testid="graph-view"]')
      : finalPanel?.querySelector("canvas");
    throw new Error(`Heavy surface ${id} did not remount within 20000ms: ${JSON.stringify({
      selected: [...document.querySelectorAll(tabSelector)].map(
        (candidate) => candidate.getAttribute("aria-selected"),
      ),
      panel: finalPanel !== null,
      state: finalPanel?.querySelector('[data-heavy-renderer-state]')
        ?.getAttribute("data-heavy-renderer-state") ?? null,
      ready: finalReady !== null && finalReady !== undefined,
      rect: finalReady ? {
        width: finalReady.getBoundingClientRect().width,
        height: finalReady.getBoundingClientRect().height,
      } : null,
    })}`);
  }, surfaceId);
}

async function measureRuntime(fixture, runtimeOutDir) {
  const [{ chromium }, { preview }] = await Promise.all([
    import("@playwright/test"),
    import("vite"),
  ]);
  const server = await preview({
    root: repoRoot,
    logLevel: "error",
    build: { outDir: runtimeOutDir },
    preview: { host: "127.0.0.1", port: 0 },
  });
  let browser;
  try {
    const baseUrl = server.resolvedUrls?.local[0];
    if (!baseUrl) throw new Error("Vite did not expose a performance URL");
    browser = await chromium.launch();
    const errors = [];
    const startup = [];
    let page;
    for (let index = 0; index < 5; index += 1) {
      if (page) await page.close();
      const prepared = await preparedPage(browser, baseUrl, fixture, errors);
      startup.push(prepared.restore_ms);
      page = prepared.page;
    }
    const tabIds = Object.keys(fixture.workbench.surfaces);
    const tabSwitch = [];
    for (let index = 0; index < 20; index += 1) {
      tabSwitch.push(await measureTabActivation(page, tabIds[index]));
    }
    const groupFocus = [];
    for (const group of Object.values(fixture.workbench.groups)) {
      groupFocus.push(await measureTabActivation(page, group.active_surface_id));
    }
    const terminalOutput = [];
    for (let index = 0; index < 10; index += 1) {
      const target = await page.evaluate(() => window.__WARDIAN_WORKBENCH_PERF__.emit_terminal_burst(20));
      await page.waitForFunction((sequence) => window.__WARDIAN_WORKBENCH_PERF__.last_ack >= sequence, target.last);
      terminalOutput.push(await page.evaluate(() => {
        const duration = window.__WARDIAN_WORKBENCH_PERF__.terminal_last_commit_ms;
        if (!Number.isFinite(duration) || duration < 0) {
          throw new Error("Terminal commit instrumentation did not record an in-page duration");
        }
        return duration;
      }));
    }
    await page.locator('[role="tab"][data-surface-id="perf-overview"]').click();
    await page.evaluate(() => {
      const overview = document.querySelector('[data-testid="agents-overview-container"]');
      if (!overview) throw new Error("Agents Overview container is unavailable");
      const runtime = window.__WARDIAN_WORKBENCH_PERF__;
      runtime.overview_resize_started_at = performance.now();
      runtime.overview_last_resize_at = performance.now();
      new ResizeObserver(() => {
        runtime.overview_last_resize_at = performance.now();
      }).observe(overview);
    });
    const overviewSettle = [];
    for (const width of [1500, 1200, 900, 1400, 800, 1600]) {
      await page.evaluate(() => {
        window.__WARDIAN_WORKBENCH_PERF__.overview_resize_started_at = performance.now();
      });
      const started = performance.now();
      await page.setViewportSize({ width, height: 850 });
      await page.waitForFunction(() => {
        const runtime = window.__WARDIAN_WORKBENCH_PERF__;
        return runtime.overview_last_resize_at >= runtime.overview_resize_started_at
          && performance.now() - runtime.overview_last_resize_at >= 120;
      });
      await twoFrames(page); overviewSettle.push(performance.now() - started);
    }
    const heavyResume = [];
    const heavyGrace = fixture.benchmark.heavy_surface_hidden_grace_ms;
    for (const surfaceId of ["perf-graph", "perf-garden", "perf-graph", "perf-garden"]) {
      await measureHeavySurfaceActivation(page, surfaceId);
      await measureTabActivation(page, "perf-agent-mirror-1");
      await waitForHeavyRendererReleased(page, surfaceId, heavyGrace + 5_000);
      heavyResume.push(await measureHeavySurfaceActivation(page, surfaceId));
    }
    const observed = await page.evaluate(() => {
      const runtime = window.__WARDIAN_WORKBENCH_PERF__;
      return {
        stream_gap_count: runtime.stream_gap_count,
        react_commits: [...runtime.react_commits],
        xterm_peak: runtime.xterm_peak,
        webgl_peak: runtime.webgl_peak,
      };
    });
    await page.close();
    if (errors.length > 0) throw new Error(`Production workbench emitted browser errors: ${JSON.stringify(errors)}`);
    if (observed.react_commits.length === 0) {
      throw new Error("React commit instrumentation produced no observed samples");
    }
    if (observed.xterm_peak < fixture.terminal_presentations.length) {
      throw new Error(
        `Expected ${fixture.terminal_presentations.length} measured terminal renderers, observed ${observed.xterm_peak}`,
      );
    }
    return {
      startup_restore_ms: summarize(startup, "startup restore"),
      tab_switch_ms: summarize(tabSwitch, "tab switch"),
      group_focus_ms: summarize(groupFocus, "group focus"),
      terminal_output_commit_ms: summarize(terminalOutput, "terminal output commit"),
      stream_gap_count: observed.stream_gap_count,
      overview_settle_ms: summarize(overviewSettle, "Overview settle"),
      heavy_surface_resume_ms: summarize(heavyResume, "heavy surface resume"),
      react_commit_max_ms: round(Math.max(0, ...observed.react_commits)),
      renderer_peaks: { xterm: observed.xterm_peak, webgl: observed.webgl_peak },
    };
  } finally {
    await browser?.close();
    await server.close();
  }
}

function passingSelfTestBaseline() {
  const timing = { samples: [1, 2, 3], median: 2, p95: 3, max: 3 };
  return {
    runtime: {
      startup_restore_ms: timing, tab_switch_ms: timing, group_focus_ms: timing,
      terminal_output_commit_ms: timing, stream_gap_count: 0, overview_settle_ms: timing,
      heavy_surface_resume_ms: timing, react_commit_max_ms: 3,
      renderer_peaks: { xterm: 4, webgl: 4 },
    },
    bundle: { production_delta_gzip_bytes: 1024 },
  };
}

async function selfTest() {
  const fixture = JSON.parse(await fs.readFile(fixturePath, "utf8"));
  const errors = fixtureErrors(fixture);
  if (errors.length > 0) throw new Error(errors.join("\n"));
  const baseline = passingSelfTestBaseline();
  const result = evaluateGates(baseline);
  if (result.checks.length !== Object.keys(gates).length) throw new Error("Self-test omitted a gate");
  const timingMetricKeys = {
    restore_p95_ms: "startup_restore_ms",
    tab_switch_p95_ms: "tab_switch_ms",
    group_focus_p95_ms: "group_focus_ms",
    terminal_output_commit_p95_ms: "terminal_output_commit_ms",
    overview_settle_p95_ms: "overview_settle_ms",
    heavy_surface_resume_p95_ms: "heavy_surface_resume_ms",
  };
  for (const metric of Object.keys(gates)) {
    const failed = structuredClone(baseline);
    if (metric === "bundle_delta_gzip_bytes") failed.bundle.production_delta_gzip_bytes = gates[metric].limit + 1;
    else if (metric === "xterm_renderer_peak") failed.runtime.renderer_peaks.xterm = gates[metric].limit + 1;
    else if (metric === "webgl_context_peak") failed.runtime.renderer_peaks.webgl = gates[metric].limit + 1;
    else if (metric === "stream_gap_count") failed.runtime.stream_gap_count = 1;
    else if (metric === "react_commit_max_ms") failed.runtime.react_commit_max_ms = gates[metric].limit + 1;
    else {
      const key = timingMetricKeys[metric];
      failed.runtime[key].p95 = gates[metric].limit + 1;
    }
    try { evaluateGates(failed); throw new Error(`Gate ${metric} did not fail closed`); }
    catch (error) { if (!String(error).includes("gate failure")) throw error; }
  }
  const missing = structuredClone(baseline);
  delete missing.runtime.group_focus_ms;
  try { evaluateGates(missing); throw new Error("Missing observation did not fail closed"); }
  catch (error) { if (!String(error).includes("Missing observed metric")) throw error; }
  process.stdout.write(`${JSON.stringify({ self_test: "passed", fixture: fixture.scenario, gates }, null, 2)}\n`);
}

async function main() {
  const home = isolatedWardianHome(process.env.WARDIAN_HOME);
  if (process.argv.includes("--validate-home-only")) {
    process.stdout.write(`${home}\n`);
    return;
  }
  if (process.argv.includes("--check")) {
    const baseline = JSON.parse(await fs.readFile(baselinePath, "utf8"));
    const gateResult = evaluateGates(baseline);
    process.stdout.write(`${JSON.stringify({
      baseline: path.relative(repoRoot, baselinePath),
      gates: gateResult,
    }, null, 2)}\n`);
    return;
  }
  const fixture = JSON.parse(await fs.readFile(fixturePath, "utf8"));
  const errors = fixtureErrors(fixture);
  if (errors.length > 0) throw new Error(`Invalid performance fixture:\n${errors.join("\n")}`);
  await fs.mkdir(home, { recursive: true });
  const seedFile = await seedHome(home, fixture);
  // Bundle comparison and the benchmark-only production build both control
  // Vite compile-time flags. Keep them serialized before serving static output.
  const bundle = await productionBundleDelta(home);
  const runtimeOutDir = await buildProductionRuntime(home, fixture);
  const runtime = await measureRuntime(fixture, runtimeOutDir);
  const baseline = {
    schema_version: 1,
    measured_at: new Date().toISOString(),
    environment: {
      platform: process.platform,
      arch: process.arch,
      node: process.version,
      wardian_home: "isolated",
      renderer_build: "production",
      react_runtime: "react-dom/profiling",
      heavy_surface_hidden_grace_ms: fixture.benchmark.heavy_surface_hidden_grace_ms,
    },
    scenario: { fixture: seedFile, groups: 4, tabs: 20, agents: 20, owner: 1, mirrors: 3 },
    bundle,
    runtime,
  };
  baseline.gates = evaluateGates(baseline);
  await fs.mkdir(path.dirname(baselinePath), { recursive: true });
  await fs.writeFile(baselinePath, `${JSON.stringify(baseline, null, 2)}\n`, "utf8");
  process.stdout.write(`${JSON.stringify({ baseline: path.relative(repoRoot, baselinePath), gates: baseline.gates }, null, 2)}\n`);
}

const operation = process.argv.includes("--self-test") ? selfTest : main;
operation().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
