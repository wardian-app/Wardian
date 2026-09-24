import { readFileSync } from "node:fs";
import * as path from "node:path";
import { expect, test } from "@playwright/test";

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

test("Issue 1407 imports a maintenance plan and reviews the seeded preview", async ({ page }) => {
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
      memory_maintenance_preview: preview,
    },
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

  const previewCalls = await ipc.calls("memory_maintenance_preview");
  expect(previewCalls).toHaveLength(1);
  expect(previewCalls[0]?.args?.plan).toEqual(plan);
  expect(await ipc.calls("memory_maintenance_apply")).toHaveLength(0);
});
