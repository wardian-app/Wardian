import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import type { AgentChatEvent, AgentChatPage } from "../../src/types";

// Browser evidence for the remote read UI; does not qualify a native/device read.
const viewports = [
  { name: "desktop", width: 1280, height: 900 },
  { name: "mobile", width: 390, height: 844 },
];
for (const viewport of viewports) {
  test(`${viewport.name} Chat retains rows through a local error and retries without re-pairing`, async ({ page }) => {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
    const agent = {
      session_id: "chat-fixture", session_name: "Chat Fixture", agent_class: "Coder",
      provider: "codex", workspace: "<agent-workspace>", status: "Idle", latest_text: null,
    };
    let reads = 0;
    await page.route("**/remote/api/**", async (route) => {
      const endpoint = new URL(route.request().url()).pathname;
      if (endpoint.endsWith("/chat")) {
        reads += 1;
        if (reads === 2) {
          await route.fulfill({ status: 400, json: { code: "agent_chat_provenance_failed", detail: "Private diagnostic must stay hidden" } });
          return;
        }
        await route.fulfill({ json: {
          session_id: agent.session_id, conversation_id: "fixture-conversation", generation: "fixture-generation",
          source_epoch: null, revision: `fixture-${reads}`, unchanged: false, reset: false,
          progress: "ready", aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0,
          events: [{
            id: reads === 1 ? "earlier" : "recovered", session_id: agent.session_id, provider: "codex",
            kind: "message", role: "assistant", text: reads === 1 ? "Earlier reply remains available." : "History loaded after retry.",
            title: null, status: null, turn_id: "fixture-turn", source: "provider_log", command: null,
            exit_code: null, path: null, language: null, created_at: "2026-01-01T00:00:00Z", sequence: reads, metadata: {},
          }], next_before: null,
        } });
        return;
      }
      const responses: Record<string, unknown> = {
        "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
        "/remote/api/agents": { agents: [agent] },
        "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
        "/remote/api/queue": { items: [] },
        "/remote/api/automations": { automations: [] },
        "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
      };
      await route.fulfill({ json: responses[endpoint] ?? {} });
    });
    await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
      socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
    });
    await page.goto("/remote", { waitUntil: "domcontentloaded" });
    await page.getByRole("button", { name: "Open Chat Fixture details" }).click();
    await expect(page.getByText("Earlier reply remains available.", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Refresh chat", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Chat history ownership could not be verified. Retry to load Chat again.");
    await expect(page.getByText("Earlier reply remains available.", { exact: true })).toBeVisible();
    await expect(page.getByText("No chat transcript yet.")).toHaveCount(0);
    await expect(page.getByText("Private diagnostic must stay hidden")).toHaveCount(0);
    const screenshotDir = process.env.WARDIAN_CHAT_RECOVERY_SCREENSHOT_DIR;
    if (screenshotDir) {
      fs.mkdirSync(screenshotDir, { recursive: true });
      await page.screenshot({ path: path.join(screenshotDir, `${viewport.name}-chat-local-error.png`), animations: "disabled" });
    }
    await page.getByRole("button", { name: "Retry Chat", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(page.getByText("History loaded after retry.", { exact: true })).toBeVisible();
    expect(reads).toBe(3);
  });
}

const boundedAgent = {
  session_id: "bounded-chat", session_name: "Bounded Chat", agent_class: "Coder",
  provider: "mock", workspace: "<agent-workspace>", status: "Off", latest_text: null,
};

const technicalText = [
  "Readable technical prose. ".repeat(100),
  "```typescript\nconst bounded = \"" + "technical_".repeat(60) + "\";\n```",
  "| Signal | Measurement |\n| --- | --- |\n| " + "wide-column-".repeat(35) + " | bounded |",
].join("\n\n");

function declaredHeader(index: number): AgentChatEvent {
  return {
    id: `bounded-${index}`, session_id: boundedAgent.session_id, provider: "mock",
    kind: "message", role: "assistant", text: index === 79 ? technicalText : `Declared header ${index}`,
    title: null, status: null, turn_id: `turn-${index}`, source: "watch", command: null,
    exit_code: null, path: null, language: null, created_at: "2026-01-01T00:00:00Z", sequence: index,
    metadata: index === 79 ? { chat_detail_ref: "owned-detail-reference", chat_body_binding: "body-79" } : {},
  };
}

/** Exercise the normal HTTP client with one fixed cohort; this is not provider proof. */
async function installBoundedChat(page: Page, retryableFirst = false, status: "Off" | "Processing" = "Off") {
  const agent = { ...boundedAgent, status };
  const traffic: { url: string; request_bytes: number; response_bytes: number; unchanged: boolean }[] = [];
  const statusTraffic = { connections: 0, client_frames: 0, client_bytes: 0, server_frames: 0, server_bytes: 0 };
  await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
  await page.route("**/remote/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.endsWith("/chat")) {
      const detail = url.searchParams.has("detail");
      const older = url.searchParams.has("before");
      if (detail) expect(url.searchParams.get("detail")).toBe("owned-detail-reference");
      if (older) expect(url.searchParams.get("before")).toBe("owned-older-frontier");
      const unchanged = !detail && !older && url.searchParams.get("revision") === "bounded-revision";
      const limit = Number(url.searchParams.get("limit") ?? "80");
      expect(Number.isInteger(limit) && limit >= 1 && limit <= 80).toBe(true);
      const dto: AgentChatPage = {
        session_id: boundedAgent.session_id, conversation_id: "bounded-conversation", generation: "bounded-generation",
        source_epoch: null, revision: "bounded-revision", unchanged, reset: false, progress: "ready",
        aliases: [], removed_ids: [],
        detail: detail ? { event_id: "bounded-79", text: "x".repeat(60_000), next: null, complete: true } : null,
        bytes_read: 0, records_decoded: 0,
        events: unchanged || detail ? [] : older
          ? Array.from({ length: 72 }, (_, index) => declaredHeader(index))
          : Array.from({ length: limit }, (_, index) => declaredHeader(80 - limit + index)),
        next_before: !older && !detail && limit < 80 ? "owned-older-frontier" : null,
        older_page: older,
      };
      const status = retryableFirst && traffic.length === 0 ? 503 : 200;
      const body = status === 503 ? JSON.stringify({ code: "agent_chat_read_retryable" }) : JSON.stringify(dto);
      traffic.push({ url: url.pathname + url.search,
        request_bytes: Buffer.byteLength(url.pathname + url.search) + Buffer.byteLength(request.postData() ?? ""),
        response_bytes: Buffer.byteLength(body), unchanged });
      await route.fulfill({ status, contentType: "application/json", body });
      return;
    }
    const responses: Record<string, unknown> = {
      "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
      "/remote/api/agents": { agents: [agent] },
      "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
      "/remote/api/queue": { items: [] },
      "/remote/api/automations": { automations: [] },
      "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
    };
    await route.fulfill({ json: responses[url.pathname] ?? {} });
  });
  await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
    statusTraffic.connections += 1;
    socket.onMessage((message) => {
      statusTraffic.client_frames += 1; statusTraffic.client_bytes += Buffer.byteLength(message);
      const response = JSON.stringify({ type: "agent_status", agents: [agent] });
      statusTraffic.server_frames += 1; statusTraffic.server_bytes += Buffer.byteLength(response);
      socket.send(response);
    });
  });
  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Open Bounded Chat details" }).click();
  return { chat: traffic, status_stream: statusTraffic };
}

test("healthy Off Chat automatically recovers a retryable read without a roster change", async ({ page }) => {
  const { chat: traffic } = await installBoundedChat(page, true);
  await expect(page.getByText("Declared header 78", { exact: true })).toBeVisible();
  expect(traffic.length).toBeGreaterThanOrEqual(2);
  expect(traffic[0].url).toContain("limit=8");
  await expect(page.getByRole("button", { name: "Retry Chat", exact: true })).toHaveCount(0);
});

test("technical Chat uses the pane width while prose and local overflow remain bounded", async ({ page }, testInfo) => {
  await installBoundedChat(page);
  await expect(page.locator(".chat-markdown").last()).toBeVisible();
  const geometry = [];
  for (const width of [320, 390, 600, 760, 768, 1440, 1920]) {
    await page.setViewportSize({ width, height: 1000 });
    const sample = await page.locator(".wardian-chat-pane").evaluate((pane) => {
      const rect = (selector: string) => {
        const element = pane.querySelector(selector);
        if (!(element instanceof HTMLElement)) throw new Error(`Missing ${selector}`);
        const box = element.getBoundingClientRect();
        return { left: box.left, width: box.width, scroll: element.scrollWidth, client: element.clientWidth };
      };
      const prose = pane.querySelector(".chat-prose-block");
      if (!(prose instanceof HTMLElement)) throw new Error("Missing prose");
      const technical = Array.from(pane.querySelectorAll(".chat-markdown")).at(-1);
      const code = technical?.querySelector("pre");
      const tableViewport = technical?.querySelector("table")?.parentElement;
      if (!(technical instanceof HTMLElement) || !(code instanceof HTMLElement) || !tableViewport) throw new Error("Missing technical content");
      return { pane: pane.getBoundingClientRect().width, lane: rect(".chat-transcript-list"),
        composer: rect(".chat-composer"), markdown: rect(".chat-markdown:last-child"),
        technical: { width: technical.getBoundingClientRect().width, code: code.getBoundingClientRect().width,
          table: tableViewport.getBoundingClientRect().width, tableOverflow: getComputedStyle(tableViewport).overflowX },
        prose: { width: prose.getBoundingClientRect().width, max: parseFloat(getComputedStyle(prose).maxWidth) },
        documentWidth: document.documentElement.scrollWidth, viewport: innerWidth };
    });
    geometry.push({ width, ...sample });
    expect(sample.documentWidth).toBeLessThanOrEqual(sample.viewport + 1);
    expect(sample.lane.width).toBeLessThanOrEqual(1152);
    expect(Math.abs(sample.lane.left - sample.composer.left)).toBeLessThanOrEqual(1);
    expect(Math.abs(sample.lane.width - sample.composer.width)).toBeLessThanOrEqual(1);
    expect(sample.prose.width).toBeLessThanOrEqual(sample.prose.max + 1);
    expect(sample.technical.tableOverflow).toBe("auto");
    if (width >= 1440) {
      expect(sample.lane.width).toBe(1152);
      expect(sample.technical.width).toBeGreaterThan(sample.prose.max + 100);
      expect(sample.technical.code).toBeGreaterThan(sample.prose.max);
      expect(sample.technical.table).toBeGreaterThan(sample.prose.max);
    }
  }
  await testInfo.attach("pane-geometry", { body: JSON.stringify(geometry, null, 2), contentType: "application/json" });
});

test("small recent Chat keeps Older and full body demand lazy and complete", async ({ page }) => {
  const { chat: traffic } = await installBoundedChat(page);
  await expect(page.getByText("Declared header 78", { exact: true })).toBeVisible();
  expect(traffic.some((request) => request.url.includes("detail=") || request.url.includes("before="))).toBe(false);
  await page.getByRole("button", { name: "Show full details", exact: true }).click();
  await expect(page.locator("pre").filter({ hasText: /^x+$/ })).toHaveText("x".repeat(60_000));
  expect(traffic.filter((request) => request.url.includes("detail=")).length).toBe(1);
  await page.locator(".chat-transcript-scroll").evaluate((element) => { element.scrollTop = 0; element.dispatchEvent(new Event("scroll")); });
  await expect(page.getByText("Declared header 0", { exact: true })).toBeVisible();
  const older = traffic.filter((request) => request.url.includes("before="));
  expect(older.length).toBe(1);
  expect(new URL(older[0].url, "http://fixture.test").searchParams.has("limit")).toBe(false);
  await expect(page.locator("[data-chat-row-key]")).toHaveCount(80);
});

test("phone Chat reserves its gutter and exposes a real touch-sized message menu", async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  try {
    const page = await context.newPage();
    await installBoundedChat(page);
    const trigger = page.locator(".chat-row-actions-menu-trigger").last();
    await expect(trigger).toBeVisible();
    const box = await trigger.boundingBox();
    expect(box?.width).toBeGreaterThanOrEqual(44);
    expect(box?.height).toBeGreaterThanOrEqual(44);
    expect(await page.locator(".chat-transcript-scroll").evaluate((element) => getComputedStyle(element).paddingLeft)).toBe("8px");
    await trigger.tap();
    await expect(page.getByRole("menuitem", { name: "Copy message", exact: true })).toBeVisible();
  } finally {
    await context.close();
  }
});

test("selected stable Off Chat bounds actual sixty-second HTTP polling traffic", async ({ page }, testInfo) => {
  test.setTimeout(90_000);
  const { chat: traffic, status_stream: statusTraffic } = await installBoundedChat(page);
  await expect(page.getByText("Declared header 78", { exact: true })).toBeVisible();
  await page.waitForTimeout(60_000);
  const baseline = process.env.WARDIAN_CHAT_TRAFFIC_BASELINE === "1";
  if (baseline) {
    expect(traffic.length).toBeGreaterThanOrEqual(20);
    expect(traffic.length).toBeLessThanOrEqual(22);
    expect(traffic.every((request) => !new URL(request.url, "http://fixture.test").searchParams.has("limit"))).toBe(true);
  } else {
    expect(traffic.length).toBeLessThanOrEqual(7);
    expect(traffic.every((request) => request.url.includes("limit=8"))).toBe(true);
  }
  expect(traffic.slice(1).every((request) => request.unchanged)).toBe(true);
  await testInfo.attach("chat-http-traffic", { body: JSON.stringify({ variant: baseline ? "before-source" : "current-source", elapsed_ms: 60_000, traffic, status_stream: statusTraffic,
    request_bytes: traffic.reduce((sum, request) => sum + request.request_bytes, 0),
    response_bytes: traffic.reduce((sum, request) => sum + request.response_bytes, 0),
    excludes: "HTTP headers, compression, TLS, provider acquisition and native UI" }, null, 2), contentType: "application/json" });
});

test("active Chat keeps its cadence for sixty seconds and stops reads after normal close", async ({ page }, testInfo) => {
  test.setTimeout(90_000);
  const { chat: traffic, status_stream: statusTraffic } = await installBoundedChat(page, false, "Processing");
  await expect(page.getByText("Declared header 78", { exact: true })).toBeVisible();
  await page.waitForTimeout(60_000);
  const selectedReads = traffic.length;
  expect(selectedReads).toBeGreaterThanOrEqual(20);
  expect(selectedReads).toBeLessThanOrEqual(22);
  expect(traffic.every((request) => request.url.includes("limit=8"))).toBe(true);
  expect(traffic.slice(1).every((request) => request.unchanged)).toBe(true);
  await page.getByRole("button", { name: "Back to remote agents", exact: true }).click();
  await expect(page.getByRole("button", { name: "Open Bounded Chat details" })).toBeVisible();
  const closedBaseline = traffic.length;
  await page.waitForTimeout(6_100);
  expect(traffic.length).toBe(closedBaseline);
  await testInfo.attach("active-chat-http-traffic", { body: JSON.stringify({ selected_interval_ms: 60_000,
    closed_interval_ms: 6_100, selected_reads: selectedReads, close_boundary_reads: closedBaseline - selectedReads,
    closed_reads: traffic.length - closedBaseline,
    request_bytes: traffic.reduce((sum, request) => sum + request.request_bytes, 0),
    response_bytes: traffic.reduce((sum, request) => sum + request.response_bytes, 0), status_stream: statusTraffic,
    excludes: "HTTP headers, compression, TLS, provider acquisition and native UI" }, null, 2), contentType: "application/json" });
});
