import { test, expect } from "@playwright/test";
import path from "node:path";
import { installGardenCompositionMock, GARDEN_AGENT } from "../fixtures/gardenComposition";
import { surfacePanel } from "../fixtures/workbench";

for (const width of [1440, 800]) {
  test(`Garden expanded data notices reserve space at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await installGardenCompositionMock(page, { incompleteData: true });
    await page.goto("/");
    const garden = surfacePanel(page, "garden");
    await expect(garden.locator("canvas").first()).toBeVisible({ timeout: 15_000 });
    await expect(garden.locator(".garden-recovery-notice")).toBeVisible();
    await expect(garden.locator(".garden-data-notice")).toBeVisible();
    await garden.getByText("Map coverage", { exact: true }).click();
    await garden.locator(".garden-data-notice").getByText("Details", { exact: true }).click();
    const overlaps = await garden.evaluate((root) => {
      const scene = root.querySelector(".garden-scene")!.getBoundingClientRect();
      return [...root.querySelectorAll(".garden-navigation, .garden-coverage > div, .garden-data-notice, .garden-data-notice details, .garden-recovery-notice")]
        .filter((element) => element.getBoundingClientRect().bottom > scene.top).map((element) => element.className);
    });
    expect(overlaps).toEqual([]);
    await expect(garden.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
    await expect(garden.getByRole("button", { name: "Load more definitions", exact: true })).toBeVisible();
  });

  test(`Garden chrome and labels at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await installGardenCompositionMock(page);
    await page.goto("/");
    const garden = surfacePanel(page, "garden");
    await expect(garden.locator("canvas").first()).toBeVisible({ timeout: 15_000 });
    const capture = async (level: string) => {
      const assertChrome = async () => {
        const collisions = await garden.evaluate((root) => {
          const scene = root.querySelector(".garden-scene")!.getBoundingClientRect();
          return [...root.querySelectorAll(".garden-navigation, .garden-footer, .garden-data-notice, .garden-recovery-notice")]
            .filter((element) => {
              const bounds = element.getBoundingClientRect();
              return bounds.width > 0 && bounds.height > 0 && bounds.left < scene.right && bounds.right > scene.left && bounds.top < scene.bottom && bounds.bottom > scene.top;
            }).map((element) => element.className);
        });
        expect(collisions).toEqual([]);
      };
      await assertChrome();
      await garden.getByLabel("Garden help", { exact: true }).click();
      await expect(garden.locator(".garden-help")).toHaveAttribute("open", "");
      await assertChrome();
      await expect(garden.locator(".garden-help")).toContainText("Escape");
      await garden.getByLabel("Garden help", { exact: true }).click();
      await garden.screenshot({ path: path.resolve("e2e/screenshots/garden-cleanup", process.env.WARDIAN_GARDEN_STAMP ?? "audit", `${width}-${level}.png`), animations: "disabled" });
    };
    await capture("overview");
    const target = garden.locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`);
    await expect(target).toBeAttached();
    const point = await target.boundingBox();
    if (!point) throw new Error("Missing agent target");
    const initialZoom = await garden.getByTestId("garden-zoom-level").textContent();
    await page.mouse.move(point.x + point.width / 2, point.y + point.height / 2);
    await page.mouse.wheel(0, -600);
    await expect(garden.getByTestId("garden-zoom-level")).not.toHaveText(initialZoom!);
    await capture("district");
    await target.press("Enter");
    await expect(garden.getByRole("region", { name: "Memory", exact: true })).toBeVisible();
    await capture("object");
  });
}
