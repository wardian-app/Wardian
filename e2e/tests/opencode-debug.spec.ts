import { test, expect } from "@playwright/test";

const runRealOpenCode = process.env.WARDIAN_E2E_REAL_OPENCODE === "1";
const workspacePath = process.env.WARDIAN_E2E_REAL_WORKSPACE || process.cwd();

test.describe("OpenCode Debug Provider", () => {
  test.skip(!runRealOpenCode, "Set WARDIAN_E2E_REAL_OPENCODE=1 to run real OpenCode E2E.");

  test("spawns an OpenCode agent in tauri dev", async ({ page }) => {
    let dialogMessage: string | null = null;
    page.on("dialog", async (dialog) => {
      dialogMessage = dialog.message();
      await dialog.dismiss();
    });

    await page.goto("/", { waitUntil: "domcontentloaded" });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });

    const hasTauriBridge = await page.evaluate(() => {
      const windowWithTauri = window as typeof window & {
        __TAURI__?: unknown;
        __TAURI_INTERNALS__?: unknown;
      };
      return (
        typeof windowWithTauri.__TAURI__ !== "undefined" ||
        typeof windowWithTauri.__TAURI_INTERNALS__ !== "undefined"
      );
    });
    test.skip(
      !hasTauriBridge,
      "This Playwright harness is running against the browser dev server, not a native Tauri webview."
    );

    await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
    await page.locator('[data-testid="spawn-agent-name"]').fill("E2E OpenCode");
    await page.locator('[data-testid="spawn-workspace-path"]').fill(workspacePath);
    await page.locator('[data-testid="spawn-provider"]').selectOption("opencode");

    await page.locator('[data-testid="spawn-submit"]').click();

    await expect(page.locator('[data-testid="agent-card"]')).toHaveCount(1, {
      timeout: 60_000,
    });
    await expect(page.locator("text=E2E OpenCode")).toBeVisible({ timeout: 30_000 });
    await expect
      .poll(() => dialogMessage, {
        timeout: 5_000,
        message: "spawn_agent should not raise an alert dialog",
      })
      .toBeNull();
  });
});
