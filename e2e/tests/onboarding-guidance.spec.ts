import { expect, test } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

import {
  installWorkbenchIpcMock,
  makeWorkbenchDocument,
  makeWorkbenchSurface,
} from "../fixtures/workbenchIpcMock";

test("shows optional onboarding controls and guided tour in Settings", async ({ page }, testInfo) => {
  const overview = makeWorkbenchSurface("onboarding-overview", "agents-overview", {
    state: {
      mode: "grid",
      focused_agent_id: null,
      search_query: "",
      status_filter: [],
    },
  });
  const document = makeWorkbenchDocument({ revision: 1, surfaces: [overview] });

  await page.setViewportSize({ width: 1440, height: 960 });
  await installWorkbenchIpcMock(page, {
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: document.revision,
      durable_token: "onboarding-evidence-token",
    },
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await expect(page.getByTestId("app-shell")).toBeVisible();

  await page.getByTestId("sidebar-tab-settings").click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Contextual tips")).toHaveValue("show");
  await expect(dialog.getByRole("button", { name: "Reset dismissed tips" })).toBeVisible();

  await dialog.getByRole("button", { name: "Open guided tour" }).click();
  const tour = dialog.getByTestId("onboarding-tour");
  await expect(tour).toBeVisible();
  await expect(tour.getByRole("heading", { name: "Start with a reliable agent" })).toBeVisible();

  const screenshotPath = process.env.WARDIAN_ONBOARDING_SCREENSHOT
    ?? testInfo.outputPath("settings-guided-tour.png");
  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  await dialog.screenshot({ path: screenshotPath, animations: "disabled" });
  await testInfo.attach("onboarding-guidance", { path: screenshotPath, contentType: "image/png" });
});
