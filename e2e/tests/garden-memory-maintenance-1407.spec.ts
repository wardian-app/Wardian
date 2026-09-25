import { mkdirSync, readFileSync } from "node:fs";
import * as path from "node:path";
import { expect, test, type Page } from "@playwright/test";

import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  type WorkbenchAgentFixture,
} from "../fixtures/workbenchIpcMock";
import { openSurface, surfacePanel } from "../fixtures/workbench";

const fixtureDirectory = path.join(
  process.cwd(),
  "e2e",
  "fixtures",
  "garden-memory-maintenance-1407",
);
const planPath = path.join(fixtureDirectory, "maintenance-plan.json");
const plan = JSON.parse(readFileSync(planPath, "utf8")) as Record<string, unknown>;
const preview = JSON.parse(
  readFileSync(path.join(fixtureDirectory, "maintenance-preview.json"), "utf8"),
) as Record<string, unknown>;
const atlas: WorkbenchAgentFixture = {
  session_id: "fixture-agent-1407",
  session_name: "Atlas",
  agent_class: "Analyst",
  folder: "/fixture/workspace",
  provider: "mock",
  is_off: false,
};

async function openMaintenance(
  page: Page,
  ipcOptions: { errors?: Record<string, string>; responses?: Record<string, unknown> } = {},
) {
  await page.setViewportSize({ width: 1920, height: 1080 });
  const document = makeWorkbenchDocument({
    shell: { left_sidebar_collapsed: true, right_sidebar_collapsed: true },
  });
  const ipc = await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "garden-memory-fixture-token",
    },
    agents: [atlas],
    responses: {
      memory_list: [],
      list_conversations: { schema: 1, conversations: [] },
      ...ipcOptions.responses,
    },
    errors: ipcOptions.errors,
  });

  await page.goto("/");
  await expect(page.locator('[data-testid="app-shell"]')).toBeVisible();
  await openSurface(page, "garden");

  const garden = surfacePanel(page, "garden");
  const atlasObject = garden.locator('[data-garden-object="agent:fixture-agent-1407"]');
  await expect(atlasObject).toBeVisible();
  await atlasObject.press("Enter");

  const memoryRegion = garden.getByRole("region", { name: "Memory" });
  await expect(memoryRegion).toBeVisible();
  await memoryRegion.getByRole("button", { name: "Maintain memory…" }).click();

  const dialog = page.getByRole("dialog", { name: "Memory maintenance" });
  await expect(dialog).toBeVisible();
  return { dialog, ipc };
}

test("Issue 1407 imports a maintenance plan and reviews the seeded preview", async ({ page }) => {
  const { dialog, ipc } = await openMaintenance(page, {
    responses: {
      memory_maintenance_parse: plan,
      memory_maintenance_preview: preview,
    },
  });
  await dialog.getByLabel("Maintenance plan (.json)").setInputFiles(planPath);

  await expect(dialog.getByText("maintenance-plan.json")).toBeVisible();
  await expect(dialog.getByText("sha256:8b6d4c2a59f0212c")).toBeVisible();
  await expect(dialog.getByText("No conflicts found in this preview.")).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Operation 1: revise" })).toBeVisible();
  await expect(dialog.getByText("fixture-revision-4")).toBeVisible();
  await expect(dialog.getByText("Allocated on apply")).toBeVisible();
  await expect(dialog.getByText("Agent-wide")).toHaveCount(2);
  await expect(dialog.getByText("No sources")).toBeVisible();
  await expect(dialog.getByText("setup.md")).toHaveCount(2);
  await expect(dialog.getByRole("button", { name: "Apply reviewed plan…" })).toBeEnabled();

  const parseCalls = await ipc.calls("memory_maintenance_parse");
  expect(parseCalls).toHaveLength(1);
  expect(parseCalls[0]?.args?.rawJson).toBe(readFileSync(planPath, "utf8"));
  const previewCalls = await ipc.calls("memory_maintenance_preview");
  expect(previewCalls).toHaveLength(1);
  expect(previewCalls[0]?.args?.plan).toEqual(plan);
  expect(await ipc.calls("memory_maintenance_apply")).toHaveLength(0);
});

test("Issue 1414 rejects duplicate scope keys before preview", async ({ page }) => {
  const rawPlan = `{"schema_version":1,"plan_id":"plan-1414","agent_id":"fixture-agent-1407","idempotency_key":"key-1414","operations":[{"op":"create","client_key":"new-1","text":"sanitized fixture","kind":"stable","scope":{"kind":"agent","kind":"workspace","path":"/fixture/workspace"},"evidence_excerpt":"sanitized fixture evidence"}]}`;
  const { dialog, ipc } = await openMaintenance(page, {
    errors: { memory_maintenance_parse: "duplicate field `kind` at line 1 column 256" },
  });
  await dialog.locator('input[type="file"]').setInputFiles({
    name: "duplicate-scope-plan.json",
    mimeType: "application/json",
    buffer: Buffer.from(rawPlan, "utf8"),
  });

  await expect(dialog.getByRole("alert")).toContainText("duplicate field `kind`");
  const parseCalls = await ipc.calls("memory_maintenance_parse");
  expect(parseCalls).toHaveLength(1);
  expect(parseCalls[0]?.args?.rawJson).toBe(rawPlan);
  expect(await ipc.calls("memory_maintenance_preview")).toHaveLength(0);
  expect(await ipc.calls("memory_maintenance_apply")).toHaveLength(0);

  const screenshotDirectory = process.env.WARDIAN_E2E_SCREENSHOT_DIR;
  if (screenshotDirectory) {
    mkdirSync(screenshotDirectory, { recursive: true });
    await dialog.screenshot({
      path: path.join(screenshotDirectory, "duplicate-scope-import-error.png"),
    });
  }
});
