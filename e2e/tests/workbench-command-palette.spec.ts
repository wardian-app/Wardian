import { expect, test } from "@playwright/test";

import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
} from "../fixtures/workbenchIpcMock";
import { surfaceTab } from "../fixtures/workbench";

test("keeps workbench commands in a searchable palette separate from Quick Open", async ({
  page,
}, testInfo) => {
  const dashboard = makeWorkbenchSurface("palette-dashboard", "dashboard");
  const document = makeWorkbenchDocument({ surfaces: [dashboard] });
  await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: `palette-token-${document.revision}`,
    },
  });

  await page.goto("/");
  await expect(page.getByTestId("workbench-host")).toBeVisible();

  const titlebarCenter = page.getByTestId("titlebar-center");
  await expect(titlebarCenter).toHaveAttribute("data-tauri-drag-region", "true");
  await expect(titlebarCenter.getByRole("button")).toHaveCount(0);
  await expect(page.getByRole("group", { name: "Workbench commands", exact: true }))
    .toHaveCount(0);

  const dashboardTab = surfaceTab(page, "dashboard");
  await dashboardTab.focus();
  await page.keyboard.press("ControlOrMeta+Shift+P");

  const palette = page.getByRole("dialog", { name: "Command Palette", exact: true });
  await expect(palette).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Open Surface", exact: true })).toHaveCount(0);
  const search = palette.getByRole("combobox", { name: "Search commands", exact: true });
  await expect(search).toBeFocused();
  await search.fill("Split Right");
  await expect(palette.getByRole("option", { name: /^Split Right/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  const screenshotPath = testInfo.outputPath("command-palette.png");
  await palette.screenshot({ path: screenshotPath });
  await testInfo.attach("command-palette", {
    path: screenshotPath,
    contentType: "image/png",
  });

  await page.keyboard.press("Enter");
  await expect(palette).toHaveCount(0);
  await expect(page.getByTestId("workbench-group")).toHaveCount(2);

  await dashboardTab.focus();
  await page.keyboard.press("ControlOrMeta+P");
  await expect(page.getByRole("dialog", { name: "Open Surface", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Command Palette", exact: true })).toHaveCount(0);
});
