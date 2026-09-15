import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { openSurface, surfacePanel } from "../fixtures/workbench";
import { installWorkbenchIpcMock } from "../fixtures/workbenchIpcMock";

const screenshotPath = join(
  "e2e",
  "screenshots",
  "subagent-badge",
  "2026-09-15",
  "subagents-inspector.png",
);

function worker(
  workerId: string,
  state: "running" | "waiting" | "succeeded" | "failed" | "cancelled" | "unknown",
  outcome: string | null = null,
) {
  return {
    worker_id: workerId,
    kind: "provider_child",
    provider: "codex",
    workspace: "/workspace/alpha",
    root_agent_id: "root-alpha",
    parent_worker_id: null,
    parent_provider_session_id: "root-thread",
    runtime_session_id: "agent-alpha",
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
    requested_at: "2026-09-14T00:00:00Z",
    last_observed_at: "2026-09-14T00:01:00Z",
  };
}

test("shows truthful subagent categories and attention reasons", async ({ page }) => {
  await installWorkbenchIpcMock(page, {
    agents: [{
      session_id: "agent-alpha",
      session_name: "Alpha",
      agent_class: "Coder",
      folder: "/workspace/alpha",
      provider: "codex",
      is_off: false,
    }],
    responses: {
      temporary_worker_root_summaries: {
        summaries: [{
          root_agent_id: "agent-alpha",
          active: 2,
          past: 3,
          unknown: 1,
          attention_count: 3,
          attention_waiting: 1,
          attention_failed: 1,
          attention_unknown: 1,
        }],
      },
      temporary_worker_root_details: {
        root_agent_id: "agent-alpha",
        workers: [
          worker("running", "running"),
          worker("waiting", "waiting", "waiting_for_follow_up"),
          worker("succeeded", "succeeded", "completed"),
          worker("failed", "failed", "provider_failed"),
          worker("cancelled", "cancelled", "cancelled_by_run"),
          worker("unknown", "unknown", "provider_outcome_uncertain"),
        ],
        worker_telemetry: {},
      },
    },
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("app-shell").waitFor({ timeout: 15_000 });
  await openSurface(page, "agents-overview");

  const panel = surfacePanel(page, "agents-overview").first();
  const badge = panel.getByTestId("agent-child-worker-indicator-agent-alpha");
  await expect(badge).toContainText("Subagents");
  await expect(badge).toContainText("2 active");
  await expect(badge).not.toContainText("3 past");
  await expect(badge).not.toContainText("1 unknown");
  await expect(badge).toHaveAttribute(
    "title",
    "Subagents for Alpha: 2 active subagents. 3 past subagents. 1 unknown subagent. 2 subagents need attention (1 waiting, 1 failed). Status unavailable for 1 subagent; the provider final status was not recorded.",
  );
  const watchlistRow = page.locator(
    '[data-testid="agent-watchlist"] .watchlist-row[aria-label="Agent Alpha"]',
  );
  await expect(watchlistRow).toContainText("Alpha");
  const watchlistBadge = watchlistRow.getByTestId(
    "watchlist-child-worker-indicator-agent-alpha",
  );
  await expect(watchlistBadge).toBeVisible();
  await expect(watchlistBadge).toContainText("2");
  await expect(watchlistBadge).not.toContainText("Subagents");
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
  const details = page.getByTestId("agent-child-worker-details-agent-alpha");
  await expect(details).toBeVisible();
  await expect(details.getByTestId("agent-child-worker-summary-agent-alpha")).toContainText("2 active subagents");
  await expect(details.getByTestId("agent-child-worker-attention-agent-alpha")).toContainText("2 subagents need attention (1 waiting, 1 failed). Status unavailable for 1 subagent; the provider final status was not recorded.");
  const current = details.getByTestId("agent-child-worker-current-agent-alpha");
  await expect(current).toContainText("Running");
  await expect(current).toContainText("Waiting");
  const history = details.getByTestId("agent-child-worker-history-agent-alpha");
  await expect(history).not.toHaveAttribute("open");
  const unavailable = details.getByTestId("agent-child-worker-unavailable-agent-alpha");
  await expect(unavailable).not.toHaveAttribute("open");

  mkdirSync(join("e2e", "screenshots", "subagent-badge", "2026-09-15"), { recursive: true });
  await page.screenshot({ path: screenshotPath, fullPage: true, animations: "disabled" });

  await history.locator("summary").click();
  await expect(history).toHaveAttribute("open");
  await expect(history).toContainText("Succeeded");
  await expect(history).toContainText("Failed");
});
