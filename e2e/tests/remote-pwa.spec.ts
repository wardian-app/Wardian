import fs from "node:fs";
import path from "node:path";
import { expect, test, type Locator, type WebSocketRoute } from "@playwright/test";

test.use({
  hasTouch: true,
  isMobile: true,
  userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
});

function remoteActionBody(body: unknown): {
  action?: string;
  target?: string;
  prompt?: string;
  input_mode?: string;
} {
  return typeof body === "object" && body !== null ? body : {};
}

type ChatRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

type ChatGeometry = {
  row: ChatRect;
  content: ChatRect;
  textRects: ChatRect[];
  clientWidth: number;
  scrollWidth: number;
};

async function chatGeometry(row: Locator): Promise<ChatGeometry> {
  return row.evaluate((element) => {
    const content = element.querySelector<HTMLElement>(".chat-message-content");
    if (!content) throw new Error("Message content is missing from the chat row");

    const rect = (value: DOMRect): ChatRect => ({
      x: value.x,
      y: value.y,
      width: value.width,
      height: value.height,
    });
    const textRects: ChatRect[] = [];
    const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node) {
      if (node.textContent?.trim()) {
        const range = document.createRange();
        range.selectNodeContents(node);
        for (const textRect of Array.from(range.getClientRects())) textRects.push(rect(textRect));
      }
      node = walker.nextNode();
    }

    return {
      row: rect(element.getBoundingClientRect()),
      content: rect(content.getBoundingClientRect()),
      textRects,
      clientWidth: content.clientWidth,
      scrollWidth: content.scrollWidth,
    };
  });
}

function expectChatGeometryUnchanged(before: ChatGeometry, after: ChatGeometry) {
  for (const key of ["x", "y", "width", "height"] as const) {
    expect(after.row[key]).toBeCloseTo(before.row[key], 1);
    expect(after.content[key]).toBeCloseTo(before.content[key], 1);
  }
  expect(after.clientWidth).toBe(before.clientWidth);
  expect(after.scrollWidth).toBe(before.scrollWidth);
}

async function expectChatTextUnobscured(geometry: ChatGeometry, action?: Locator) {
  expect(geometry.textRects.length).toBeGreaterThan(0);
  expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.clientWidth + 1);
  expect(geometry.textRects.every((textRect) => (
    textRect.x >= geometry.content.x - 1
    && textRect.y >= geometry.content.y - 1
    && textRect.x + textRect.width <= geometry.content.x + geometry.content.width + 1
    && textRect.y + textRect.height <= geometry.content.y + geometry.content.height + 1
  ))).toBe(true);

  if (action) {
    const actionBox = await action.boundingBox();
    expect(actionBox).not.toBeNull();
    if (!actionBox) throw new Error("Message action is not laid out");
    expect(geometry.textRects.some((textRect) => (
      textRect.x < actionBox.x + actionBox.width
      && textRect.x + textRect.width > actionBox.x
      && textRect.y < actionBox.y + actionBox.height
      && textRect.y + textRect.height > actionBox.y
    ))).toBe(false);
  }
}

test("remote mobile shell renders team-ordered watchlist and opens agent detail", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });

  const screenshotDir = process.env.WARDIAN_MOBILE_PWA_PARITY_SCREENSHOT_DIR;
  if (screenshotDir) fs.mkdirSync(screenshotDir, { recursive: true });
  const chatCopyLayoutScreenshotDir = process.env.WARDIAN_CHAT_COPY_LAYOUT_SCREENSHOT_DIR;
  if (chatCopyLayoutScreenshotDir) fs.mkdirSync(chatCopyLayoutScreenshotDir, { recursive: true });
  const automationScreenshotDir = process.env.WARDIAN_AUTOMATION_MONITOR_SCREENSHOT_DIR;
  if (automationScreenshotDir) fs.mkdirSync(automationScreenshotDir, { recursive: true });
  const captureFeatureScreenshot = async (name: string, locator: Locator) => {
    if (!screenshotDir) return;
    await locator.screenshot({ path: path.join(screenshotDir, name), animations: "disabled" });
  };
  const captureChatCopyLayoutScreenshot = async (name: string, locator: Locator) => {
    if (!chatCopyLayoutScreenshotDir) return;
    await locator.screenshot({ path: path.join(chatCopyLayoutScreenshotDir, name), animations: "disabled" });
  };

  const actionRequests: Array<{ headers: Record<string, string>; body: unknown }> = [];
  let statusStream: WebSocketRoute | null = null;
  let terminalStream: WebSocketRoute | null = null;
  const terminalInputs: string[] = [];
  const terminalControlRequests: unknown[] = [];
  const recoveryScrollback = Array.from(
    { length: 160 },
    (_, index) => `recovery scrollback line ${String(index + 1).padStart(3, "0")}`,
  );

  await page.route("**/remote/api/session", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        csrf_nonce: "csrf-e2e",
        expires_at: "2099-05-21T08:05:00.000Z",
        absolute_expires_at: "2099-05-21T20:00:00.000Z",
      }),
    });
  });
  await page.route("**/remote/api/agents", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        agents: [
          {
            session_id: "agent-2",
            session_name: "Remote Reviewer",
            agent_class: "Reviewer",
            provider: "claude",
            workspace: "<absolute-workspace-path>",
            status: "Processing",
            latest_text: null,
          },
          {
            session_id: "agent-1",
            session_name: "Remote Coder",
            agent_class: "Coder",
            provider: "opencode",
            workspace: "<absolute-workspace-path>",
            status: "Processing",
            latest_text: "Working",
          },
        ],
      }),
    });
  });
  await page.route("**/remote/api/watchlists", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        watchlists: [{ id: "main", name: "Main", entries: [{ type: "team", teamId: "team-1" }] }],
        teams: [{ id: "team-1", name: "Remote Team", agentIds: ["agent-2", "agent-1"] }],
        prefs: { columns: [], sort: null, preserve_team_grouping_when_sorted: false, collapsed_team_ids: [] },
      }),
    });
  });
  await page.route("**/remote/api/automations", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ automations: [] }) });
  });
  await page.route("**/remote/api/automations/monitor**", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        schema_version: 1,
        generated_at: "2026-08-31T12:00:00.000Z",
        active_runs: [
          {
            run_id: "approval-1",
            blueprint_id: "release-validation",
            automation_name: "Release validation",
            schedule_id: "schedule-release",
            status: "awaiting_approval",
            node_count: 4,
            completed_node_count: 3,
            failure: null,
            started_at: "2026-08-31T11:40:00.000Z",
            updated_at: "2026-08-31T11:55:00.000Z",
            completed_at: null,
          },
          {
            run_id: "running-1",
            blueprint_id: "daily-brief",
            automation_name: "Daily project brief",
            schedule_id: null,
            status: "running",
            node_count: 3,
            completed_node_count: 1,
            failure: null,
            started_at: "2026-08-31T11:50:00.000Z",
            updated_at: "2026-08-31T11:59:00.000Z",
            completed_at: null,
          },
        ],
        active_runs_truncated: false,
        active_runs_next_offset: null,
        recent_runs: [{
          run_id: "completed-1",
          blueprint_id: "dependency-refresh",
          automation_name: "Dependency refresh",
          schedule_id: null,
          status: "completed",
          node_count: 2,
          completed_node_count: 2,
          failure: null,
          started_at: "2026-08-31T10:00:00.000Z",
          updated_at: "2026-08-31T10:05:00.000Z",
          completed_at: "2026-08-31T10:05:00.000Z",
        }],
        recent_runs_truncated: false,
        recent_runs_next_offset: null,
        schedules: [{
          id: "schedule-release",
          blueprint_id: "release-validation",
          automation_name: "Morning status report",
          schedule: { schedule_type: "daily", time_of_day: "09:00", repeat_every: 1, end_condition: "never", occurrence_count: 0, active: true },
          next_run_epoch_ms: Date.parse("2026-09-01T13:00:00.000Z"),
          is_paused: false,
          last_run_status: "failed",
          last_run_error: "Last run failed. Open Wardian desktop for details.",
          last_run_epoch_ms: Date.parse("2026-08-31T13:00:00.000Z"),
          target_labels: ["writer · Agent"],
        }, {
          id: "schedule-paused",
          blueprint_id: "daily-brief",
          automation_name: "Paused brief",
          schedule: { schedule_type: "daily", time_of_day: "10:00", repeat_every: 1, end_condition: "never", occurrence_count: 0, active: true },
          next_run_epoch_ms: null,
          is_paused: true,
          last_run_status: "completed",
          last_run_error: null,
          last_run_epoch_ms: Date.parse("2026-08-31T12:01:00.000Z"),
          target_labels: ["writer · Agent"],
        }],
        schedules_truncated: false,
        schedules_next_offset: null,
      }),
    });
  });
  await page.route("**/remote/api/queue", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        items: [{
          id: "desktop-inbox-1",
          type: "agent_update",
          timestamp: 1779417600000,
          read: false,
          agent_session_id: "agent-1",
          notification_title: "Agent task completed",
          summary: [
            "Finished remote e2e update.",
            "",
            "This longer update verifies that the mobile inbox starts with the same compact preview as desktop.",
            "The complete message should remain available without making every card maximal by default.",
            "The inbox should also provide a direct path back to the agent that produced this update.",
          ].join("\n"),
        }, ...Array.from({ length: 119 }, (_, index) => ({
          id: `desktop-inbox-history-${index}`,
          type: "agent_completed",
          timestamp: 1779417500000 - index,
          read: false,
          agent_name: `Inbox history ${index}`,
          summary: `Completed remote Inbox task ${index}.`,
        }))],
      }),
    });
  });
  await page.route("**/remote/api/agents/agent-1/chat", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        events: [
          {
            id: "remote-user-message",
            session_id: "agent-1",
            provider: "opencode",
            kind: "message",
            role: "user",
            text: "Summarize the current implementation status.",
            title: null,
            status: null,
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: null,
            path: null,
            language: null,
            created_at: "2099-05-21T07:59:00.000Z",
            sequence: 1,
            metadata: {},
          },
          {
            id: "remote-agent-message",
            session_id: "agent-1",
            provider: "opencode",
            kind: "message",
            role: "assistant",
            text: "The navigation workbench is implemented and the focused verification is passing.",
            title: null,
            status: null,
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: null,
            path: null,
            language: null,
            created_at: "2099-05-21T08:00:00.000Z",
            sequence: 2,
            metadata: {},
          },
          {
            id: "remote-tool-call-1",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_call",
            role: null,
            text: "Inspecting the focused workbench surface.",
            title: "exec starting",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: "rg AgentChatView src/features/grid",
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:01.000Z",
            sequence: 3,
            metadata: {},
          },
          {
            id: "remote-lifecycle-only",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_call",
            role: null,
            text: null,
            title: "exec running",
            status: "running",
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: null,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:01.500Z",
            sequence: 4,
            metadata: { raw_type: "exec_running" },
          },
          {
            id: "remote-tool-result-1",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_result",
            role: null,
            text: "AgentChatView.tsx",
            title: "Output",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:02.000Z",
            sequence: 5,
            metadata: {},
          },
          {
            id: "remote-tool-call-2",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_call",
            role: null,
            text: "Running focused verification.",
            title: "Run tests",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: "npm run test -- AgentChatView.test.tsx",
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:03.000Z",
            sequence: 6,
            metadata: {},
          },
          {
            id: "remote-tool-result-2",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_result",
            role: null,
            text: "135 tests passed",
            title: "Output",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:04.000Z",
            sequence: 7,
            metadata: {},
          },
          {
            id: "remote-new-exec-call",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_call",
            role: null,
            text: null,
            title: "exec",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: "printf actual tool call",
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:05.000Z",
            sequence: 8,
            metadata: { raw_type: "exec" },
          },
          {
            id: "remote-new-exec-result",
            session_id: "agent-1",
            provider: "opencode",
            kind: "tool_result",
            role: null,
            text: "Script completed",
            title: "output",
            status: "succeeded",
            turn_id: "turn-1",
            source: "provider_log",
            command: null,
            exit_code: 0,
            path: null,
            language: "shell",
            created_at: "2099-05-21T08:00:06.000Z",
            sequence: 9,
            metadata: {},
          },
        ],
      }),
    });
  });
  await page.route("**/remote/api/ws-ticket", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        ticket: "ws-ticket-e2e",
        expires_at: "2099-05-21T08:05:00.000Z",
      }),
    });
  });
  await page.routeWebSocket("**/remote/api/status-stream", async (ws) => {
    statusStream = ws;
    ws.onMessage(() => {});
  });
  await page.routeWebSocket("**/remote/api/agents/agent-1/terminal-stream", async (ws) => {
    terminalStream = ws;
    let seeded = false;
    let recoverySnapshotSent = false;
    let liveEventSent = false;
    ws.onMessage((message) => {
      const payload = JSON.parse(String(message));
      if (payload.type === "input") terminalInputs.push(String(payload.data ?? ""));
      if (payload.type === "begin_activation") terminalControlRequests.push(payload);
      if (!seeded) {
        expect(payload).toMatchObject({
          protocol_version: 2,
          ticket: "ws-ticket-e2e",
          cols: expect.any(Number),
          rows: expect.any(Number),
        });
        seeded = true;
        ws.send(
          JSON.stringify({
            type: "registered",
            protocol_version: 2,
            presentation: {
              presentation_id: "remote-e2e",
              client_kind: "remote",
              desired_geometry: { cols: 80, rows: 24 },
              visibility: "visible",
              render_state: "mounted",
              interaction_capability: "interactive",
              interaction_sequence: 1,
              requires_resync: false,
            },
            broker_state: {
              session_id: "agent-1",
              runtime_generation: 1,
              lease_epoch: 1,
              stream_sequence: 1,
              interaction_sequence: 1,
              geometry: { cols: 80, rows: 24 },
              owner_presentation_id: "desktop-e2e",
              pending_activation: null,
              runtime_state: "live",
            },
            initial_snapshot: {
              snapshot_id: "snapshot-e2e",
              session_id: "agent-1",
              runtime_generation: 1,
              sequence_barrier: 0,
              geometry: { cols: 80, rows: 24 },
              terminal_state_base64: Buffer.from("terminal ready from e2e", "utf8").toString("base64"),
              visible_grid: "terminal ready from e2e",
              scrollback: [],
            },
          }),
        );
        return;
      }
      if (payload.type === "request_events" && !recoverySnapshotSent) {
        recoverySnapshotSent = true;
        ws.send(JSON.stringify({
          type: "events",
          batch: {
            status: "gap",
            runtime_generation: 1,
            events: [],
            next_sequence: 10,
            available_from_sequence: 10,
            latest_sequence: 10,
            recovery_snapshot: {
              snapshot_id: "recovery-snapshot-e2e",
              session_id: "agent-1",
              runtime_generation: 1,
              sequence_barrier: 10,
              geometry: { cols: 80, rows: 24 },
              terminal_state_base64: Buffer.from("recovered current viewport", "utf8").toString("base64"),
              visible_grid: "recovered current viewport",
              scrollback: recoveryScrollback,
            },
          },
        }));
      } else if (payload.type === "request_events" && !liveEventSent) {
        liveEventSent = true;
        ws.send(JSON.stringify({
          type: "events",
          batch: {
            status: "events",
            runtime_generation: 1,
            events: [{
              type: "output",
              sequence: 11,
              runtime_generation: 1,
              bytes_base64: Buffer.from("Finished remote e2e update.", "utf8").toString("base64"),
            }],
            next_sequence: 11,
            available_from_sequence: 11,
            latest_sequence: 11,
            recovery_snapshot: null,
          },
        }));
      } else if (payload.type === "begin_activation") {
        ws.send(JSON.stringify({
          type: "activation_begin",
          result: {
            decision: {
              status: "accepted",
              reason: null,
              runtime_generation: 1,
              lease_epoch: 2,
              owner_presentation_id: "desktop-e2e",
            },
            activation_id: "activation-e2e",
            snapshot: {
              snapshot_id: "activation-snapshot-e2e",
              session_id: "agent-1",
              runtime_generation: 1,
              sequence_barrier: 1,
              geometry: { cols: 80, rows: 24 },
              terminal_state_base64: Buffer.from("terminal ready from e2e", "utf8").toString("base64"),
              visible_grid: "terminal ready from e2e",
              scrollback: [],
            },
            sequence_barrier: 1,
          },
        }));
      } else if (payload.type === "ack_activation") {
        ws.send(JSON.stringify({
          type: "activation_ack",
          result: {
            decision: {
              status: "accepted",
              reason: null,
              runtime_generation: 1,
              lease_epoch: 2,
              owner_presentation_id: "remote-e2e",
            },
            broker_state: {
              session_id: "agent-1",
              runtime_generation: 1,
              lease_epoch: 2,
              stream_sequence: 1,
              interaction_sequence: 2,
              geometry: { cols: 80, rows: 24 },
              owner_presentation_id: "remote-e2e",
              pending_activation: null,
              runtime_state: "live",
            },
            snapshot: {
              snapshot_id: "activation-ack-snapshot-e2e",
              session_id: "agent-1",
              runtime_generation: 1,
              sequence_barrier: 11,
              geometry: { cols: 80, rows: 24 },
              terminal_state_base64: Buffer.from("Finished remote e2e update.", "utf8").toString("base64"),
              visible_grid: "Finished remote e2e update.",
              scrollback: recoveryScrollback,
            },
          },
        }));
      }
    });
  });
  await page.route("**/remote/api/agents/action", async (route) => {
    actionRequests.push({
      headers: route.request().headers(),
      body: JSON.parse(route.request().postData() ?? "{}"),
    });
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ ok: true }) });
  });

  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await expect(page.locator('[data-testid="remote-mobile-app"]')).toBeVisible();
  await expect(page.getByText("Remote Coder")).toBeVisible();
  await expect(page.locator('[data-testid="remote-watchlist-view"]')).toBeVisible();
  await expect(page.getByText("Remote Team")).toBeVisible();
  const rowNames = await page.locator('[data-testid="remote-watchlist-agent-row"]').allTextContents();
  expect(rowNames).toEqual([
    expect.stringContaining("Remote Reviewer"),
    expect.stringContaining("Remote Coder"),
  ]);
  await expect(page.getByRole("navigation", { name: "Remote sections" })).toBeVisible();

  const remoteNavigation = page.getByRole("navigation", { name: "Remote sections" });
  await remoteNavigation.locator('[data-remote-tab="automations"]').click();
  const automationMonitor = page.getByTestId("remote-automations-view");
  await expect(automationMonitor.getByText("Needs attention")).toBeVisible();
  await expect(automationMonitor.getByText("Running now")).toBeVisible();
  await expect(automationMonitor.getByText("Up next")).toBeVisible();
  await expect(automationMonitor.getByRole("heading", { name: "Paused", exact: true })).toBeVisible();
  await expect(automationMonitor.getByText("Recent outcomes")).toBeVisible();
  await expect(automationMonitor.getByTestId("remote-schedule-card-schedule-release-attention")).toHaveAttribute("data-status-tone", "attention");
  await expect(automationMonitor.getByTestId("remote-schedule-card-schedule-release-pending")).toHaveAttribute("data-status-tone", "pending");
  await expect(automationMonitor.getByTestId("remote-schedule-card-schedule-paused-paused")).toHaveAttribute("data-status-tone", "paused");
  await expect(automationMonitor.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-pressed", "true");
  if (automationScreenshotDir) {
    await automationMonitor.screenshot({
      path: path.join(automationScreenshotDir, "automation-overview.png"),
      animations: "disabled",
    });
  }
  await automationMonitor.getByRole("button", { name: /Release validation/ }).click();
  await expect(page.getByRole("dialog", { name: "Release validation" })).toBeVisible();
  await page.getByRole("button", { name: "Close automation details" }).click();
  await remoteNavigation.locator('[data-remote-tab="watchlist"]').click();
  await expect(page.locator('[data-testid="remote-watchlist-view"]')).toBeVisible();

  await page.getByRole("button", { name: "Open remote settings" }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await page.getByLabel("Theme").selectOption("dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await captureFeatureScreenshot("settings-view.png", page.locator("main"));
  await page.getByRole("button", { name: "Back to remote watchlist" }).click();
  await expect(page.locator('[data-testid="remote-watchlist-view"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Open broadcast prompt" })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Broadcast prompt" })).toHaveCount(0);
  await captureFeatureScreenshot("watchlist-no-broadcast.png", page.locator('[data-testid="remote-watchlist-view"]'));

  await page.getByRole("button", { name: "Open Remote Coder details" }).click();
  await expect(page.locator('[data-testid="remote-agent-detail"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Terminal", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => terminalControlRequests.length).toBe(1);
  await expect(page.getByText("Finished remote e2e update.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Take terminal control" })).toHaveCount(0);
  await captureFeatureScreenshot("lifecycle-actions.png", page.locator('[data-testid="remote-agent-detail"]'));
  await captureFeatureScreenshot("terminal-detail.png", page.locator('[data-testid="remote-agent-detail"]'));
  const terminalScrollThumb = page.locator(
    '[data-testid="remote-terminal-scroll-surface"] .scrollbar.vertical .slider',
  );
  const scrollTopBefore = await terminalScrollThumb.evaluate(
    (element) => Number.parseFloat((element as HTMLElement).style.top),
  );
  expect(scrollTopBefore).toBeGreaterThan(0);
  const scrollSurface = page.locator('[data-testid="remote-terminal-scroll-surface"]');
  const scrollSurfaceBox = await scrollSurface.boundingBox();
  expect(scrollSurfaceBox).not.toBeNull();
  if (!scrollSurfaceBox) throw new Error("remote terminal scroll surface is not visible");
  const cdp = await page.context().newCDPSession(page);
  const touchX = scrollSurfaceBox.x + scrollSurfaceBox.width / 2;
  const touchY = scrollSurfaceBox.y + scrollSurfaceBox.height * 0.3;
  const touchPoint = (y: number) => ({ id: 1, x: touchX, y, radiusX: 1, radiusY: 1, force: 1 });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [touchPoint(touchY)] });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [touchPoint(touchY + 180)] });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await expect.poll(async () => terminalScrollThumb.evaluate(
    (element) => Number.parseFloat((element as HTMLElement).style.top),
  )).toBeLessThan(scrollTopBefore);
  expect(terminalControlRequests).toHaveLength(1);
  await captureFeatureScreenshot("terminal-recovery-scrollback.png", page.locator('[data-testid="remote-agent-detail"]'));

  await expect.poll(() => terminalStream !== null).toBe(true);
  terminalStream?.send(JSON.stringify({
    type: "events",
    batch: {
      status: "events",
      runtime_generation: 1,
      events: [{
        type: "output",
        sequence: 12,
        runtime_generation: 1,
        bytes_base64: Buffer.from(
          "\u001b[?1049h\u001b[?1000h\u001b[?1006hAlternate terminal scroll target",
          "utf8",
        ).toString("base64"),
      }],
      next_sequence: 12,
      available_from_sequence: 12,
      latest_sequence: 12,
      recovery_snapshot: null,
    },
  }));
  await expect(page.getByText("Alternate terminal scroll target")).toBeVisible();
  const terminalInputCountBeforeTouch = terminalInputs.length;
  await page.locator('[data-testid="remote-terminal-scroll-surface"]').dispatchEvent("touchstart", {
    touches: [{ identifier: 0, clientX: 180, clientY: 420 }],
  });
  await page.locator('[data-testid="remote-terminal-scroll-surface"]').dispatchEvent("touchmove", {
    touches: [{ identifier: 0, clientX: 180, clientY: 360 }],
  });
  await expect.poll(() => terminalInputs.length).toBeGreaterThan(terminalInputCountBeforeTouch);
  expect(terminalInputs.at(-1)).toMatch(/^\u001b\[<6[45];/);

  await expect.poll(() => statusStream !== null).toBe(true);
  statusStream?.send(
    JSON.stringify({
      type: "agent_status",
      agents: [
        {
          session_id: "agent-2",
          session_name: "Remote Reviewer",
          agent_class: "Reviewer",
          provider: "claude",
          workspace: "<absolute-workspace-path>",
          status: "Processing",
          latest_text: null,
        },
        {
          session_id: "agent-1",
          session_name: "Remote Coder",
          agent_class: "Coder",
          provider: "opencode",
          workspace: "<absolute-workspace-path>",
          status: "Idle",
          latest_text: "Ready",
        },
      ],
    }),
  );
  await page.getByRole("button", { name: "Chat", exact: true }).click();
  await expect(page.getByText("exec starting", { exact: true })).toHaveCount(0);
  await expect(page.getByText("exec running", { exact: true })).toHaveCount(0);
  await expect(page.getByText("exec", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Script completed", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("user message")).toHaveClass(/\bitems-end\b/);
  await expect(page.getByLabel("assistant message")).toHaveClass(/\bw-full\b/);
  await expect(page.getByTestId("chat-work-group")).toHaveAttribute("data-expanded", "false");
  await page.setViewportSize({ width: 1020, height: 844 });
  const transcript = page.locator(".chat-transcript-list");
  const transcriptBox = await transcript.boundingBox();
  const transcriptWidthMetrics = await transcript.evaluate((element) => {
    const probe = document.createElement("span");
    probe.style.cssText = "position:absolute; width:1ch; height:0; overflow:hidden;";
    element.append(probe);
    const chWidth = probe.getBoundingClientRect().width;
    probe.remove();
    return { chWidth, maxWidth: Number.parseFloat(getComputedStyle(element).maxWidth) };
  });
  expect(transcriptWidthMetrics.maxWidth).toBeCloseTo(transcriptWidthMetrics.chWidth * 76, 0);
  const rowBoxes = await transcript.locator(":scope > .chat-row").evaluateAll((elements) =>
    elements.map((element) => {
      const { width, x } = element.getBoundingClientRect();
      return { width, x };
    }),
  );
  expect(transcriptBox).not.toBeNull();
  expect(rowBoxes.length).toBeGreaterThan(1);
  for (const rowBox of rowBoxes) {
    expect(rowBox.width).toBeCloseTo(transcriptBox!.width, 0);
    expect(rowBox.x).toBeCloseTo(transcriptBox!.x, 0);
  }
  await captureFeatureScreenshot("chat-consistent-width.png", page.locator('[data-testid="remote-agent-detail"]'));
  await page.setViewportSize({ width: 390, height: 844 });
  const narrowTranscript = await transcript.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  expect(narrowTranscript.scrollWidth).toBeLessThanOrEqual(narrowTranscript.clientWidth);
  const workGroup = page.getByTestId("chat-work-group");
  const workActions = workGroup.locator(".chat-row-actions--rail");
  const workToggle = workGroup.getByRole("button", { name: "Show all" });
  await expect(workActions).toBeVisible();
  await expect(workToggle).toBeVisible();
  const [workActionsBox, workToggleBox] = await Promise.all([workActions.boundingBox(), workToggle.boundingBox()]);
  expect(workActionsBox).not.toBeNull();
  expect(workToggleBox).not.toBeNull();
  expect(workActionsBox!.y + workActionsBox!.height / 2).toBeCloseTo(workToggleBox!.y + workToggleBox!.height / 2, 0);
  expect(workActionsBox!.x + workActionsBox!.width).toBeLessThanOrEqual(workToggleBox!.x);
  await captureFeatureScreenshot("chat-collapsed-work.png", page.locator('[data-testid="remote-agent-detail"]'));
  const messageRows = [
    page.getByLabel("user message"),
    page.getByLabel("assistant message"),
  ];
  for (const row of messageRows) {
    const beforeActions = await chatGeometry(row);
    const action = row.getByRole("button", { name: "Message actions", exact: true });
    await expect(action).toBeVisible();
    await expect(action).toHaveAttribute("aria-haspopup", "menu");
    const actionBox = await action.boundingBox();
    expect(actionBox).not.toBeNull();
    if (!actionBox) throw new Error("Message action hit area is missing");
    expect(actionBox.width).toBeGreaterThanOrEqual(44);
    expect(actionBox.height).toBeGreaterThanOrEqual(44);
    await expectChatTextUnobscured(beforeActions, action);

    await action.click();
    const menu = row.getByRole("menu");
    await expect(menu).toBeVisible();
    const menuBox = await menu.boundingBox();
    expect(menuBox).not.toBeNull();
    if (!menuBox) throw new Error("Message actions menu is missing");
    const viewport = page.viewportSize();
    expect(viewport).not.toBeNull();
    if (!viewport) throw new Error("Viewport size is unavailable");
    expect(menuBox.x).toBeGreaterThanOrEqual(0);
    expect(menuBox.x + menuBox.width).toBeLessThanOrEqual(viewport.width);
    expect(menuBox.y).toBeGreaterThanOrEqual(0);
    expect(menuBox.y + menuBox.height).toBeLessThanOrEqual(viewport.height);
    expectChatGeometryUnchanged(beforeActions, await chatGeometry(row));

    const copyItem = row.getByRole("menuitem", { name: "Copy message" });
    await expect(copyItem).toBeVisible();
    if (row === messageRows[1]) {
      await captureFeatureScreenshot("chat-message-actions-menu.png", page.locator('[data-testid="remote-agent-detail"]'));
      await captureChatCopyLayoutScreenshot("mobile-copy-layout.png", page.locator('[data-testid="remote-agent-detail"]'));
    }
    await page.keyboard.press("Escape");
  }
  await page.getByRole("button", { name: "Show all" }).click();
  await expect(page.getByTestId("chat-work-group")).toHaveAttribute("data-expanded", "true");
  await expect(page.getByText("rg AgentChatView src/features/grid", { exact: true })).toBeVisible();
  await expect(page.getByText("printf actual tool call", { exact: true })).toBeVisible();
  const selectableToolCall = page.getByText("printf actual tool call", { exact: true });
  await selectableToolCall.evaluate((element) => {
    const selection = window.getSelection();
    if (!selection) throw new Error("Text selection is unavailable");
    const range = document.createRange();
    range.selectNodeContents(element);
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await expect.poll(() => page.evaluate(() => window.getSelection()?.toString() ?? "")).toBe("printf actual tool call");
  await captureFeatureScreenshot("chat-text-selection.png", page.locator('[data-testid="remote-agent-detail"]'));
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  await captureFeatureScreenshot("chat-expanded-work.png", page.locator('[data-testid="remote-agent-detail"]'));
  await page.getByRole("button", { name: "Collapse" }).click();
  await captureFeatureScreenshot("chat-full-width.png", page.locator('[data-testid="remote-agent-detail"]'));
  await page.getByLabel("Prompt Remote Coder").fill("status please");
  await page.getByRole("button", { name: "Send prompt" }).click();

  await expect
    .poll(() => actionRequests.filter(({ body }) => remoteActionBody(body).prompt === "status please").length)
    .toBe(1);
  const chatPromptRequest = actionRequests.find(({ body }) => remoteActionBody(body).prompt === "status please");
  expect(chatPromptRequest).toMatchObject({
    headers: {
      "x-wardian-csrf": "csrf-e2e",
    },
    body: {
      action: "send_prompt",
      target: "agent-1",
      prompt: "status please",
    },
  });

  await page.getByRole("button", { name: "Back to remote agents" }).click();
  await page.getByRole("button", { name: "Inbox" }).click();
  await expect(page.getByText("Important update")).toBeVisible();
  await expect(page.getByText("Finished remote e2e update.")).toBeVisible();
  const inboxSummary = page.getByTestId("remote-queue-item-summary-desktop-inbox-1");
  const summaryToggle = page.getByRole("button", { name: "Show full summary" });
  await expect(summaryToggle).toHaveAttribute("aria-expanded", "false");
  await summaryToggle.click();
  await expect(page.getByRole("button", { name: "Collapse summary" })).toHaveAttribute("aria-expanded", "true");
  await expect(inboxSummary).toHaveClass(/max-h-80/);
  await captureFeatureScreenshot("inbox-summary.png", page.locator("main"));
  await expect(page.getByText("Inbox history 100", { exact: true })).toBeHidden();
  const inboxScrollRegion = page.getByTestId("remote-inbox-scroll-region");
  await inboxScrollRegion.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await expect(page.getByText("Inbox history 118", { exact: true })).toBeVisible();
  await inboxScrollRegion.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await captureFeatureScreenshot("inbox-lazy-history.png", page.locator("main"));
  await page.getByRole("button", { name: "Open agent terminal" }).click();
  await expect(page.locator('[data-testid="remote-agent-detail"]')).toBeVisible();
});
