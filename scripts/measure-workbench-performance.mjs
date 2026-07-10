import fsSync from "node:fs";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { performance } from "node:perf_hooks";
import { chromium } from "@playwright/test";
import { build, createServer } from "vite";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const baselinePath = path.join(repoRoot, "docs", "research", "workbench-navigation", "dockview-baseline.json");
const harnessPath = path.join(repoRoot, "src", "layout", "workbench", "proof", "DockviewEvaluationHarness.tsx");
const viteConfigPath = path.join(repoRoot, "vite.config.ts");

function resolveIsolatedWardianHome(rawValue) {
  if (typeof rawValue !== "string" || rawValue.trim() === "") {
    throw new Error("WARDIAN_HOME is required and must name an isolated temporary directory");
  }
  const normalized = path.resolve(rawValue.trim());
  const profileRoot = path.resolve(os.homedir());
  const productionHome = path.resolve(profileRoot, ".wardian");
  if (samePath(normalized, profileRoot)) {
    throw new Error("WARDIAN_HOME must not resolve to the user profile root");
  }
  if (samePath(normalized, productionHome)) {
    throw new Error("WARDIAN_HOME must not resolve to the production Wardian home");
  }
  const resolved = fsSync.existsSync(normalized) ? fsSync.realpathSync.native(normalized) : normalized;
  if (samePath(resolved, profileRoot)) {
    throw new Error("WARDIAN_HOME must not resolve to the user profile root");
  }
  if (samePath(resolved, productionHome)) {
    throw new Error("WARDIAN_HOME must not resolve to the production Wardian home");
  }
  const normalizedTempRoot = path.resolve(os.tmpdir());
  const tempRoot = fsSync.existsSync(normalizedTempRoot)
    ? fsSync.realpathSync.native(normalizedTempRoot)
    : normalizedTempRoot;
  const relativeToTemp = path.relative(tempRoot, resolved);
  const insideTemp = relativeToTemp !== ""
    && !relativeToTemp.startsWith(`..${path.sep}`)
    && relativeToTemp !== ".."
    && !path.isAbsolute(relativeToTemp);
  if (!insideTemp || !path.basename(resolved).startsWith("wardian-workbench-proof-")) {
    throw new Error(
      "WARDIAN_HOME must be an isolated directory under the OS temp root named wardian-workbench-proof-*",
    );
  }
  return resolved;
}

function samePath(left, right) {
  return process.platform === "win32"
    ? left.toLowerCase() === right.toLowerCase()
    : left === right;
}

function assertInside(parent, candidate) {
  const relative = path.relative(parent, candidate);
  if (relative === "" || relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`Refusing filesystem mutation outside isolated WARDIAN_HOME: ${candidate}`);
  }
}

async function seedIsolatedHome(wardianHome) {
  const settingsDir = path.join(wardianHome, "settings");
  const seedPath = path.join(settingsDir, "workbench-proof-seed.json");
  assertInside(wardianHome, settingsDir);
  assertInside(wardianHome, seedPath);
  await fs.mkdir(settingsDir, { recursive: true });
  const seed = {
    schema_version: 1,
    scenario: "dockview-adapter-proof",
    group_count: 4,
    tab_count: 20,
    layout_source: "wardian-proof-model",
    contains_dockview_json: false,
  };
  await fs.writeFile(seedPath, `${JSON.stringify(seed, null, 2)}\n`, "utf8");
  return seedPath;
}

async function measureBundles(wardianHome) {
  const buildRoot = path.join(wardianHome, "bundle-measurement", `${Date.now()}`);
  const baseOutDir = path.join(buildRoot, "base");
  const candidateOutDir = path.join(buildRoot, "candidate");
  assertInside(wardianHome, buildRoot);
  assertInside(wardianHome, baseOutDir);
  assertInside(wardianHome, candidateOutDir);
  await fs.mkdir(buildRoot, { recursive: true });

  await build({
    root: repoRoot,
    configFile: viteConfigPath,
    logLevel: "warn",
    build: {
      outDir: baseOutDir,
      emptyOutDir: true,
    },
  });
  await build({
    root: repoRoot,
    configFile: viteConfigPath,
    logLevel: "warn",
    build: {
      outDir: candidateOutDir,
      emptyOutDir: true,
      rollupOptions: {
        input: {
          app: path.join(repoRoot, "index.html"),
          "workbench-proof": harnessPath,
        },
      },
    },
  });

  const base = await collectBundleSize(baseOutDir);
  const candidate = await collectBundleSize(candidateOutDir);
  return {
    base_raw_bytes: base.raw_bytes,
    base_gzip_bytes: base.gzip_bytes,
    candidate_raw_bytes: candidate.raw_bytes,
    candidate_gzip_bytes: candidate.gzip_bytes,
    production_delta_raw_bytes: candidate.raw_bytes - base.raw_bytes,
    production_delta_gzip_bytes: candidate.gzip_bytes - base.gzip_bytes,
    candidate_files: candidate.files,
  };
}

async function collectBundleSize(outDir) {
  const files = await listFiles(outDir);
  const assets = files.filter((file) => /\.(?:js|css)$/i.test(file));
  let rawBytes = 0;
  let gzipBytes = 0;
  const entries = [];
  for (const file of assets) {
    const content = await fs.readFile(file);
    const raw = content.byteLength;
    const gzip = gzipSync(content).byteLength;
    rawBytes += raw;
    gzipBytes += gzip;
    entries.push({
      file: path.relative(outDir, file).replaceAll(path.sep, "/"),
      raw_bytes: raw,
      gzip_bytes: gzip,
    });
  }
  entries.sort((left, right) => right.raw_bytes - left.raw_bytes);
  return { raw_bytes: rawBytes, gzip_bytes: gzipBytes, files: entries };
}

async function listFiles(directory) {
  const entries = await fs.readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const resolved = path.join(directory, entry.name);
    return entry.isDirectory() ? listFiles(resolved) : [resolved];
  }));
  return nested.flat();
}

async function measureRuntime() {
  const server = await createServer({
    root: repoRoot,
    configFile: viteConfigPath,
    logLevel: "error",
    server: {
      host: "127.0.0.1",
      port: 0,
      strictPort: false,
    },
  });
  let browser;
  try {
    await server.listen();
    const baseUrl = server.resolvedUrls?.local[0]?.replace(/\/$/, "");
    if (!baseUrl) throw new Error("Vite did not expose a local measurement URL");
    browser = await chromium.launch();
    const browserVersion = browser.version();
    const warmupPage = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    await installBrowserInstrumentation(warmupPage);
    await installProofHostRoute(warmupPage);
    await warmupPage.goto(`${baseUrl}/__workbench-proof-measurement.html`, { waitUntil: "domcontentloaded" });
    await warmupPage.locator('[data-testid="workbench-proof"][data-ready="true"]').waitFor({ timeout: 20_000 });
    await warmupPage.locator('[data-testid="proof-graph-wrapper"]').waitFor({ state: "attached", timeout: 20_000 });
    await warmupPage.locator('[data-testid="proof-garden-wrapper"]').waitFor({ state: "attached", timeout: 20_000 });
    await warmupPage.close();

    const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    const consoleErrors = [];
    const pageErrors = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await installBrowserInstrumentation(page);
    await installProofHostRoute(page);

    const startupStartedAt = performance.now();
    await page.goto(`${baseUrl}/__workbench-proof-measurement.html`, { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="workbench-proof"][data-ready="true"]').waitFor({ timeout: 20_000 });
    const startupMs = performance.now() - startupStartedAt;
    await page.locator('[data-testid="proof-graph-wrapper"]').waitFor({ state: "attached", timeout: 20_000 });
    await page.locator('[data-testid="proof-garden-wrapper"]').waitFor({ state: "attached", timeout: 20_000 });
    const heavyRenderersReadyMs = performance.now() - startupStartedAt;

    const tabSwitchMs = await page.evaluate(async () => {
      const runtime = window.__WARDIAN_WORKBENCH_PROOF__;
      if (!runtime) throw new Error("proof runtime is unavailable");
      const ids = ["graph", "terminal-owner", "synthetic-01", "terminal-owner"];
      const samples = [];
      for (let index = 0; index < 16; index += 1) {
        const startedAt = performance.now();
        runtime.commands.activateSurface(ids[index % ids.length]);
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        samples.push(performance.now() - startedAt);
      }
      return samples;
    });

    const dragMs = [];
    for (let index = 0; index < 3; index += 1) {
      const source = page.getByRole("tab", { name: "Terminal Owner", exact: true });
      const target = page.getByRole("tab", { name: "Terminal Mirror 1", exact: true });
      await source.click();
      await source.focus();
      await page.keyboard.press("Control+]");
      await page.keyboard.press("F6");
      const sourceBox = await source.boundingBox();
      const targetBox = await target.boundingBox();
      if (!sourceBox || !targetBox) throw new Error("Dockview drag handles are not measurable");
      const startedAt = performance.now();
      await page.mouse.move(sourceBox.x + sourceBox.width / 2, sourceBox.y + sourceBox.height / 2);
      await page.mouse.down();
      await page.mouse.move(targetBox.x + targetBox.width / 2, targetBox.y + targetBox.height / 2, { steps: 16 });
      await page.mouse.up();
      await page.waitForFunction(() => window.__WARDIAN_WORKBENCH_PROOF__?.getModel()
        .groups.find((group) => group.group_id === "proof-group-2")
        ?.surface_ids.includes("terminal-owner") === true);
      dragMs.push(performance.now() - startedAt);
      await page.evaluate(() => window.__WARDIAN_WORKBENCH_PROOF__?.commands
        .moveSurface("terminal-owner", "proof-group-1"));
      await page.waitForFunction(() => window.__WARDIAN_WORKBENCH_PROOF__?.getModel()
        .groups.find((group) => group.group_id === "proof-group-1")
        ?.surface_ids.includes("terminal-owner") === true);
    }

    const terminalOutputMs = await page.evaluate(async () => {
      const runtime = window.__WARDIAN_WORKBENCH_PROOF__;
      if (!runtime) throw new Error("proof runtime is unavailable");
      const startedAt = performance.now();
      await runtime.commands.emitTerminalBurst(500);
      return performance.now() - startedAt;
    });

    const observed = await page.evaluate(() => {
      const runtime = window.__WARDIAN_WORKBENCH_PROOF__;
      if (!runtime) throw new Error("proof runtime is unavailable");
      const tabElements = [...document.querySelectorAll('[role="tab"]')];
      const separators = [...document.querySelectorAll(".dv-sash")];
      return {
        model: runtime.getModel(),
        metrics: structuredClone(runtime.metrics),
        renderer_counts: {
          mounted_surfaces: Object.keys(runtime.metrics.surface_mounts).length,
          mounted_terminal_hosts: document.querySelectorAll('[data-testid^="proof-terminal-host-"]').length,
          mounted_graph_wrappers: document.querySelectorAll('[data-testid="proof-graph-wrapper"]').length,
          mounted_garden_wrappers: document.querySelectorAll('[data-testid="proof-garden-wrapper"]').length,
          canvas_elements: document.querySelectorAll("canvas").length,
          webgl_canvases: window.__proofWebglCanvases?.size ?? 0,
        },
        accessibility: {
          tab_count: tabElements.length,
          tablist_count: document.querySelectorAll('[role="tablist"]').length,
          selected_tab_count: tabElements.filter((element) => element.getAttribute("aria-selected") === "true").length,
          roving_tabstop_count: tabElements.filter((element) => element.getAttribute("tabindex") === "0").length,
          separator_count: separators.length,
          separators_with_role: separators.filter((element) => element.getAttribute("role") === "separator").length,
          separators_with_value: separators.filter((element) => element.hasAttribute("aria-valuenow")).length,
        },
      };
    });
    await page.close();

    if (consoleErrors.length > 0 || pageErrors.length > 0) {
      throw new Error(`Browser proof emitted errors: ${JSON.stringify({ consoleErrors, pageErrors })}`);
    }
    return {
      browser_version: browserVersion,
      startup_measurement: "fresh browser context after Vite transform warm-up",
      startup_ms: round(startupMs),
      heavy_renderers_ready_ms: round(heavyRenderersReadyMs),
      tab_switch_ms: summarize(tabSwitchMs),
      drag_ms: summarize(dragMs),
      terminal_output_500_lines_ms: round(terminalOutputMs),
      react_commit_measurement: "model publish to React layout effect",
      react_commit_count: observed.metrics.react_commit_count,
      react_commits: summarize(observed.metrics.react_commit_duration_ms),
      model_command_ms: summarize(observed.metrics.model_command_duration_ms),
      renderer_counts: observed.renderer_counts,
      surface_mounts: observed.metrics.surface_mounts,
      surface_unmounts: observed.metrics.surface_unmounts,
      terminal_write_chars: observed.metrics.terminal_write_chars,
      terminal_webgl_loaded: observed.metrics.terminal_webgl_loaded,
      terminal_webgl_failures: observed.metrics.terminal_webgl_failures,
      adapter_move_events: observed.metrics.adapter_move_events,
      accessibility: observed.accessibility,
      final_group_count: observed.model.groups.length,
      final_tab_count: Object.keys(observed.model.surfaces).length,
    };
  } finally {
    await browser?.close();
    await server.close();
  }
}

async function installProofHostRoute(page) {
  await page.route("**/__workbench-proof-measurement.html", (route) => route.fulfill({
    contentType: "text/html",
    body: proofHostHtml(),
  }));
}

async function installBrowserInstrumentation(page) {
  await page.addInitScript(() => {
    const tauriWindow = window;
    let callbackId = 1;
    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      transformCallback: () => callbackId++,
      unregisterCallback: () => undefined,
      convertFileSrc: (filePath) => filePath,
      invoke: async (command) => {
        if ([
          "list_agents", "list_agent_classes", "list_provider_readiness", "load_watchlists",
          "load_queue_items", "list_workflows", "list_scheduled_runs", "list_deployed_skills",
          "workflow_list_blueprints", "workflow_list_runs", "get_pair_activity", "list_available_shells",
        ].includes(command)) return [];
        if (command === "get_topology") return { edges: [], ignored_pairs: [], fallback_groups: [] };
        if (command === "load_agent_interactions" || command === "load_queue_preferences") return {};
        if (command === "plugin:event|listen") return callbackId++;
        return null;
      },
    };
    window.__proofWebglCanvases = new Set();
    const originalGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function getContext(type, ...args) {
      const context = originalGetContext.call(this, type, ...args);
      if (context && (type === "webgl" || type === "webgl2" || type === "experimental-webgl")) {
        window.__proofWebglCanvases.add(this);
      }
      return context;
    };
  });
}

function proofHostHtml() {
  return `<!doctype html>
    <html>
      <head>
        <meta charset="UTF-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <link rel="stylesheet" href="/src/styles/App.css" />
      </head>
      <body>
        <div id="workbench-proof-root"></div>
        <script type="module">
          import RefreshRuntime from "/@react-refresh";
          RefreshRuntime.injectIntoGlobalHook(window);
          window.$RefreshReg$ = () => {};
          window.$RefreshSig$ = () => (type) => type;
          window.__vite_plugin_react_preamble_installed__ = true;
        </script>
        <script type="module">
          import { mountDockviewEvaluationHarness } from "/src/layout/workbench/proof/DockviewEvaluationHarness.tsx";
          mountDockviewEvaluationHarness(document.getElementById("workbench-proof-root"));
        </script>
      </body>
    </html>`;
}

function summarize(values) {
  const samples = values.map(round).sort((left, right) => left - right);
  if (samples.length === 0) return { samples: [], median: 0, p95: 0, max: 0 };
  return {
    samples,
    median: percentile(samples, 0.5),
    p95: percentile(samples, 0.95),
    max: samples.at(-1),
  };
}

function percentile(sorted, ratio) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * ratio) - 1)];
}

function round(value) {
  return Math.round(value * 100) / 100;
}

async function main() {
  const wardianHome = resolveIsolatedWardianHome(process.env.WARDIAN_HOME);
  await fs.mkdir(wardianHome, { recursive: true });
  const seedPath = await seedIsolatedHome(wardianHome);
  const packageJson = JSON.parse(await fs.readFile(path.join(repoRoot, "package.json"), "utf8"));
  const lockfile = JSON.parse(await fs.readFile(path.join(repoRoot, "package-lock.json"), "utf8"));
  const harnessSource = await fs.readFile(harnessPath, "utf8");
  const bundle = await measureBundles(wardianHome);
  const runtime = await measureRuntime();
  const baseline = {
    schema_version: 1,
    measured_at: new Date().toISOString(),
    environment: {
      platform: process.platform,
      arch: process.arch,
      node: process.version,
      cpu: os.cpus()[0]?.model ?? "unknown",
      logical_cpu_count: os.cpus().length,
      total_memory_bytes: os.totalmem(),
      wardian_home: "isolated-os-temp",
    },
    package: {
      name: "dockview-react",
      version: packageJson.dependencies["dockview-react"],
      license: "MIT",
      unpacked_size_bytes: 3300226,
      react_version: lockfile.packages["node_modules/react"].version,
      react_dom_version: lockfile.packages["node_modules/react-dom"].version,
      react_peer_compatible: true,
    },
    scenario: {
      group_count: 4,
      tab_count: 20,
      xterm_owner_count: 1,
      xterm_mirror_count: 3,
      heavy_renderers: ["GraphView", "GardenView"],
      seed_file: path.relative(wardianHome, seedPath).replaceAll(path.sep, "/"),
    },
    bundle,
    runtime,
    adapter_boundary: {
      model_source: "wardian-proof-model",
      dockview_json_persisted: false,
      dockview_serialization_reference_count: (harnessSource.match(/\.(?:toJSON|fromJSON)\s*\(/g) ?? []).length,
      renderer_mode: "always",
      production_route_present: false,
    },
  };
  await fs.mkdir(path.dirname(baselinePath), { recursive: true });
  await fs.writeFile(baselinePath, `${JSON.stringify(baseline, null, 2)}\n`, "utf8");
  process.stdout.write(`${JSON.stringify({
    baseline: path.relative(repoRoot, baselinePath).replaceAll(path.sep, "/"),
    bundle_delta_raw_bytes: bundle.production_delta_raw_bytes,
    bundle_delta_gzip_bytes: bundle.production_delta_gzip_bytes,
    startup_ms: runtime.startup_ms,
    switch_p95_ms: runtime.tab_switch_ms.p95,
    drag_p95_ms: runtime.drag_ms.p95,
    react_commit_p95_ms: runtime.react_commits.p95,
    mounted_surfaces: runtime.renderer_counts.mounted_surfaces,
    webgl_canvases: runtime.renderer_counts.webgl_canvases,
  }, null, 2)}\n`);
}

main().catch((error) => {
  process.stderr.write(`workbench performance proof failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
  process.exitCode = 1;
});
