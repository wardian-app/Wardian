import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { installWorkbenchIpcMock } from "../fixtures/workbenchIpcMock";

const rootAgentId = "agent-seeded";
const seededAgentName = "Seeded Agent";
const screenshotStamp = process.env.WARDIAN_ACTIVE_ONLY_SCREENSHOT_STAMP
  ?? new Date().toISOString().replace(/[:.]/g, "-");
const screenshotDir = join(
  "e2e",
  "screenshots",
  "active-only-subagents",
  screenshotStamp,
);

type SummaryResponse = {
  summaries: [{
    root_agent_id: string;
    active: number;
    past: number;
    unknown: number;
    attention_count: number;
    attention_waiting: number;
    attention_failed: number;
    attention_unknown: number;
  }];
};

type ActiveOnlySummaryWindow = Window & {
  __WARDIAN_ACTIVE_ONLY_SUMMARY_RESPONSE__?: SummaryResponse;
  __TAURI_INTERNALS__?: {
    invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  };
};

function summaryResponse({
  active,
  past,
  unknown,
  attentionCount = 0,
  attentionWaiting = 0,
  attentionFailed = 0,
  attentionUnknown = 0,
}: {
  active: number;
  past: number;
  unknown: number;
  attentionCount?: number;
  attentionWaiting?: number;
  attentionFailed?: number;
  attentionUnknown?: number;
}): SummaryResponse {
  return {
    summaries: [{
      root_agent_id: rootAgentId,
      active,
      past,
      unknown,
      attention_count: attentionCount,
      attention_waiting: attentionWaiting,
      attention_failed: attentionFailed,
      attention_unknown: attentionUnknown,
    }],
  };
}

function worker(
  workerId: string,
  state: "requested" | "running" | "waiting" | "succeeded" | "failed" | "cancelled" | "unknown",
  outcome: string | null = null,
  parentWorkerId: string | null = null,
) {
  return {
    worker_id: workerId,
    kind: "provider_child",
    provider: "codex",
    workspace: "/workspace/seeded",
    root_agent_id: rootAgentId,
    parent_worker_id: parentWorkerId,
    parent_provider_session_id: "root-thread",
    runtime_session_id: "agent-seeded",
    provider_session_id: `thread-${workerId}`,
    runtime_generation: null,
    state,
    outcome,
    capabilities: {
      inspection: true,
      follow_up: false,
      interruption: false,
      resume: false,
      source: "codex child adapter is observe-only",
    },
    coverage: "codex_parent_thread_id_verified",
    source_path: `/rollouts/${workerId}.jsonl`,
    requested_at: "2026-09-17T00:00:00Z",
    last_observed_at: "2026-09-17T00:01:00Z",
  };
}

function seededAgent() {
  return [{
    session_id: rootAgentId,
    session_name: seededAgentName,
    agent_class: "Coder",
    folder: "/workspace/seeded",
    provider: "codex",
    is_off: false,
  }];
}

async function installSummaryOverride(
  page: Parameters<typeof installWorkbenchIpcMock>[0],
  initialResponse: SummaryResponse,
) {
  await page.addInitScript((response) => {
    const pageWindow = window as ActiveOnlySummaryWindow;
    pageWindow.__WARDIAN_ACTIVE_ONLY_SUMMARY_RESPONSE__ = response;
    const tauri = pageWindow.__TAURI_INTERNALS__;
    if (!tauri?.invoke) throw new Error("Tauri mock is not installed");
    const originalInvoke = tauri.invoke.bind(tauri);
    tauri.invoke = async (command, args) => {
      if (command === "temporary_worker_root_summaries") {
        return structuredClone(pageWindow.__WARDIAN_ACTIVE_ONLY_SUMMARY_RESPONSE__);
      }
      return originalInvoke(command, args);
    };
  }, initialResponse);
}

async function setSummaryResponse(
  page: Parameters<typeof installWorkbenchIpcMock>[0],
  response: SummaryResponse,
) {
  await page.evaluate((nextResponse) => {
    const pageWindow = window as ActiveOnlySummaryWindow;
    pageWindow.__WARDIAN_ACTIVE_ONLY_SUMMARY_RESPONSE__ = nextResponse;
  }, response);
}

test("shows only requested and active workers in a mixed roster", async ({ page }, testInfo) => {
  await installWorkbenchIpcMock(page, {
    agents: seededAgent(),
    responses: {
      temporary_worker_root_summaries: summaryResponse({
        active: 3,
        past: 2,
        unknown: 1,
        attentionCount: 1,
        attentionWaiting: 1,
      }),
      temporary_worker_root_details: {
        root_agent_id: rootAgentId,
        workers: [
          worker("active-requested", "requested"),
          worker("active-running", "running"),
          worker("active-waiting", "waiting", "waiting_for_follow_up", "finished-parent"),
          worker("finished-parent", "succeeded", "completed"),
          worker("finished-failed", "failed", "provider_failed"),
          worker("unavailable-record", "unknown", "provider_outcome_uncertain"),
        ],
        worker_telemetry: {},
      },
    },
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("app-shell").waitFor({ timeout: 15_000 });
  await openSurface(page, "agents-overview");

  const panel = surfacePanel(page, "agents-overview").first();
  const badge = panel.getByTestId(`agent-child-worker-indicator-${rootAgentId}`);
  await expect(badge).toContainText("3 active");
  await expect(badge).not.toContainText("past");
  await expect(badge).not.toContainText("unknown");
  await expect(badge).toHaveAttribute(
    "title",
    "Subagents for Seeded Agent: 3 active subagents. 1 subagent needs attention (1 waiting).",
  );

  const watchlistRow = page.locator(
    `[data-testid="agent-watchlist"] .watchlist-row[aria-label="Agent ${seededAgentName}"]`,
  );
  await expect(watchlistRow).toContainText(seededAgentName);
  const watchlistBadge = watchlistRow.getByTestId(
    `watchlist-child-worker-indicator-${rootAgentId}`,
  );
  await expect(watchlistBadge).toBeVisible();
  await expect(watchlistBadge).toContainText("3");
  await expect(watchlistBadge).not.toContainText("past");
  await expect(watchlistBadge).not.toContainText("unknown");
  await expect(watchlistBadge).not.toContainText("attention");
  const [rowBox, badgeBox] = await Promise.all([
    watchlistRow.boundingBox(),
    watchlistBadge.boundingBox(),
  ]);
  expect(rowBox).not.toBeNull();
  expect(badgeBox).not.toBeNull();
  expect(badgeBox!.x + badgeBox!.width).toBeLessThanOrEqual(rowBox!.x + rowBox!.width + 1);

  await badge.hover();
  await badge.click();
  const details = page.getByTestId(`agent-child-worker-details-${rootAgentId}`);
  await expect(details).toBeVisible();
  await expect(details.getByTestId(`agent-child-worker-summary-${rootAgentId}`)).toContainText("3 active subagents");
  await expect(details.getByTestId(`agent-child-worker-summary-${rootAgentId}`)).not.toContainText("past");
  await expect(details.getByTestId(`agent-child-worker-summary-${rootAgentId}`)).not.toContainText("unknown");
  await expect(details.getByTestId(`agent-child-worker-attention-${rootAgentId}`)).toContainText("1 subagent needs attention (1 waiting).");
  const current = details.getByTestId(`agent-child-worker-current-${rootAgentId}`);
  await expect(current).toContainText("Requested");
  await expect(current).toContainText("Running");
  await expect(current).toContainText("Waiting");
  await expect(details.getByTestId(`agent-child-worker-history-${rootAgentId}`)).toHaveCount(0);
  await expect(details.getByTestId(`agent-child-worker-unavailable-${rootAgentId}`)).toHaveCount(0);

  mkdirSync(screenshotDir, { recursive: true });
  const screenshotPath = join(screenshotDir, "mixed-active-workers.png");
  await page.screenshot({ path: screenshotPath, fullPage: true, animations: "disabled" });
  await testInfo.attach("mixed-active-workers", { path: screenshotPath, contentType: "image/png" });

  await badge.focus();
  await expect(badge).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(details).not.toBeVisible();
  await page.keyboard.press("Enter");
  await expect(details).toBeVisible();
});

test("hides the indicator when only past and unavailable records remain", async ({ page }, testInfo) => {
  await installWorkbenchIpcMock(page, {
    agents: seededAgent(),
    responses: {
      temporary_worker_root_summaries: summaryResponse({
        active: 0,
        past: 2,
        unknown: 1,
      }),
      temporary_worker_root_details: {
        root_agent_id: rootAgentId,
        workers: [
          worker("finished-parent", "succeeded", "completed"),
          worker("finished-failed", "failed", "provider_failed"),
          worker("unavailable-record", "unknown", "provider_outcome_uncertain"),
        ],
        worker_telemetry: {},
      },
    },
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("app-shell").waitFor({ timeout: 15_000 });
  await openSurface(page, "agents-overview");

  const panel = surfacePanel(page, "agents-overview").first();
  await expect(panel.getByTestId(`agent-child-worker-indicator-${rootAgentId}`)).toHaveCount(0);
  await expect(panel).toContainText(seededAgentName);

  const watchlistRow = page.locator(
    `[data-testid="agent-watchlist"] .watchlist-row[aria-label="Agent ${seededAgentName}"]`,
  );
  await expect(watchlistRow).toBeVisible();
  await expect(watchlistRow.getByTestId(`watchlist-child-worker-indicator-${rootAgentId}`)).toHaveCount(0);
  await expect(page.getByTestId(`agent-child-worker-details-${rootAgentId}`)).toHaveCount(0);

  mkdirSync(screenshotDir, { recursive: true });
  const screenshotPath = join(screenshotDir, "zero-active-workers-hidden.png");
  await page.screenshot({ path: screenshotPath, fullPage: true, animations: "disabled" });
  await testInfo.attach("zero-active-workers-hidden", { path: screenshotPath, contentType: "image/png" });
});

test("removes both mounted indicators after the next summary refresh reaches zero active workers", async ({ page }) => {
  const initialResponse = summaryResponse({ active: 2, past: 0, unknown: 0 });
  await installWorkbenchIpcMock(page, {
    agents: seededAgent(),
    responses: {
      temporary_worker_root_details: {
        root_agent_id: rootAgentId,
        workers: [
          worker("active-running", "running"),
          worker("active-waiting", "waiting", "waiting_for_follow_up"),
        ],
        worker_telemetry: {},
      },
    },
  });
  await installSummaryOverride(page, initialResponse);
  await page.clock.install();
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("app-shell").waitFor({ timeout: 15_000 });
  await openSurface(page, "agents-overview");

  const panel = surfacePanel(page, "agents-overview").first();
  const badge = panel.getByTestId(`agent-child-worker-indicator-${rootAgentId}`);
  await expect(badge).toContainText("2 active");
  const watchlistRow = page.locator(
    `[data-testid="agent-watchlist"] .watchlist-row[aria-label="Agent ${seededAgentName}"]`,
  );
  const watchlistBadge = watchlistRow.getByTestId(
    `watchlist-child-worker-indicator-${rootAgentId}`,
  );
  await expect(watchlistBadge).toBeVisible();

  await setSummaryResponse(page, summaryResponse({ active: 0, past: 1, unknown: 1 }));
  await page.clock.runFor(60_001);

  await expect(badge).toHaveCount(0);
  await expect(watchlistBadge).toHaveCount(0);
});
