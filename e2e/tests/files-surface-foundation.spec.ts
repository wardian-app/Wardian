import { expect, test, type Locator, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import path from "node:path";

import type { AppSettings } from "../../src/types/settings";
import type { ArtifactResourceV1, WorkbenchDocumentV1 } from "../../src/types";
import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
  type WorkbenchIpcMockOptions,
  type WorkbenchIpcMockController,
} from "../fixtures/workbenchIpcMock";

const ROOT = "/workspace/project";
const ALPHA_PATH = `${ROOT}/alpha.md`;
const BETA_PATH = `${ROOT}/beta.md`;
const ALPHA_COPY_PATH = `${ROOT}/alpha-copy.md`;
const WIDE_PDF_PATH = `${ROOT}/wide.pdf`;
const IMAGE_PATH = `${ROOT}/figure.png`;
const MANY_CHANGES_PATH = `${ROOT}/many-changes.txt`;

const MANY_CHANGES_BASE = Array.from(
  { length: 25 },
  (_, index) => `Stable line ${index + 1}`,
).join("\n");
const MANY_CHANGES_EDITED = Array.from(
  { length: 25 },
  (_, index) => index % 2 === 0
    ? `Stable line ${index + 1}`
    : `Changed line ${index + 1}`,
).join("\n");

const ONE_PIXEL_PNG = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const MARKDOWN_FIGURE = "/icon.png";

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

async function bootFilesWorkbench(
  page: Page,
  options: Pick<WorkbenchIpcMockOptions, "explorer_root" | "files" | "responses"> & {
    document?: WorkbenchDocumentV1;
  } = {},
): Promise<WorkbenchIpcMockController> {
  const dashboard = makeWorkbenchSurface("dashboard-1", "dashboard");
  const document = options.document ?? makeWorkbenchDocument({ surfaces: [dashboard] });
  const explorerRoot = options.explorer_root ?? ROOT;
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
    explorer_root: explorerRoot,
    save_target_path: ALPHA_COPY_PATH,
    files: options.files ?? [
      { path: ALPHA_PATH, content: "# Alpha document\n\nFirst file." },
      { path: BETA_PATH, content: "# Beta document\n\nSecond file." },
      { path: MANY_CHANGES_PATH, content: MANY_CHANGES_BASE },
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
      folder: explorerRoot,
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
      ...options.responses,
    },
  });
  await page.goto("/");
  await expect(page.getByTestId("workbench-host")).toBeVisible();
  await page.getByTestId("sidebar-tab-explorer").click();
  await expect(page.getByRole("tree", { name: "Workspace files" })).toBeVisible();
  return ipc;
}

test("keeps artifact details limited to actionable provenance and history", async ({ page }) => {
  const artifactPath = `${ROOT}/Wardian-README.md`;
  const surface = makeWorkbenchSurface("artifact-surface", "files", {
    resource_key: "artifact:artifact-1",
    state_schema_version: 2,
    state: {
      resource_kind: "artifact",
      transient_preview: false,
      presentation: "rendered",
      comparison_open: false,
      comparison_layout_preference: "auto",
      comparison_baseline: null,
      review_drawer_open: false,
      selected_version_id: "version-2",
      optional_checkpoint_id: null,
    },
  });
  const document = makeWorkbenchDocument({ surfaces: [surface] });
  const artifact: ArtifactResourceV1 = {
    schema: 1,
    manifest: {
      schema: 1,
      artifact_id: "artifact-1",
      canonical_path: artifactPath,
      title: "Wardian README",
      description: null,
      origin: {
        session_id: "agent-files",
        agent_id: "agent-files",
        agent_name: "Wardian-Arch",
        provider: "codex",
      },
      status: "presented",
      active: true,
      created_at_ms: Date.UTC(2026, 6, 18, 20, 2, 3),
      updated_at_ms: Date.UTC(2026, 6, 18, 20, 2, 3),
      versions: [
        {
          version_id: "version-1",
          sequence: 1,
          content_hash: "sha256:version-one",
          size_bytes: 12,
          presented_at_ms: Date.UTC(2026, 6, 18, 19, 2, 3),
          addressed_comment_ids: [],
        },
        {
          version_id: "version-2",
          sequence: 2,
          content_hash: "sha256:version-two",
          size_bytes: 12,
          presented_at_ms: Date.UTC(2026, 6, 18, 20, 2, 3),
          addressed_comment_ids: [],
        },
      ],
      latest_review_id: null,
    },
    selected_version: {
      version_id: "version-2",
      sequence: 2,
      content_hash: "sha256:version-two",
      size_bytes: 12,
      presented_at_ms: Date.UTC(2026, 6, 18, 20, 2, 3),
      addressed_comment_ids: [],
    },
    selected_text: "# Wardian\n",
    working: {
      canonical_path: artifactPath,
      agent_id: "agent-files",
      content_hash: "sha256:working-copy",
      unavailable_reason: null,
    },
    attention: false,
  };

  await bootFilesWorkbench(page, {
    document,
    files: [{ path: artifactPath, content: "# Wardian\n\nWorking copy." }],
    responses: { get_artifact_resource: artifact },
  });

  const details = page.getByRole("complementary", { name: "Artifact details" });
  await expect(details.locator(".artifact-details-primary")).toHaveText(
    /^\s*Presented by Wardian-Arch\s*Changed since presented\s*$/,
  );
  await expect(details.getByRole("combobox", { name: "Artifact version" })).toHaveValue("version-2");
  await expect(details.getByRole("option", { name: "2 / 2" })).toBeAttached();

  const screenshotPath = path.join(
    "e2e",
    "screenshots",
    "files-artifact-details",
    "2026-07-19",
    "compact-artifact-details.png",
  );
  await details.screenshot({ path: screenshotPath });
});

async function persistedDocument(ipc: WorkbenchIpcMockController) {
  return (await ipc.snapshot()).load_result.document;
}

async function replaceMonacoText(page: Page, surface: Locator, text: string) {
  const editorLines = surface.locator(".monaco-editor .view-lines");
  const editorInput = surface.locator(".monaco-editor .native-edit-context");
  await editorLines.click({ position: { x: 100, y: 8 } });
  await expect(editorInput).toBeFocused();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.insertText(text);
}

async function visibleMonacoText(surface: Locator) {
  return (await surface.locator(".monaco-editor .view-line").evaluateAll((lines) => (
    lines.map((line) => line.textContent ?? "").join("\n")
  ))).replace(/\u00a0/g, " ").replace(/\u00b7/g, "");
}

async function closeTabFromContextMenu(page: Page, tab: Locator) {
  await tab.click({ button: "right", position: { x: 16, y: 12 } });
  await page.getByRole("menuitem", { name: "Close tab" }).click();
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

test("renders Windows Explorer and Files paths with stable compact separators", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const windowsRoot = "//?/C:/work/project";
  const windowsFile = `${windowsRoot}/alpha.md`;
  await bootFilesWorkbench(page, {
    explorer_root: windowsRoot,
    files: [{ path: windowsFile, content: "# Windows path\n\nStable display." }],
  });

  const explorerRoot = page.getByTestId("explorer-panel").locator("span[title]").first();
  await expect(explorerRoot).toHaveAttribute("title", "C:\\work\\project");
  await expect(explorerRoot).toHaveText("C:\\work\\project");

  await page.getByRole("treeitem", { name: "alpha.md" }).dblclick();
  const breadcrumb = page.getByRole("navigation", { name: "File location" });
  await expect(breadcrumb).toHaveAttribute("title", "C:\\work\\project\\alpha.md");
  await expect(breadcrumb).toHaveText("C:\\work\\project\\alpha.md");
  await expect(breadcrumb.locator(".files-breadcrumb-path")).toHaveCSS("user-select", "text");
  await expect(page.getByRole("heading", { name: "Windows path" })).toBeVisible();

  await page.screenshot({
    path: path.resolve(
      "e2e/screenshots/files-surface/2026-07-18/windows-path-chrome.png",
    ),
    fullPage: true,
  });
});

test("renders a complete Markdown document without flashing during revision refresh", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1200, height: 900 });
  const markdown = [
    "<div align=\"center\">",
    "",
    "# Habitat report",
    "",
    '<img src="figure.png" width="128" alt="Wardian figure" />',
    "",
    "</div>",
    "",
    "> A visible blockquote.",
    "",
    "- [x] Render headings",
    "- [ ] Keep iterating",
    "",
    "| Surface | State |",
    "| --- | --- |",
    "| Files | Active |",
    "",
    "<details open><summary>Details</summary>Sanitized HTML content.</details>",
  ].join("\n");
  const ipc = await bootFilesWorkbench(page, {
    files: [
      { path: ALPHA_PATH, content: markdown },
      {
        path: IMAGE_PATH,
        content: "image fixture",
        mime_type: "image/png",
        renderer_kind: "image",
        stream_url: MARKDOWN_FIGURE,
      },
    ],
  });
  await page.getByRole("treeitem", { name: "alpha.md" }).dblclick();

  const surface = page.getByTestId("files-surface");
  const title = surface.getByRole("heading", { level: 1, name: "Habitat report" });
  await expect(title).toBeVisible();
  const figure = surface.getByRole("img", { name: "Wardian figure" });
  await expect(figure).toBeVisible();
  await expect(figure).toHaveAttribute("src", MARKDOWN_FIGURE);
  await expect.poll(async () => figure.evaluate((image) => (image as HTMLImageElement).naturalWidth))
    .toBeGreaterThan(0);
  await title.evaluate((element) => { element.setAttribute("data-render-identity", "heading"); });
  await figure.evaluate((element) => { element.setAttribute("data-render-identity", "image"); });
  await expect(surface.getByRole("table")).toContainText("Files");
  await expect(surface.getByText("Sanitized HTML content.")).toBeVisible();
  const titleStyle = await title.evaluate((element) => {
    const style = getComputedStyle(element);
    return { fontSize: Number.parseFloat(style.fontSize), fontWeight: Number(style.fontWeight) };
  });
  expect(titleStyle.fontSize).toBeGreaterThan(24);
  expect(titleStyle.fontWeight).toBeGreaterThanOrEqual(600);

  await surface.evaluate((element) => {
    (window as Window & { __markdownSawLoading?: boolean }).__markdownSawLoading = false;
    const observer = new MutationObserver(() => {
      if (element.textContent?.includes("Loading Markdown")) {
        (window as Window & { __markdownSawLoading?: boolean }).__markdownSawLoading = true;
      }
      if (element.querySelector(".files-markdown-image-loading")) {
        (window as Window & { __markdownSawLoading?: boolean }).__markdownSawLoading = true;
      }
    });
    observer.observe(element, { childList: true, subtree: true, characterData: true });
    (element as HTMLElement & { __markdownObserver?: MutationObserver }).__markdownObserver = observer;
  });
  await ipc.updateFile(ALPHA_PATH, markdown.replace("Habitat report", "Habitat report updated"));
  const updatedTitle = surface.getByRole("heading", { name: "Habitat report updated" });
  await expect(updatedTitle).toBeVisible();
  await expect(updatedTitle).toHaveAttribute("data-render-identity", "heading");
  await expect(surface.getByRole("img", { name: "Wardian figure" }))
    .toHaveAttribute("data-render-identity", "image");
  const sawLoading = await surface.evaluate((element) => {
    (element as HTMLElement & { __markdownObserver?: MutationObserver }).__markdownObserver?.disconnect();
    return (window as Window & { __markdownSawLoading?: boolean }).__markdownSawLoading;
  });
  expect(sawLoading).toBe(false);

  await surface.screenshot({
    path: path.resolve(
      "e2e/screenshots/files-surface/2026-07-18/markdown-document-renderer.png",
    ),
  });
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
  const savedText = "# Alpha edited\n\nSaved from Monaco.";
  await replaceMonacoText(page, activeSurface, savedText);

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
  await replaceMonacoText(page, activeSurface, copiedText);
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

test("shares one Monaco buffer across panes and guards only the final dirty close", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  const ipc = await bootFilesWorkbench(page);
  const alphaRow = page.getByRole("treeitem", { name: "alpha.md" });

  await alphaRow.dblclick();
  await page.getByRole("button", { name: "Edit source" }).click();
  await alphaRow.click({ button: "right" });
  await page.getByRole("button", { name: "Open to Side", exact: true }).click();

  const surfaces = page.getByTestId("files-surface");
  await expect(surfaces).toHaveCount(2);
  for (let index = 0; index < 2; index += 1) {
    const editSource = surfaces.nth(index).getByRole("button", { name: "Edit source" });
    if (await editSource.count()) await editSource.click();
    await expect(surfaces.nth(index).getByTestId("monaco-text-renderer")).toBeVisible();
  }

  const sharedText = "# Shared buffer acceptance\n\nBoth panes see these exact bytes.";
  await replaceMonacoText(page, surfaces.nth(0), sharedText);
  await expect.poll(() => visibleMonacoText(surfaces.nth(0))).toContain("Shared buffer acceptance");
  await expect.poll(() => visibleMonacoText(surfaces.nth(1))).toContain("Both panes see these exact bytes.");
  const firstPaneText = await visibleMonacoText(surfaces.nth(0));
  await expect.poll(() => visibleMonacoText(surfaces.nth(1))).toBe(firstPaneText);
  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]'))
    .toHaveCount(2);
  await expect(surfaces.nth(0).getByLabel("Unsaved changes")).toBeVisible();
  await expect(surfaces.nth(1).getByLabel("Unsaved changes")).toBeVisible();
  await expect.poll(async () => (await ipc.calls("checkpoint_file_recovery")).length)
    .toBeGreaterThan(0);

  await closeTabFromContextMenu(page, filesTab(page, ALPHA_PATH).first());
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(1);
  await expect(page.getByRole("dialog", { name: /Unsaved Files changes/ })).toHaveCount(0);
  await expect(filesTab(page, ALPHA_PATH).locator('[data-surface-badge="dirty"]')).toBeVisible();

  await closeTabFromContextMenu(page, filesTab(page, ALPHA_PATH));
  const closePrompt = page.getByRole("dialog", { name: /Unsaved Files changes/ });
  await expect(closePrompt).toBeVisible();
  await closePrompt.getByRole("button", { name: "Cancel" }).click();
  await expect(closePrompt).toHaveCount(0);
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(1);

  const saveCountBeforeClose = (await ipc.calls("save_file_resource_text")).length;
  await closeTabFromContextMenu(page, filesTab(page, ALPHA_PATH));
  await closePrompt.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(async () => (await ipc.calls("save_file_resource_text")).length)
    .toBeGreaterThan(saveCountBeforeClose);
  const finalCloseSave = (await ipc.calls("save_file_resource_text")).at(-1)?.args?.request as {
    text?: string;
  } | undefined;
  expect(finalCloseSave?.text).toBe(sharedText);
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(0);

  await alphaRow.dblclick();
  await page.getByRole("button", { name: "Edit source" }).click();
  const reopenedSurface = page.getByTestId("files-surface");
  const checkpointCountBeforeDiscard = (await ipc.calls("checkpoint_file_recovery")).length;
  await replaceMonacoText(page, reopenedSurface, "# Discard this generation\n");
  await expect.poll(async () => (await ipc.calls("checkpoint_file_recovery")).length)
    .toBeGreaterThan(checkpointCountBeforeDiscard);

  const discardCountBeforeClose = (await ipc.calls("discard_file_recovery")).length;
  await closeTabFromContextMenu(page, filesTab(page, ALPHA_PATH));
  await closePrompt.getByRole("button", { name: "Don't Save" }).click();
  await expect.poll(async () => (await ipc.calls("discard_file_recovery")).length)
    .toBeGreaterThan(discardCountBeforeClose);
  await expect(filesTab(page, ALPHA_PATH)).toHaveCount(0);
});

test("keeps inline saved-file changes and responsive comparison independent of presentation", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1800, height: 1000 });
  await bootFilesWorkbench(page);
  await page.getByRole("treeitem", { name: "alpha.md" }).dblclick();
  await page.getByRole("button", { name: "Edit source" }).click();

  const surface = page.getByTestId("files-surface");
  const host = surface.getByTestId("files-content-host-shell");
  await host.evaluate((element) => {
    element.dataset.acceptanceHost = "preserved";
  });
  await replaceMonacoText(
    page,
    surface,
    "# Alpha revised\n\nFirst file.\n\nA new saved-file annotation.",
  );

  await expect.poll(async () => surface.locator(".files-diff-modified-line").count())
    .toBeGreaterThan(0);
  await expect.poll(async () => surface.locator(".files-diff-added-line").count())
    .toBeGreaterThan(0);
  const diffControl = surface.getByRole("button", { name: /Open comparison:/ });
  await expect(diffControl).toBeVisible();
  await expect(diffControl).toHaveAttribute("aria-pressed", "false");
  await expect(surface.getByRole("group", { name: "Saved file changes" })).toBeVisible();

  await surface.getByRole("button", { name: "View rendered" }).click();
  await expect(surface.getByRole("heading", { name: "Alpha revised" })).toBeVisible();
  await expect(surface.getByRole("button", { name: "Edit source" }))
    .toHaveAttribute("aria-pressed", "false");

  await diffControl.click();
  const lens = surface.getByRole("region", { name: "File comparison" });
  const comparisonBody = lens.locator(".files-comparison-body");
  await expect(lens).toBeVisible();
  await expect(host).toHaveAttribute("data-comparison-open", "true");
  await expect(host).toHaveAttribute("data-acceptance-host", "preserved");
  await expect(surface.getByRole("button", { name: "View rendered" }))
    .toHaveAttribute("aria-pressed", "true");

  await surface.evaluate((element) => {
    element.style.width = "900px";
    element.style.maxWidth = "900px";
    element.style.flex = "0 0 900px";
  });
  await expect(comparisonBody).toHaveAttribute("data-layout", "side_by_side");
  const monacoDiff = lens.locator(".monaco-diff-editor");
  await expect(monacoDiff).toHaveClass(/side-by-side/);
  await monacoDiff.evaluate((element) => {
    element.setAttribute("data-diff-identity", "preserved");
  });

  const screenshotPath = path.resolve(
    "e2e/screenshots/files-editor/2026-07-18/saved-file-comparison-lens.png",
  );
  await surface.screenshot({ path: screenshotPath });

  await surface.evaluate((element) => {
    element.style.width = "650px";
    element.style.maxWidth = "650px";
    element.style.flex = "0 0 650px";
  });
  await expect(comparisonBody).toHaveAttribute("data-layout", "unified");
  await expect(monacoDiff).not.toHaveClass(/side-by-side/);

  const layout = lens.getByRole("combobox", { name: "Comparison layout" });
  await layout.selectOption("side_by_side");
  await expect(layout).toHaveValue("side_by_side");
  await expect(comparisonBody).toHaveAttribute("data-layout", "side_by_side");
  await expect(monacoDiff).toHaveClass(/side-by-side/);
  await expect(monacoDiff).toHaveAttribute("data-diff-identity", "preserved");

  await surface.evaluate((element) => {
    element.style.width = "520px";
    element.style.maxWidth = "520px";
    element.style.flex = "0 0 520px";
  });
  await expect(comparisonBody).toHaveAttribute("data-layout", "unified");
  await expect(monacoDiff).not.toHaveClass(/side-by-side/);
  await expect(layout).toHaveValue("side_by_side");
  await expect(layout).toHaveAttribute("title", /needs at least 560 px/i);

  await lens.getByRole("button", { name: "Close comparison" }).click();
  await expect(lens).toHaveCount(0);
  await expect(host).toHaveAttribute("data-comparison-open", "false");
  await expect(host).toHaveAttribute("data-acceptance-host", "preserved");
  await expect(surface.getByRole("button", { name: "View rendered" }))
    .toHaveAttribute("aria-pressed", "true");
  await expect(surface.getByTestId("monaco-text-renderer")).toBeVisible();
});

test("contains a multi-digit saved-file diff control in a 100px pane", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await bootFilesWorkbench(page);
  await page.getByRole("treeitem", { name: "many-changes.txt" }).dblclick();

  const surface = page.getByTestId("files-surface");
  await expect(surface.getByTestId("monaco-text-renderer")).toBeVisible();
  await replaceMonacoText(page, surface, MANY_CHANGES_EDITED);

  const diffControl = surface.getByRole("button", {
    name: /Open comparison: 12 change regions, 0 added, 12 modified, 0 deleted against Saved file/i,
  });
  await expect(diffControl).toBeVisible();
  await expect(diffControl.locator(".files-diff-count")).toHaveText("12");

  await surface.evaluate((element) => {
    element.style.width = "100px";
    element.style.maxWidth = "100px";
    element.style.flex = "0 0 100px";
  });
  await expect.poll(async () => surface.evaluate((element) => element.clientWidth)).toBe(100);

  const surfaceBox = await surface.boundingBox();
  expect(surfaceBox).not.toBeNull();
  for (const control of [diffControl, surface.getByRole("button", { name: "File actions" })]) {
    const box = await control.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(surfaceBox!.x - 0.5);
    expect(box!.x + box!.width).toBeLessThanOrEqual(surfaceBox!.x + surfaceBox!.width + 0.5);
  }
  const headerMetrics = await surface.locator(".files-header").evaluate((element) => ({
    client_width: element.clientWidth,
    scroll_width: element.scrollWidth,
  }));
  expect(headerMetrics.scroll_width).toBeLessThanOrEqual(headerMetrics.client_width);
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
