import { expect, test, type Locator, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
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
const ALPHA_COPY_PATH = `${ROOT}/alpha-copy.md`;
const WIDE_PDF_PATH = `${ROOT}/wide.pdf`;
const IMAGE_PATH = `${ROOT}/figure.png`;

const ONE_PIXEL_PNG = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

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

function contentHash(text: string) {
  return `sha256:${createHash("sha256").update(text).digest("hex")}`;
}

async function invokeMock<T>(
  page: Page,
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return await page.evaluate(async ({ mockCommand, mockArgs }) => {
    const invoke = (window as Window & {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
    }).__TAURI_INTERNALS__.invoke;
    return await invoke(mockCommand, mockArgs);
  }, { mockCommand: command, mockArgs: args }) as T;
}

interface RecoveryCheckpoint {
  recovery_id: string;
  recovery_revision: number;
  base_content_hash: string;
  base_opaque_revision: string;
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
    save_target_path: ALPHA_COPY_PATH,
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
      {
        path: IMAGE_PATH,
        content: "image fixture",
        mime_type: "image/png",
        renderer_kind: "image",
        stream_url: ONE_PIXEL_PNG,
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

test("switches Markdown source without reopening the resource and preserves it per tab", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const ipc = await bootFilesWorkbench(page);
  const alphaRow = page.getByRole("treeitem", { name: "alpha.md" });
  const betaRow = page.getByRole("treeitem", { name: "beta.md" });

  await alphaRow.dblclick();
  await expect(page.getByRole("heading", { name: "Alpha document" })).toBeVisible();
  const openCount = (await ipc.calls("open_file_resource")).length;
  const alphaTextReads = await ipc.calls("read_file_resource_text");
  const firstAlphaRead = alphaTextReads[0]?.args?.request as {
    resource_id?: string;
  } | undefined;
  expect(firstAlphaRead?.resource_id).toBe(`file:${ALPHA_PATH}`);
  expect(JSON.stringify(alphaTextReads)).not.toContain("mock-file:");

  const editSource = page.getByRole("button", { name: "Edit source" });
  await expect(editSource.locator("svg.lucide-book-open")).toBeVisible();
  await expect(editSource).toHaveAttribute("aria-pressed", "false");
  await editSource.click();
  await expect(page.getByTestId("monaco-text-renderer")).toBeVisible();
  await expect(page.getByRole("button", { name: "View rendered" }))
    .toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("button", { name: "View rendered" })
    .locator("svg.lucide-pencil")).toBeVisible();
  expect((await ipc.calls("open_file_resource")).length).toBe(openCount);

  await betaRow.dblclick();
  await expect(page.getByRole("heading", { name: "Beta document" })).toBeVisible();
  await filesTab(page, ALPHA_PATH).click();
  await expect(page.getByTestId("monaco-text-renderer")).toBeVisible();
  await expect(page.getByRole("button", { name: "View rendered" })).toBeVisible();

  const surface = page.locator('[data-testid="files-surface"]:not([data-suspended="true"])');
  await surface.evaluate((element) => {
    element.style.width = "220px";
    element.style.maxWidth = "220px";
    element.style.flex = "0 0 220px";
  });
  const keyboardOpenCount = (await ipc.calls("open_file_resource")).length;
  const toggle = surface.getByRole("button", { name: "View rendered" });
  const fileActions = surface.getByRole("button", { name: "File actions" });
  await fileActions.focus();
  await expect(fileActions).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(toggle).toBeFocused();
  const [toggleBox, surfaceBox] = await Promise.all([toggle.boundingBox(), surface.boundingBox()]);
  expect(toggleBox).not.toBeNull();
  expect(surfaceBox).not.toBeNull();
  expect(toggleBox!.x).toBeGreaterThanOrEqual(surfaceBox!.x - 0.5);
  expect(toggleBox!.x + toggleBox!.width).toBeLessThanOrEqual(surfaceBox!.x + surfaceBox!.width + 0.5);

  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "Alpha document" })).toBeVisible();
  const keyboardEditSource = surface.getByRole("button", { name: "Edit source" });
  await expect(keyboardEditSource).toBeFocused();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("monaco-text-renderer")).toBeVisible();
  await expect(surface.getByRole("button", { name: "View rendered" })).toBeFocused();
  expect((await ipc.calls("open_file_resource")).length).toBe(keyboardOpenCount);

  await page.screenshot({
    path: path.resolve(
      "e2e/screenshots/files-editor/2026-07-18/markdown-editor-toggle.png",
    ),
    fullPage: true,
  });

  const recoveryOwner = await invokeMock<{
    resource_id: string;
    subscription_id: string;
  }>(page, "open_file_resource", {
    request: {
      path: ALPHA_PATH,
      agent_id: null,
      user_file_capability_id: null,
    },
  });
  await ipc.updateFile(ALPHA_PATH, "# External disk head\n");
  const recoveryBase = "# Alpha document\n\nFirst file.";
  const firstCheckpoint = await invokeMock<RecoveryCheckpoint>(
    page,
    "checkpoint_file_recovery",
    {
      request: {
        recovery_id: null,
        expected_recovery_revision: null,
        resource_id: recoveryOwner.resource_id,
        subscription_id: recoveryOwner.subscription_id,
        base_content_hash: contentHash(recoveryBase),
        resource_key: recoveryOwner.resource_id,
        base: recoveryBase,
        buffer: "first unsaved generation\n",
      },
    },
  );
  expect(firstCheckpoint).toMatchObject({
    recovery_revision: 1,
    base_content_hash: contentHash(recoveryBase),
  });

  const rebased = "# Accepted rebased base\n";
  const rebasedCheckpoint = await invokeMock<RecoveryCheckpoint>(
    page,
    "checkpoint_file_recovery",
    {
      request: {
        recovery_id: firstCheckpoint.recovery_id,
        expected_recovery_revision: firstCheckpoint.recovery_revision,
        resource_id: recoveryOwner.resource_id,
        subscription_id: recoveryOwner.subscription_id,
        base_content_hash: contentHash(rebased),
        resource_key: recoveryOwner.resource_id,
        base: rebased,
        buffer: "rebased unsaved generation\n",
      },
    },
  );
  expect(rebasedCheckpoint).toMatchObject({
    recovery_id: firstCheckpoint.recovery_id,
    recovery_revision: 2,
    base_content_hash: contentHash(rebased),
  });
  expect(rebasedCheckpoint.base_opaque_revision)
    .not.toBe(firstCheckpoint.base_opaque_revision);

  await expect(invokeMock(
    page,
    "checkpoint_file_recovery",
    {
      request: {
        recovery_id: rebasedCheckpoint.recovery_id,
        expected_recovery_revision: 1,
        resource_id: recoveryOwner.resource_id,
        subscription_id: recoveryOwner.subscription_id,
        base_content_hash: contentHash(rebased),
        resource_key: recoveryOwner.resource_id,
        base: rebased,
        buffer: "stale CAS generation\n",
      },
    },
  )).rejects.toThrow(/stale_recovery_revision/);

  await expect(invokeMock(
    page,
    "checkpoint_file_recovery",
    {
      request: {
        recovery_id: null,
        expected_recovery_revision: null,
        resource_id: recoveryOwner.resource_id,
        subscription_id: recoveryOwner.subscription_id,
        base_content_hash: "sha256:not-the-base",
        resource_key: recoveryOwner.resource_id,
        base: recoveryBase,
        buffer: "invalid hash generation\n",
      },
    },
  ))
    .rejects.toThrow(/base content does not match/i);
});

test("edits and saves in place, then Save As opens an ordinary file without retargeting", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const ipc = await bootFilesWorkbench(page);
  await page.getByRole("treeitem", { name: "alpha.md" }).dblclick();
  await page.getByRole("button", { name: "Edit source" }).click();

  const activeSurface = page.locator('[data-testid="files-surface"]:not([data-suspended="true"])');
  const editorInput = activeSurface.locator(".monaco-editor .native-edit-context");
  const editorLines = activeSurface.locator(".monaco-editor .view-lines");
  await editorLines.click({ position: { x: 120, y: 8 } });
  await expect(editorInput).toBeFocused();
  const savedText = "# Alpha edited\n\nSaved from Monaco.";
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("# Alpha edited");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Saved from Monaco.");

  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]')).toBeVisible();
  await expect(activeSurface.getByLabel("Unsaved changes")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+S");
  await expect.poll(async () => (await ipc.calls("save_file_resource_text")).length).toBe(1);
  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]')).toHaveCount(0);
  const saveRequest = (await ipc.calls("save_file_resource_text"))[0]?.args?.request as {
    expected_revision?: number;
    buffer_base_hash?: string;
    text?: string;
  } | undefined;
  expect(saveRequest).toMatchObject({
    expected_revision: 1,
    buffer_base_hash: contentHash("# Alpha document\n\nFirst file."),
    text: savedText,
  });

  const copiedText = "# Alpha copy\n\nUnsaved source retained.";
  await editorLines.click({ position: { x: 120, y: 8 } });
  await expect(editorInput).toBeFocused();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("# Alpha copy");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Unsaved source retained.");
  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]')).toBeVisible();

  await activeSurface.getByRole("button", { name: "File actions" }).click();
  await page.getByRole("menuitem", { name: "Save As" }).click();
  await expect(filesTab(page, ALPHA_COPY_PATH)).toBeVisible();
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(1);
  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]')).toBeVisible();
  const saveAsRequest = (await ipc.calls("save_file_resource_as_text"))[0]?.args?.request as {
    save_target_grant_id?: string;
    text?: string;
  } | undefined;
  expect(saveAsRequest).toEqual({
    save_target_grant_id: "mock-save-target-1",
    text: copiedText,
  });
  const copyOpen = (await ipc.calls("open_file_resource")).find((call) => (
    (call.args?.request as { path?: string } | undefined)?.path === ALPHA_COPY_PATH
  ));
  expect(copyOpen).toBeTruthy();

  await filesTab(page, ALPHA_PATH).click();
  await expect(activeSurface.locator(".monaco-editor .view-lines")).toBeVisible();
  await expect(activeSurface.getByLabel("Unsaved changes")).toBeVisible();

  await page.screenshot({
    path: path.resolve(
      "e2e/screenshots/files-editor/2026-07-18/conventional-files-editor.png",
    ),
    fullPage: true,
  });
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

test("keeps Files chrome and image controls reachable in 100px and 300px panes", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await bootFilesWorkbench(page);
  await page.getByRole("treeitem", { name: "figure.png" }).click();

  const surface = page.getByTestId("files-surface");
  const breadcrumb = page.getByRole("navigation", { name: "File location" });
  const actions = page.getByRole("button", { name: "File actions" });
  const imageToolbar = page.getByRole("toolbar", { name: "Image controls" });
  await expect(page.getByRole("img", { name: "figure.png" })).toBeVisible();

  for (const paneWidth of [100, 300]) {
    await surface.evaluate((element, width) => {
      element.style.width = `${width}px`;
      element.style.maxWidth = `${width}px`;
      element.style.flex = `0 0 ${width}px`;
    }, paneWidth);
    await expect.poll(async () => surface.evaluate((element) => element.clientWidth))
      .toBe(paneWidth);

    const chromeGeometry = await surface.evaluate((surfaceElement) => {
      const breadcrumbElement = surfaceElement.querySelector<HTMLElement>(".files-breadcrumb");
      const actionsElement = surfaceElement.querySelector<HTMLElement>(".files-overflow-trigger");
      if (!breadcrumbElement || !actionsElement) throw new Error("Files chrome is unavailable");
      const surfaceRect = surfaceElement.getBoundingClientRect();
      const breadcrumbRect = breadcrumbElement.getBoundingClientRect();
      const actionsRect = actionsElement.getBoundingClientRect();
      return {
        surface_left: surfaceRect.left,
        surface_right: surfaceRect.right,
        breadcrumb_left: breadcrumbRect.left,
        breadcrumb_right: breadcrumbRect.right,
        breadcrumb_width: breadcrumbRect.width,
        actions_left: actionsRect.left,
        actions_right: actionsRect.right,
      };
    });
    expect(chromeGeometry.breadcrumb_width).toBeGreaterThan(0);
    expect(chromeGeometry.breadcrumb_left).toBeGreaterThanOrEqual(chromeGeometry.surface_left - 0.5);
    expect(chromeGeometry.breadcrumb_right).toBeLessThanOrEqual(chromeGeometry.surface_right + 0.5);
    expect(chromeGeometry.actions_left).toBeGreaterThanOrEqual(chromeGeometry.surface_left - 0.5);
    expect(chromeGeometry.actions_right).toBeLessThanOrEqual(chromeGeometry.surface_right + 0.5);

    await expect(page.getByRole("tablist", { name: "File mode" })).toHaveCount(0);
    for (const legacyMode of ["Preview", "Changes", "Draft"]) {
      await expect(page.getByRole("tab", { name: legacyMode })).toHaveCount(0);
    }

    await actions.click();
    const menu = page.getByRole("menu", { name: "File actions" });
    await expect(menu).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Open With" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Reveal" })).toBeVisible();
    const [menuBox, surfaceBox] = await Promise.all([menu.boundingBox(), surface.boundingBox()]);
    expect(menuBox).not.toBeNull();
    expect(surfaceBox).not.toBeNull();
    expect(menuBox!.x).toBeGreaterThanOrEqual(surfaceBox!.x - 0.5);
    expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(surfaceBox!.x + surfaceBox!.width + 0.5);
    await page.keyboard.press("Escape");

    const toolbarMetrics = await imageToolbar.evaluate((element) => ({
      client_width: element.clientWidth,
      scroll_width: element.scrollWidth,
    }));
    expect(toolbarMetrics.client_width).toBeLessThanOrEqual(paneWidth);
    if (paneWidth === 100) expect(toolbarMetrics.scroll_width).toBeGreaterThan(toolbarMetrics.client_width);
    for (const control of ["Fit", "100%", "Zoom out", "Zoom in"]) {
      const button = page.getByRole("button", { name: control, exact: true });
      await button.focus();
      await expect(button).toBeFocused();
      const [buttonBox, toolbarBox] = await Promise.all([button.boundingBox(), imageToolbar.boundingBox()]);
      expect(buttonBox).not.toBeNull();
      expect(toolbarBox).not.toBeNull();
      expect(buttonBox!.x).toBeGreaterThanOrEqual(toolbarBox!.x - 0.5);
      expect(buttonBox!.x + buttonBox!.width).toBeLessThanOrEqual(toolbarBox!.x + toolbarBox!.width + 0.5);
    }
  }

  await expect(breadcrumb).toContainText("figure.png");
});
