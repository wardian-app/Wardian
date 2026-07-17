import { expect, test, type Locator, type Page } from "@playwright/test";
import path from "node:path";

import type { AppSettings } from "../../src/types/settings";
import type { WorkbenchDocumentV1 } from "../../src/types";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchIpcMockController,
} from "../fixtures/workbenchIpcMock";

const ROOT = "/workspace/project";
const ALPHA_PATH = `${ROOT}/alpha.md`;
const BETA_PATH = `${ROOT}/beta.md`;

function filesTab(page: Page, filePath: string): Locator {
  return page.getByRole("tab").and(page.locator(
    `[data-surface-type="files"][data-resource-key=${JSON.stringify(`file:${filePath}`)}]`,
  ));
}

async function bootFilesWorkbench(page: Page): Promise<WorkbenchIpcMockController> {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const document: WorkbenchDocumentV1 = makeWorkbenchDocument({ surfaces: [dashboard] });
  const settings: AppSettings = {
    theme: "light",
    auto_patch_gemini: false,
    terminal_font_size: 14,
    terminal_font_family: null,
    grid_card_display_mode: "chat",
    watchlist_new_agent_position: "top",
    titlebar_telemetry_visible: true,
    external_editor: "system",
    external_editor_custom_executable: null,
    explorer_file_click_action: "preview",
    workbench_new_tab_action: "home",
  };
  const ipc = await installWorkbenchIpcMock(page, {
    explorer_root: ROOT,
    files: [
      { path: ALPHA_PATH, content: "# Alpha document\n\nFirst file." },
      { path: BETA_PATH, content: "# Beta document\n\nSecond file." },
    ],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: `mock-token-${document.revision}`,
    },
    agents: [{
      session_id: "agent-files",
      session_name: "Files Agent",
      agent_class: "Coder",
      folder: ROOT,
      provider: "mock",
      is_off: false,
    }],
    responses: {
      load_app_settings: {
        schema_version: 2,
        settings,
        overrides: {},
        persisted: true,
      },
    },
  });
  await page.goto("/");
  await expect(page.getByTestId("workbench-host")).toBeVisible();
  await page.getByTestId("sidebar-tab-explorer").click();
  await expect(page.getByRole("tree", { name: "Workspace files" })).toBeVisible();
  return ipc;
}

async function persistedDocument(ipc: WorkbenchIpcMockController) {
  return (await ipc.snapshot()).load_result.document;
}

test("routes Explorer files through transient, permanent, and side Workbench presentations", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const ipc = await bootFilesWorkbench(page);
  const alphaRow = page.getByRole("treeitem", { name: "alpha.md" });
  const betaRow = page.getByRole("treeitem", { name: "beta.md" });

  await alphaRow.click();
  const alphaTransient = filesTab(page, ALPHA_PATH);
  await expect(alphaTransient).toBeVisible();
  const transientSurfaceId = await alphaTransient.getAttribute("data-surface-id");
  expect(transientSurfaceId).toBeTruthy();
  await expect(page.getByRole("heading", { name: "Alpha document" })).toBeVisible();

  await betaRow.click();
  const betaTransient = filesTab(page, BETA_PATH);
  await expect(betaTransient).toBeVisible();
  await expect(alphaTransient).toHaveCount(0);
  await expect(betaTransient).toHaveAttribute("data-surface-id", transientSurfaceId!);
  await expect(page.getByRole("heading", { name: "Beta document" })).toBeVisible();
  await expect.poll(async () => (await ipc.calls("close_file_resource")).length).toBeGreaterThan(0);

  await ipc.updateFile(BETA_PATH, "# Beta updated\n\nStable revision event.");
  await expect(page.getByRole("heading", { name: "Beta updated" })).toBeVisible();

  await betaRow.dblclick();
  await expect(betaTransient).toHaveCount(1);
  await expect(betaTransient).toHaveAttribute("data-surface-id", transientSurfaceId!);
  await expect.poll(async () => {
    const document = await persistedDocument(ipc);
    return document.surfaces[transientSurfaceId!]?.state;
  }).toMatchObject({ transient_preview: false });

  await alphaRow.click();
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(1);
  await expect(betaTransient).toHaveCount(1);
  await expect(page.getByRole("tab").and(page.locator('[data-surface-type="files"]'))).toHaveCount(2);

  await betaRow.click({ button: "right" });
  await page.getByRole("button", { name: "Open to Side", exact: true }).click();
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);
  await expect(betaTransient).toHaveCount(2);
  await expect(page.getByRole("heading", { name: "Beta updated" })).toBeVisible();

  expect(await ipc.calls("read_file_preview")).toEqual([]);
  expect((await ipc.calls("open_file_resource")).length).toBeGreaterThan(0);
  expect((await ipc.calls("read_file_resource_text")).length).toBeGreaterThan(0);

  const screenshotPath = path.resolve(
    "e2e/screenshots/files-surface/2026-07-16T2305Z/explorer-files-tabs.png",
  );
  await page.screenshot({ path: screenshotPath, fullPage: true });
});
