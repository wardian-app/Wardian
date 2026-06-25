import { chromium } from "@playwright/test";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const host = process.env.WARDIAN_PERF_HOST ?? "127.0.0.1";
const port = Number.parseInt(process.env.WARDIAN_PERF_PORT ?? "1422", 10);
const baseURL = process.env.WARDIAN_PERF_BASE_URL ?? `http://${host}:${port}`;
const wardianHome = process.env.WARDIAN_HOME || path.join(os.homedir(), ".wardian");
const iterations = Number.parseInt(process.env.WARDIAN_PERF_ITERATIONS ?? "7", 10);
const agentLimit = Number.parseInt(process.env.WARDIAN_PERF_AGENT_LIMIT ?? "0", 10);
const queueLimit = Number.parseInt(process.env.WARDIAN_PERF_QUEUE_LIMIT ?? "0", 10);
const outputDir = path.join(repoRoot, ".tmp", "perf");
const wardianCommand = process.platform === "win32" ? "wardian.cmd" : "wardian";

const views = [
  { label: "Grid", mode: "grid", ready: '[data-testid="agent-grid"]' },
  { label: "Dashboard", mode: "dashboard", ready: "main" },
  { label: "Queue", mode: "queue", ready: "main" },
  { label: "Graph", mode: "graph", ready: '[data-testid="graph-view"]' },
  { label: "Garden", mode: "garden", ready: ".garden-canvas canvas" },
  { label: "Library", mode: "library", ready: '[data-testid="library-view"]' },
  { label: "Workflows", mode: "workflows", ready: '[data-testid="workflows-view"]' },
];

function runJson(command, args) {
  const spawnCommand = process.platform === "win32" ? "cmd.exe" : command;
  const spawnArgs = process.platform === "win32"
    ? ["/d", "/s", "/c", command, ...args]
    : args;
  const result = spawnSync(spawnCommand, spawnArgs, {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed:\n${result.error?.message || result.stderr || result.stdout}`);
  }
  return JSON.parse(result.stdout);
}

function readJsonIfExists(filePath, fallback) {
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch {
    return fallback;
  }
}

function libraryItemType(libraryType, filePath) {
  if (libraryType === "skills") return "Skill";
  return "Prompt";
}

function buildLibraryTree(rootPath, libraryType) {
  const rootName = path.basename(rootPath);
  if (!fs.existsSync(rootPath)) {
    return { type: "Folder", path: "", name: rootName, children: [] };
  }

  function walk(currentPath) {
    const children = fs.readdirSync(currentPath, { withFileTypes: true })
      .filter((entry) => !entry.name.startsWith("."))
      .map((entry) => {
        const childPath = path.join(currentPath, entry.name);
        const relPath = path.relative(rootPath, childPath).replaceAll("\\", "/");
        if (entry.isDirectory()) {
          return walk(childPath);
        }
        return {
          type: libraryItemType(libraryType, childPath),
          path: relPath,
          name: path.basename(entry.name, path.extname(entry.name)),
          description: "",
          content: "",
          metadata: { id: relPath, tags: [], is_starred: false },
        };
      });
    return {
      type: "Folder",
      path: path.relative(rootPath, currentPath).replaceAll("\\", "/"),
      name: path.basename(currentPath),
      children,
    };
  }

  return walk(rootPath);
}

function listWorkflowRefs(workflowsRoot) {
  const refs = [];
  if (!fs.existsSync(workflowsRoot)) return refs;

  function walk(currentPath) {
    for (const entry of fs.readdirSync(currentPath, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue;
      const childPath = path.join(currentPath, entry.name);
      if (entry.isDirectory()) {
        walk(childPath);
      } else if (entry.name.endsWith(".md") || entry.name.endsWith(".json")) {
        const rel = path.relative(workflowsRoot, childPath).replaceAll("\\", "/");
        refs.push({
          id: rel.replace(/\.[^.]+$/, "").replace(/[^a-zA-Z0-9_-]+/g, "-"),
          path: childPath,
        });
      }
    }
  }

  walk(workflowsRoot);
  return refs;
}

function listWorkflowRuns(logsRoot) {
  const runs = [];
  if (!fs.existsSync(logsRoot)) return runs;

  function walk(currentPath) {
    for (const entry of fs.readdirSync(currentPath, { withFileTypes: true })) {
      const childPath = path.join(currentPath, entry.name);
      if (entry.isDirectory()) {
        walk(childPath);
        continue;
      }
      if (!entry.name.endsWith(".json")) continue;

      const summary = readWorkflowRunSummary(childPath, logsRoot);
      if (summary) runs.push(summary);
    }
  }

  walk(logsRoot);
  return runs.sort((left, right) => (right.updated_at ?? "").localeCompare(left.updated_at ?? ""));
}

function readWorkflowRunSummary(filePath, logsRoot) {
  const raw = readJsonIfExists(filePath, null);
  if (!raw) return null;

  const relativeParts = path.relative(logsRoot, filePath).split(path.sep);
  const workflowId = relativeParts[0] || "workflow";
  const stat = fs.statSync(filePath);
  const updatedAt = stat.mtime.toISOString();

  if (entryLooksLikeRunState(raw)) {
    return {
      run_id: raw.run_id,
      blueprint_id: raw.blueprint_id,
      status: normalizeRunStatus(raw.status),
      node_count: Object.keys(raw.nodes ?? {}).length,
      path: filePath,
      updated_at: updatedAt,
      completed_at: raw.status === "running" || raw.status === "awaiting_approval" ? null : updatedAt,
      failure: raw.failure ?? null,
    };
  }

  if (Array.isArray(raw)) {
    const failed = raw.some((event) => event?.status === "failed" || event?.error);
    const runId = path.basename(filePath, ".json");
    return {
      run_id: runId,
      blueprint_id: raw.find((event) => event?.workflow_id)?.workflow_id ?? workflowId,
      status: failed ? "failed" : "completed",
      node_count: raw.length,
      path: filePath,
      updated_at: updatedAt,
      completed_at: updatedAt,
      failure: failed ? raw.find((event) => event?.error)?.error ?? "Run failed" : null,
    };
  }

  return null;
}

function entryLooksLikeRunState(value) {
  return value && typeof value === "object" && typeof value.run_id === "string" && typeof value.blueprint_id === "string";
}

function normalizeRunStatus(status) {
  if (status === "running" || status === "awaiting_approval" || status === "failed" || status === "completed") return status;
  return "completed";
}

function workflowNameFromPath(filePath) {
  try {
    const head = fs.readFileSync(filePath, "utf8").slice(0, 2000);
    const title = head.match(/^#\s+(.+)$/m)?.[1]?.trim();
    if (title) return title;
  } catch {
    // Fall through to filename.
  }
  return path.basename(filePath, path.extname(filePath));
}

function normalizeAgents(cliAgents) {
  return cliAgents.map((agent) => ({
    session_id: agent.uuid,
    session_name: agent.name,
    agent_class: agent.class,
    folder: agent.workspace,
    provider: agent.provider,
    is_off: agent.status === "off",
  }));
}

function classDefinitions(agents) {
  return [...new Set(agents.map((agent) => agent.agent_class).filter(Boolean))]
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({ name, description: "", is_default: true }));
}

function telemetryForAgents(agents) {
  return agents.map((agent) => ({
    session_id: agent.session_id,
    cpu_usage: 0,
    memory_mb: 0,
    uptime_seconds: 0,
    query_count: 0,
    init_timestamp: null,
    current_status: agent.is_off ? "Off" : "Idle",
    log_path: null,
  }));
}

function collectSnapshot() {
  const cliAgents = runJson(wardianCommand, ["agent", "list", "--scope", "all", "--verbose"]).agents;
  const agents = normalizeAgents(agentLimit > 0 ? cliAgents.slice(0, agentLimit) : cliAgents);
  const watchlists = readJsonIfExists(path.join(wardianHome, "watchlists", "index.json"), {
    version: 2,
    watchlists: [],
    teams: [],
  });
  const allQueueItems = readJsonIfExists(path.join(wardianHome, "queue", "items.json"), []);
  const queueItems = queueLimit > 0 ? allQueueItems.slice(0, queueLimit) : allQueueItems;
  const queuePreferences = readJsonIfExists(path.join(wardianHome, "queue", "preferences.json"), {});
  const promptTree = buildLibraryTree(path.join(wardianHome, "library", "prompts"), "prompts");
  const skillTree = buildLibraryTree(path.join(wardianHome, "library", "skills"), "skills");
  const workflowRefs = listWorkflowRefs(path.join(wardianHome, "library", "workflows"));
  const workflowRuns = listWorkflowRuns(path.join(wardianHome, "logs", "workflows"));
  const schedules = readJsonIfExists(path.join(wardianHome, "library", "schedules.json"), []);

  return {
    captured_at: new Date().toISOString(),
    wardian_home: wardianHome,
    agents,
    classes: classDefinitions(agents),
    telemetry: telemetryForAgents(agents),
    watchlists,
    queueItems,
    queuePreferences,
    promptTree,
    skillTree,
    workflowRefs,
    workflowRuns,
    schedules,
  };
}

async function waitForServer(url, timeoutMs = 60_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await urlResponds(url)) return;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${url}`);
}

function urlResponds(url) {
  return new Promise((resolve) => {
    const request = http.get(url, (response) => {
      response.resume();
      resolve(response.statusCode && response.statusCode < 500);
    });
    request.on("error", () => resolve(false));
    request.setTimeout(1000, () => {
      request.destroy();
      resolve(false);
    });
  });
}

async function startViteIfNeeded() {
  if (await urlResponds(baseURL)) return null;
  const npmCommand = process.platform === "win32" ? "npm.cmd" : "npm";
  const npmArgs = ["run", "vite", "--", "--host", host, "--port", String(port), "--strictPort"];
  const spawnCommand = process.platform === "win32" ? "cmd.exe" : npmCommand;
  const spawnArgs = process.platform === "win32"
    ? ["/d", "/s", "/c", npmCommand, ...npmArgs]
    : npmArgs;
  const child = spawn(
    spawnCommand,
    spawnArgs,
    {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, WARDIAN_HOME: wardianHome },
    },
  );
  child.stdout.on("data", (chunk) => process.stdout.write(chunk));
  child.stderr.on("data", (chunk) => process.stderr.write(chunk));
  await waitForServer(baseURL);
  return child;
}

function stopVite(child) {
  if (!child) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], {
      cwd: repoRoot,
      stdio: "ignore",
    });
    return;
  }
  child.kill();
}

function installTauriMock(snapshot) {
  const callbacks = new Map();
  let callbackId = 1;
  const tauriWindow = window;

  tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => undefined,
  };

  tauriWindow.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
    transformCallback: (callback) => {
      const id = callbackId++;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback: (id) => callbacks.delete(id),
    convertFileSrc: (filePath) => filePath,
    invoke: async (command, args) => {
      if (command === "list_agents") return snapshot.agents;
      if (command === "list_agent_classes") return snapshot.classes;
      if (command === "list_provider_readiness") {
        return ["claude", "codex", "gemini", "antigravity", "opencode"].map((provider) => ({
          provider,
          display_name: provider,
          available: true,
          executable: provider,
          reason: null,
        }));
      }
      if (command === "load_watchlists") return snapshot.watchlists;
      if (command === "load_watchlist_prefs") return null;
      if (command === "load_agent_interactions") return {};
      if (command === "load_queue_items") return snapshot.queueItems;
      if (command === "load_queue_preferences") return snapshot.queuePreferences;
      if (command === "save_queue_items" || command === "save_queue_preferences") return null;
      if (command === "load_onboarding_hints") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
      if (command === "dismiss_onboarding_hint") return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
      if (command === "get_library_tree") {
        return args?.libraryType === "skills" ? snapshot.skillTree : snapshot.promptTree;
      }
      if (command === "library_watch" || command === "library_unwatch") return null;
      if (command === "list_deployed_skills" || command === "list_deployed_skill_refs" || command === "list_skill_deployments") return [];
      if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
      if (command === "save_workflow_library") return null;
      if (command === "list_workflows") return snapshot.workflowRefs;
      if (command === "list_scheduled_runs") return snapshot.schedules;
      if (command === "schedule_list") return snapshot.schedules.map((schedule) => ({ ...schedule }));
      if (command === "workflow_list_blueprints") return snapshot.workflowRefs;
      if (command === "workflow_parse") {
        const ref = snapshot.workflowRefs.find((candidate) => candidate.path === args?.path);
        const id = ref?.id ?? "workflow";
        return {
          blueprint: {
            id,
            name: ref?.path ? window.__wardianWorkflowNames?.[ref.path] ?? id : id,
            nodes: [{ id: "start", type: "trigger", fields: {} }],
            edges: [],
          },
          diagnostics: [],
        };
      }
      if (command === "workflow_list_runs") return snapshot.workflowRuns.map((run) => ({ ...run }));
      if (command === "sync_provider_theme_settings") return null;
      if (command === "plugin:event|listen") return callbackId++;
      if (command === "plugin:event|unlisten") return null;
      return null;
    },
  };

  tauriWindow.__wardianWorkflowNames = Object.fromEntries(
    snapshot.workflowRefs.map((ref) => [ref.path, ref.name ?? ref.id]),
  );
}

async function measureView(page, view) {
  const start = await page.evaluate(() => performance.now());
  await page.locator(".titlebar-tab").filter({ hasText: view.label }).first().click();
  await page.locator(view.ready).first().waitFor({ state: "visible", timeout: 20_000 });
  await page.waitForFunction(
    (label) => [...document.querySelectorAll(".titlebar-tab.active")].some((tab) => tab.textContent?.includes(label)),
    view.label,
  );
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const end = await page.evaluate(() => performance.now());
  return end - start;
}

async function measureWorkflowHistory(page) {
  const openSamples = [];
  const scrollSamples = [];
  let scrollProfiles = null;
  for (let i = 0; i < iterations; i += 1) {
    await page.locator(".titlebar-tab").filter({ hasText: "Grid" }).first().click();
    await page.locator('[data-testid="agent-grid"]').waitFor({ state: "visible", timeout: 20_000 });

    const openStart = await page.evaluate(() => performance.now());
    await page.locator(".titlebar-tab").filter({ hasText: "Workflows" }).first().click();
    await page.locator('[data-testid="workflows-view"]').waitFor({ state: "visible", timeout: 20_000 });
    await page.getByRole("button", { name: "Monitor" }).click();
    await page.locator('[data-testid="workflow-monitor"]').waitFor({ state: "visible", timeout: 20_000 });
    await page.getByRole("button", { name: "History" }).click();
    await page.getByRole("heading", { name: "History" }).waitFor({ state: "visible", timeout: 20_000 });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const openEnd = await page.evaluate(() => performance.now());
    openSamples.push(openEnd - openStart);

    scrollSamples.push(await page.evaluate(async () => {
      const scroller = document.querySelector('[data-testid="workflow-monitor"] .overflow-y-auto');
      const start = performance.now();
      if (scroller instanceof HTMLElement) {
        for (let step = 1; step <= 5; step += 1) {
          scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) * (step / 5);
          await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        }
      }
      return performance.now() - start;
    }));

    if (i === 0) {
      scrollProfiles = [
        await measureHistoryWheelProfile(page, "initial"),
        await expandHistoryRowsAndProfile(page, 110),
        await expandHistoryRowsAndProfile(page, 510),
      ];
    }
  }

  return {
    label: "Workflow History",
    open: summarize(openSamples),
    scroll: summarize(scrollSamples),
    scrollProfiles,
  };
}

async function expandHistoryRowsAndProfile(page, minimumRows) {
  for (let guard = 0; guard < 80; guard += 1) {
    const logicalRowCount = await historyLogicalRowCount(page);
    if (logicalRowCount >= minimumRows) break;
    const showMore = page.getByRole("button", { name: /show .*older/i });
    if (await showMore.count() === 0) break;
    await showMore.click();
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  }
  await page.waitForFunction(() => {
    const virtualList = document.querySelector('[data-testid="workflow-history-virtual-list"]');
    if (!(virtualList instanceof HTMLElement)) return true;
    const rendered = Number.parseInt(virtualList.dataset.renderedCount ?? "0", 10);
    return Number.isFinite(rendered) && virtualList.children.length <= Math.max(32, rendered + 2);
  }, null, { timeout: 5_000 }).catch(() => undefined);
  return measureHistoryWheelProfile(page, `rows-${minimumRows}`);
}

async function historyLogicalRowCount(page) {
  return page.evaluate(() => {
    const virtualList = document.querySelector('[data-testid="workflow-history-virtual-list"]');
    if (virtualList instanceof HTMLElement) return Math.round(virtualList.offsetHeight / 128);
    return document.querySelectorAll('[data-testid^="workflow-history-run-"]').length;
  });
}

async function measureHistoryWheelProfile(page, label) {
  const scroller = page.locator('[data-testid="workflow-monitor"] .overflow-y-auto').first();
  const box = await scroller.boundingBox();
  if (!box) return { label, available: false };
  const rowCount = await page.locator('[data-testid^="workflow-history-run-"]').count();
  const domNodeCount = await page.evaluate(() => document.querySelectorAll("*").length);
  const layout = await page.evaluate(() => {
    const scroller = document.querySelector('[data-testid="workflow-monitor"] .overflow-y-auto');
    const virtualList = document.querySelector('[data-testid="workflow-history-virtual-list"]');
    return {
      scrollerClientHeight: scroller instanceof HTMLElement ? scroller.clientHeight : null,
      scrollerScrollHeight: scroller instanceof HTMLElement ? scroller.scrollHeight : null,
      virtualListHeight: virtualList instanceof HTMLElement ? virtualList.offsetHeight : null,
      virtualListChildren: virtualList instanceof HTMLElement ? virtualList.children.length : null,
      virtualListRenderedCount: virtualList instanceof HTMLElement ? virtualList.dataset.renderedCount : null,
    };
  });
  await scroller.evaluate((el) => {
    el.scrollTop = 0;
  });
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.evaluate(() => {
    const frames = [];
    let last = performance.now();
    let running = true;
    function tick(now) {
      frames.push(now - last);
      last = now;
      if (running) requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
    window.__wardianStopFrameProbe = () => {
      running = false;
      const sorted = [...frames].sort((left, right) => left - right);
      return {
        frames: frames.length,
        maxFrameMs: sorted[sorted.length - 1] ?? 0,
        p95FrameMs: sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1)] ?? 0,
        over16ms: frames.filter((value) => value > 16.7).length,
        over33ms: frames.filter((value) => value > 33.4).length,
      };
    };
  });
  const start = await page.evaluate(() => performance.now());
  for (let i = 0; i < 24; i += 1) {
    await page.mouse.wheel(0, 260);
    await page.waitForTimeout(12);
  }
  await page.waitForTimeout(100);
  const durationMs = await page.evaluate((startTime) => performance.now() - startTime, start);
  const frameProfile = await page.evaluate(() => window.__wardianStopFrameProbe?.() ?? null);
  const scrollTop = await scroller.evaluate((el) => el.scrollTop);
  return {
    label,
    available: true,
    rowCount,
    domNodeCount,
    layout,
    durationMs,
    scrollTop,
    ...frameProfile,
  };
}

function summarize(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const sum = samples.reduce((acc, value) => acc + value, 0);
  return {
    min_ms: sorted[0],
    median_ms: sorted[Math.floor(sorted.length / 2)],
    p95_ms: sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1)],
    max_ms: sorted[sorted.length - 1],
    mean_ms: sum / samples.length,
    samples_ms: samples,
  };
}

function printSummary(result) {
  console.log(`\nWardian view performance (${result.snapshot.agents} agents, ${result.snapshot.queueItems} queue items)`);
  for (const view of result.views) {
    console.log(`${view.label.padEnd(10)} median=${view.median_ms.toFixed(1)}ms p95=${view.p95_ms.toFixed(1)}ms max=${view.max_ms.toFixed(1)}ms`);
  }
  if (result.interactions?.workflowHistory) {
    const history = result.interactions.workflowHistory;
    console.log(`Workflow History open median=${history.open.median_ms.toFixed(1)}ms p95=${history.open.p95_ms.toFixed(1)}ms`);
    console.log(`Workflow History scroll median=${history.scroll.median_ms.toFixed(1)}ms p95=${history.scroll.p95_ms.toFixed(1)}ms`);
  }
  console.log(`\nWrote ${result.output_file}`);
}

let vite = null;
let browser = null;
try {
  fs.mkdirSync(outputDir, { recursive: true });
  const snapshot = collectSnapshot();
  snapshot.workflowRefs = snapshot.workflowRefs.map((ref) => ({
    ...ref,
    name: workflowNameFromPath(ref.path),
  }));
  vite = await startViteIfNeeded();
  browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
  await page.addInitScript(installTauriMock, snapshot);
  await page.goto(baseURL, { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ state: "visible", timeout: 20_000 });

  const results = [];
  for (const view of views) {
    const samples = [];
    for (let i = 0; i < iterations; i += 1) {
      await page.locator(".titlebar-tab").filter({ hasText: "Grid" }).first().click();
      await page.locator('[data-testid="agent-grid"]').waitFor({ state: "visible", timeout: 20_000 });
      samples.push(await measureView(page, view));
    }
    results.push({ label: view.label, mode: view.mode, ...summarize(samples) });
  }
  const workflowHistory = await measureWorkflowHistory(page);

  const outputFile = path.join(outputDir, `view-performance-${new Date().toISOString().replace(/[:.]/g, "-")}.json`);
  const result = {
    captured_at: new Date().toISOString(),
    baseURL,
    iterations,
    snapshot: {
      agents: snapshot.agents.length,
      queueItems: snapshot.queueItems.length,
      agentLimit: agentLimit || null,
      queueLimit: queueLimit || null,
      watchlists: snapshot.watchlists.watchlists?.length ?? 0,
      teams: snapshot.watchlists.teams?.length ?? 0,
      promptRootChildren: snapshot.promptTree.children.length,
      skillRootChildren: snapshot.skillTree.children.length,
      workflows: snapshot.workflowRefs.length,
      workflowRuns: snapshot.workflowRuns.length,
    },
    views: results,
    interactions: {
      workflowHistory,
    },
    output_file: outputFile,
  };
  fs.writeFileSync(outputFile, `${JSON.stringify(result, null, 2)}\n`);
  printSummary(result);
} finally {
  if (browser) await browser.close();
  stopVite(vite);
}
