import { expect, test, type Locator } from "@playwright/test";
import { mkdirSync } from "node:fs";
import path from "node:path";

import { surfacePanel, surfaceTab } from "../fixtures/workbench";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";

const agents: WorkbenchAgentFixture[] = [
  {
    session_id: "agent-alpha",
    session_name: "Alpha",
    agent_class: "Coder",
    folder: "/workspace/alpha",
    provider: "mock",
    is_off: false,
  },
  {
    session_id: "agent-beta",
    session_name: "Beta",
    agent_class: "Reviewer",
    folder: "/workspace/beta",
    provider: "mock",
    is_off: false,
  },
];

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

async function expectChatTextUnobscured(row: Locator, geometry: ChatGeometry, action?: Locator) {
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
    if (!actionBox) throw new Error("Copy action is not laid out");
    const actionRect: ChatRect = actionBox;
    expect(geometry.textRects.some((textRect) => (
      textRect.x < actionRect.x + actionRect.width
      && textRect.x + textRect.width > actionRect.x
      && textRect.y < actionRect.y + actionRect.height
      && textRect.y + textRect.height > actionRect.y
    ))).toBe(false);
  }
  await expect(row).toBeVisible();
}

test("renders a capture-ready tabs-and-splits workbench", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("overview-evidence", "agents-overview", {
    state: {
      mode: "grid",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    },
  });
  const queue = makeWorkbenchSurface("inbox-evidence", "inbox");
  const document = makeWorkbenchDocument({
    revision: 2,
    root: {
      kind: "split",
      node_id: "evidence-split",
      direction: "horizontal",
      ratio: 0.7,
      first: { kind: "group", group_id: "group-overview" },
      second: { kind: "group", group_id: "group-queue" },
    },
    groups: {
      "group-overview": {
        group_id: "group-overview",
        surface_ids: [overview.surface_id],
        active_surface_id: overview.surface_id,
      },
      "group-queue": {
        group_id: "group-queue",
        surface_ids: [queue.surface_id],
        active_surface_id: queue.surface_id,
      },
    },
    surfaces: [overview, queue],
    active_group_id: "group-overview",
  });

  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents,
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "evidence-token-2",
    },
  });

  await page.goto("/");
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  await expect(surfaceTab(page, "agents-overview")).toBeVisible();
  await expect(surfaceTab(page, "inbox")).toBeVisible();
  await expect(surfacePanel(page, "agents-overview")).toBeVisible();
  await expect(surfacePanel(page, "inbox")).toBeVisible();
  await expect(page.getByTestId("sidebar-icon-rail")).toBeVisible();
  await expect(page.getByTestId("agent-watchlist")).toBeVisible();
  await expect(page.locator('[data-testid="agent-card"]:visible')).toHaveCount(2);
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  await page.waitForTimeout(500);

  const path = process.env.WARDIAN_WORKBENCH_SCREENSHOT
    ?? testInfo.outputPath("tabs-and-splits.png");
  await page.screenshot({ path, animations: "disabled" });
  await testInfo.attach("tabs-and-splits", { path, contentType: "image/png" });
});

test("renders chat attachment chips while hiding the provider launch screen", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("chat-attachment-evidence", "agents-overview", {
    state: {
      mode: "single",
      focused_agent_id: "agent-alpha",
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 3, surfaces: [overview] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents: [{
      session_id: "agent-alpha",
      session_name: "Alpha",
      agent_class: "Coder",
      folder: "/workspace/alpha",
      provider: "codex",
      is_off: false,
      model: "gpt-5.6-sol",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "chat-attachment-evidence-token-3",
    },
    responses: {
      load_agent_chat_transcript: [
        {
          id: "codex-launch",
          session_id: "agent-alpha",
          provider: "codex",
          kind: "terminal_output",
          role: "system",
          text: "OpenAI Codex\nReady for your task\n/workspace/alpha",
          title: "Codex started",
          status: null,
          turn_id: null,
          source: null,
          command: null,
          exit_code: null,
          path: null,
          language: null,
          created_at: null,
          sequence: 1,
          metadata: { terminal_presentation: "launch" },
        },
        {
          id: "assistant-kickoff",
          session_id: "agent-alpha",
          provider: "codex",
          kind: "message",
          role: "assistant",
          text: "Working through the requested refactor now.",
          title: null,
          status: null,
          turn_id: null,
          source: null,
          command: null,
          exit_code: null,
          path: null,
          language: null,
          created_at: null,
          sequence: 2,
          metadata: {},
        },
      ],
      "plugin:dialog|open": ["C:/evidence/dashboard.png", "C:/evidence/notes.txt"],
      list_provider_model_catalog: {
        provider: "codex",
        version: "codex-cli 0.146.0",
        source: "live_catalog",
        refresh_error: null,
        models: [{
          id: "gpt-5.6-sol",
          display_name: "5.6 Terra",
          effort_options: ["low", "high"],
          default_effort: "high",
          is_default: true,
        }],
      },
    },
  });

  await page.goto("/");
  await expect(page.getByTestId("agent-card")).toBeVisible();
  await expect(page.getByText("Codex started", { exact: true })).toBeHidden();
  await expect(page.getByTestId("terminal-fallback-row")).toHaveCount(0);
  await expect(page.getByText("Working through the requested refactor now.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Attach files" }).click();
  await expect(page.getByText("dashboard.png", { exact: true })).toBeVisible();
  await expect(page.getByText("notes.txt", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Send message" })).toBeEnabled();

  const path = process.env.WARDIAN_CHAT_ATTACHMENTS_SCREENSHOT
    ?? testInfo.outputPath("chat-attachments.png");
  await page.locator('[data-testid="agent-card"]').screenshot({ path, animations: "disabled" });
  await testInfo.attach("chat-attachments", { path, contentType: "image/png" });
});

test("shows the actual command for a lifecycle-labelled tool call", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("chat-tool-call-one-line", "agents-overview", {
    state: {
      mode: "single",
      focused_agent_id: "agent-alpha",
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 7, surfaces: [overview] });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents: [{
      ...agents[0],
      provider: "codex",
      model: "gpt-5.6-sol",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "chat-tool-call-one-line-token-7",
    },
    responses: {
      load_agent_chat_transcript: [{
        id: "exec-call-1",
        session_id: "agent-alpha",
        provider: "codex",
        kind: "tool_call",
        role: null,
        text: null,
        title: "exec_command_begin",
        status: "running",
        turn_id: "turn-1",
        source: "provider_log",
        command: "npm test -- chat",
        exit_code: null,
        path: null,
        language: "shell",
        created_at: "2026-08-24T06:00:00.000Z",
        sequence: 1,
        metadata: { raw_type: "exec_command_begin" },
      }],
      list_provider_model_catalog: {
        provider: "codex",
        version: "codex-cli 0.149.1",
        source: "live_catalog",
        refresh_error: null,
        models: [{
          id: "gpt-5.6-sol",
          display_name: "GPT-5.6-Sol",
          effort_options: ["low", "high"],
          default_effort: "high",
          is_default: true,
        }],
      },
    },
  });

  await page.goto("/", { waitUntil: "domcontentloaded" });
  const card = page.getByTestId("agent-card");
  const summary = card.getByTestId("chat-tool-call-summary");
  await expect(summary).toBeVisible();
  await expect(summary).toContainText("$ npm test -- chat");
  await expect(summary).not.toContainText("exec command begin");

  const screenshotPath = path.resolve(
    "e2e/screenshots/chat-tool-call-one-line/2026-08-24/mobile-lifecycle-command.png",
  );
  mkdirSync(path.dirname(screenshotPath), { recursive: true });
  await card.screenshot({ path: screenshotPath, animations: "disabled" });
  await testInfo.attach("mobile-lifecycle-command", { path: screenshotPath, contentType: "image/png" });
});

test("keeps the offline agent composer send button consistent across empty, populated, and executing states", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("composer-send-button-evidence", "agents-overview", {
    state: {
      mode: "single",
      focused_agent_id: "agent-alpha",
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 5, surfaces: [overview] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents: [{
      ...agents[0],
      is_off: false,
      provider: "codex",
      model: "gpt-5.6-sol",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "composer-send-button-evidence-token-5",
    },
    responses: {
      list_provider_model_catalog: {
        provider: "codex",
        version: "codex-cli 0.146.0",
        source: "live_catalog",
        refresh_error: null,
        models: [{
          id: "gpt-5.6-sol",
          display_name: "5.6 Terra",
          effort_options: ["low", "high"],
          default_effort: "high",
          is_default: true,
        }],
      },
    },
  });

  await page.goto("/");
  const card = page.locator('[data-testid="agent-card"]');
  const input = page.getByLabel("Message agent");
  const sendButton = page.getByRole("button", { name: "Send message" });
  await expect(card).toBeVisible();
  await page.evaluate(() => {
    const runtime = (window as unknown as {
      __WARDIAN_WORKBENCH_IPC_MOCK__: { emit: (event: string, payload: unknown) => void };
    }).__WARDIAN_WORKBENCH_IPC_MOCK__;
    runtime.emit("agent-status-updated", {
      session_id: "agent-alpha",
      current_status: "Off",
    });
  });
  await expect(input).toBeEnabled();
  await expect(page.getByText("Provider returned an invalid model catalogue.", { exact: true })).toBeHidden();
  await expect(sendButton).toBeDisabled();
  await expect(sendButton).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(sendButton).toHaveCSS("color", "rgb(75, 85, 99)");
  await expect(sendButton).toHaveCSS("opacity", "0.5");

  const emptyPath = process.env.WARDIAN_COMPOSER_EMPTY_SCREENSHOT
    ?? testInfo.outputPath("composer-send-empty.png");
  await card.screenshot({ path: emptyPath, animations: "disabled" });
  await testInfo.attach("composer-send-empty", { path: emptyPath, contentType: "image/png" });

  await input.fill("Check the composer action styling.");
  await expect(sendButton).toBeEnabled();
  await expect(sendButton).toHaveCSS("background-color", "rgb(146, 106, 9)");

  const textPath = process.env.WARDIAN_COMPOSER_TEXT_SCREENSHOT
    ?? testInfo.outputPath("composer-send-text.png");
  await card.screenshot({ path: textPath, animations: "disabled" });
  await testInfo.attach("composer-send-text", { path: textPath, contentType: "image/png" });

  await page.evaluate(() => {
    const runtime = (window as unknown as {
      __WARDIAN_WORKBENCH_IPC_MOCK__: { emit: (event: string, payload: unknown) => void };
    }).__WARDIAN_WORKBENCH_IPC_MOCK__;
    runtime.emit("agent-status-updated", {
      session_id: "agent-alpha",
      current_status: "Processing...",
    });
  });
  const queueButton = page.getByRole("button", { name: "Queue message" });
  await expect(queueButton).toBeEnabled();
  await expect(queueButton).toHaveCSS("background-color", "rgb(146, 106, 9)");

  const queuePath = process.env.WARDIAN_COMPOSER_QUEUE_SCREENSHOT
    ?? testInfo.outputPath("composer-send-queue.png");
  await card.screenshot({ path: queuePath, animations: "disabled" });
  await testInfo.attach("composer-send-queue", { path: queuePath, contentType: "image/png" });
});

test("serializes chat model selection while persistence and live application are active", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("chat-model-saving-evidence", "agents-overview", {
    state: {
      mode: "single",
      focused_agent_id: "agent-alpha",
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 6, surfaces: [overview] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  const ipc = await installWorkbenchIpcMock(page, {
    agents: [{
      ...agents[0],
      provider: "codex",
      model: "gpt-5.6-sol",
      provider_config: { type: "codex", reasoning_effort: "low" },
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "chat-model-saving-evidence-token-6",
    },
    response_delays_ms: { update_agent_model_selection: 1_500 },
    responses: {
      list_provider_model_catalog: {
        provider: "codex",
        version: "codex-cli 0.146.0",
        source: "live_catalog",
        refresh_error: null,
        models: [
          {
            id: "gpt-5.6-sol",
            display_name: "5.6 Terra",
            effort_options: ["low", "high"],
            default_effort: "low",
            is_default: true,
          },
          {
            id: "gpt-5.6-luna",
            display_name: "5.6 Luna",
            effort_options: ["low", "high"],
            default_effort: "high",
            is_default: false,
          },
        ],
      },
      update_agent_model_selection: {
        config: {
          ...agents[0],
          provider: "codex",
          model: "gpt-5.6-luna",
          provider_config: { type: "codex", reasoning_effort: "high" },
        },
        live_application: "applied",
        live_error: null,
      },
      submit_prompt_to_agent: null,
    },
  });

  await page.goto("/", { waitUntil: "domcontentloaded" });
  const card = page.getByTestId("agent-card");
  const model = card.getByLabel("Model", { exact: true });
  const effort = card.getByLabel("Effort");
  await expect(card).toBeVisible();
  await expect(model).toBeEnabled();
  await model.selectOption("gpt-5.6-luna");

  await expect(model).toBeDisabled();
  await expect(effort).toBeDisabled();
  await expect(card.getByRole("status")).toHaveText("Applying model…");
  await expect.poll(async () => (await ipc.calls("update_agent_model_selection")).length).toBe(1);

  const path = process.env.WARDIAN_CHAT_MODEL_SAVING_SCREENSHOT
    ?? testInfo.outputPath("chat-model-saving.png");
  await card.screenshot({ path, animations: "disabled" });
  await testInfo.attach("chat-model-saving", { path, contentType: "image/png" });

  await expect(model).toBeEnabled();
  await expect(effort).toBeEnabled();
  expect(await ipc.calls("update_agent_model_selection")).toHaveLength(1);

  await card.getByRole("button", { name: /mode: Chat\. Switch to Terminal\./ }).click();
  await expect(card.getByRole("button", { name: /mode: Terminal\. Switch to Chat\./ })).toBeVisible();
  await card.getByRole("button", { name: /mode: Terminal\. Switch to Chat\./ }).click();
  await expect(card.getByLabel("Model", { exact: true })).toHaveValue("gpt-5.6-luna");
  await expect(card.getByLabel("Effort")).toHaveValue("high");

  const restoredPath = process.env.WARDIAN_CHAT_MODEL_RESTORED_SCREENSHOT
    ?? testInfo.outputPath("chat-model-restored.png");
  await card.screenshot({ path: restoredPath, animations: "disabled" });
  await testInfo.attach("chat-model-restored", { path: restoredPath, contentType: "image/png" });

  await card.getByLabel("Effort").selectOption("low");
  await expect.poll(async () => (await ipc.calls("update_agent_model_selection")).length).toBe(2);
  expect((await ipc.calls("update_agent_model_selection"))[1]?.args).toEqual({
    sessionId: "agent-alpha",
    model: "gpt-5.6-luna",
    reasoningEffort: "low",
  });
});

test("keeps assistant and user message geometry stable while copying", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("copy-feedback-evidence", "agents-overview", {
    state: {
      mode: "single",
      focused_agent_id: "agent-alpha",
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 4, surfaces: [overview] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("wardian-settings", JSON.stringify({
      state: { gridCardDisplayMode: "chat" },
      version: 2,
    }));
  });
  await installWorkbenchIpcMock(page, {
    agents: [{
      ...agents[0],
      provider: "codex",
      model: "gpt-5.6-sol",
      provider_config: { type: "codex", reasoning_effort: "high" },
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "copy-feedback-evidence-token-4",
    },
    responses: {
      load_agent_chat_transcript: [{
        id: "copy-feedback-user-message",
        session_id: "agent-alpha",
        provider: "mock",
        kind: "message",
        role: "user",
        text: "User request with enough text to wrap across several lines while the copy control appears and reports success.",
        title: null,
        status: null,
        turn_id: null,
        source: null,
        command: null,
        exit_code: null,
        path: null,
        language: null,
        created_at: null,
        sequence: 1,
        metadata: {},
      }, {
        id: "copy-feedback-assistant-message",
        session_id: "agent-alpha",
        provider: "mock",
        kind: "message",
        role: "assistant",
        text: "Assistant response with enough text to wrap across several lines while the copy control appears and reports success.",
        title: null,
        status: null,
        turn_id: null,
        source: null,
        command: null,
        exit_code: null,
        path: null,
        language: null,
        created_at: null,
        sequence: 2,
        metadata: {},
      }],
      "plugin:clipboard-manager|write_text": null,
      list_provider_model_catalog: {
        provider: "codex",
        version: "codex-cli 0.146.0",
        source: "live_catalog",
        refresh_error: null,
        models: [{
          id: "gpt-5.6-sol",
          display_name: "5.6 Terra",
          effort_options: ["low", "high"],
          default_effort: "high",
          is_default: true,
        }],
      },
    },
  });

  await page.goto("/");
  const card = page.getByTestId("agent-card");
  const transcript = card.getByTestId("agent-chat-transcript");
  const transcriptWidthMetrics = await transcript.evaluate((element) => {
    const probe = document.createElement("span");
    probe.style.cssText = "position:absolute; width:1ch; height:0; overflow:hidden;";
    element.append(probe);
    const chWidth = probe.getBoundingClientRect().width;
    probe.remove();
    return { chWidth, maxWidth: Number.parseFloat(getComputedStyle(element).maxWidth) };
  });
  expect(transcriptWidthMetrics.maxWidth).toBeCloseTo(transcriptWidthMetrics.chWidth * 76, 0);
  const rows = [
    card.getByLabel("assistant message"),
    card.getByLabel("user message"),
  ];
  await expect(rows[0]).toBeVisible();
  await expect(rows[1]).toBeVisible();

  for (const row of rows) {
    const baseline = await chatGeometry(row);
    await expectChatTextUnobscured(row, baseline);

    await row.hover();
    const copyButton = row.getByRole("button", { name: /^Copy message(?: copied)?$/ });
    await expect(copyButton).toBeVisible();
    const afterHover = await chatGeometry(row);
    expectChatGeometryUnchanged(baseline, afterHover);
    await expectChatTextUnobscured(row, afterHover, copyButton);

    await copyButton.focus();
    await expect(copyButton).toBeFocused();
    const afterFocus = await chatGeometry(row);
    expectChatGeometryUnchanged(baseline, afterFocus);
    await expectChatTextUnobscured(row, afterFocus, copyButton);

    await copyButton.click();
    await expect(row.getByRole("button", { name: "Copy message copied" })).toBeVisible();
    const afterCopy = await chatGeometry(row);
    expectChatGeometryUnchanged(baseline, afterCopy);
    await expectChatTextUnobscured(row, afterCopy, row.getByRole("button", { name: "Copy message copied" }));
  }

  const screenshotDir = process.env.WARDIAN_CHAT_COPY_LAYOUT_SCREENSHOT_DIR;
  if (screenshotDir) mkdirSync(screenshotDir, { recursive: true });
  const screenshotPath = process.env.WARDIAN_CHAT_COPY_LAYOUT_SCREENSHOT
    ?? process.env.WARDIAN_COPY_FEEDBACK_SCREENSHOT
    ?? (screenshotDir ? path.join(screenshotDir, "desktop-copy-layout.png") : testInfo.outputPath("copy-layout-desktop.png"));
  await card.screenshot({ path: screenshotPath, animations: "disabled" });
  await testInfo.attach("chat-copy-layout-desktop", { path: screenshotPath, contentType: "image/png" });
});

test("renders a capture-ready new-tab surface launcher", async ({ page }, testInfo) => {
  const dashboard = makeWorkbenchSurface("dashboard-launcher-evidence", "dashboard");
  const document = makeWorkbenchDocument({ revision: 3, surfaces: [dashboard] });
  await page.setViewportSize({ width: 1440, height: 900 });
  await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "launcher-evidence-token-3",
    },
  });

  await page.goto("/");
  const group = page.getByTestId("workbench-group").filter({
    has: surfaceTab(page, "dashboard"),
  });
  await group.getByLabel("Open Surface", { exact: true }).click();
  await expect(group.getByRole("tab", { name: "New Tab", exact: true }))
    .toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "Choose a surface" })).toBeVisible();
  await expect(page.getByLabel("Available surfaces").getByRole("button")).toHaveCount(8);
  await expect(page.getByText("Monitor active agents.", { exact: true })).toBeVisible();
  await page.waitForTimeout(250);

  const path = process.env.WARDIAN_WORKBENCH_LAUNCHER_SCREENSHOT
    ?? testInfo.outputPath("surface-launcher.png");
  await page.screenshot({ path, animations: "disabled" });
  await testInfo.attach("surface-launcher", { path, contentType: "image/png" });
});
