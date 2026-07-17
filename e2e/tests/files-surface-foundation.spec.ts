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
const WIDE_PDF_PATH = `${ROOT}/wide.pdf`;

function blankPdfDataUrl(width: number, height: number) {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${width} ${height}] /Resources <<>> /Contents 4 0 R >>`,
    "<< /Length 0 >>\nstream\n\nendstream",
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(pdf, "ascii"));
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xrefOffset = Buffer.byteLength(pdf, "ascii");
  pdf += `xref\n0 ${objects.length + 1}\n`;
  pdf += "0000000000 65535 f \n";
  pdf += offsets.slice(1)
    .map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`)
    .join("");
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\n`;
  pdf += `startxref\n${xrefOffset}\n%%EOF\n`;
  return `data:application/pdf;base64,${Buffer.from(pdf, "ascii").toString("base64")}`;
}

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
      {
        path: WIDE_PDF_PATH,
        content: "wide PDF fixture",
        mime_type: "application/pdf",
        renderer_kind: "pdf",
        stream_url: blankPdfDataUrl(1_000, 400),
      },
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

test("keeps oversized PDF page origins reachable and centers pages that fit", async ({ page }) => {
  await page.setViewportSize({ width: 1800, height: 900 });
  await bootFilesWorkbench(page);
  await page.getByRole("treeitem", { name: "wide.pdf" }).click();

  const surface = page.getByTestId("files-surface");
  const viewport = page.getByRole("region", { name: "PDF document viewport" });
  const canvas = page.getByLabel("PDF page 1");
  await expect(canvas).toBeVisible();

  for (const paneWidth of [100, 300]) {
    await surface.evaluate((element, width) => {
      element.style.width = `${width}px`;
      element.style.maxWidth = `${width}px`;
      element.style.flex = `0 0 ${width}px`;
    }, paneWidth);
    await expect.poll(async () => viewport.evaluate((element) => element.clientWidth))
      .toBe(paneWidth);
    const geometry = await viewport.evaluate((viewportElement) => {
      const pageElement = document.querySelector<HTMLElement>('.files-pdf-page[data-page-number="1"]');
      const canvasElement = document.querySelector<HTMLElement>('[aria-label="PDF page 1"]');
      if (!pageElement || !canvasElement) throw new Error("PDF page geometry is unavailable");
      const viewportRect = viewportElement.getBoundingClientRect();
      const pageRect = pageElement.getBoundingClientRect();
      const canvasRect = canvasElement.getBoundingClientRect();
      return {
        viewport_left: viewportRect.left,
        page_left: pageRect.left,
        canvas_left: canvasRect.left,
        client_width: viewportElement.clientWidth,
        scroll_width: viewportElement.scrollWidth,
      };
    });
    expect(geometry.page_left).toBeGreaterThanOrEqual(geometry.viewport_left - 0.5);
    expect(geometry.canvas_left).toBeGreaterThanOrEqual(geometry.viewport_left - 0.5);
    expect(geometry.scroll_width).toBeGreaterThan(geometry.client_width);
  }

  await surface.evaluate((element) => {
    element.style.width = "1400px";
    element.style.maxWidth = "1400px";
    element.style.flex = "0 0 1400px";
  });
  await expect.poll(async () => viewport.evaluate((element) => element.clientWidth)).toBe(1400);
  const centered = await viewport.evaluate((viewportElement) => {
    const pageElement = document.querySelector<HTMLElement>('.files-pdf-page[data-page-number="1"]');
    if (!pageElement) throw new Error("PDF page geometry is unavailable");
    const viewportRect = viewportElement.getBoundingClientRect();
    const pageRect = pageElement.getBoundingClientRect();
    return {
      actual_left: pageRect.left - viewportRect.left,
      expected_left: (viewportRect.width - pageRect.width) / 2,
    };
  });
  expect(Math.abs(centered.actual_left - centered.expected_left)).toBeLessThan(1);
});
