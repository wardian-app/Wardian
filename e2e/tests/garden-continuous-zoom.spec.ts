import { test, expect, type Page, type Locator } from "@playwright/test";
import { installGardenCompositionMock, GARDEN_AGENT } from "../fixtures/gardenComposition";
import { surfacePanel } from "../fixtures/workbench";

const garden = (page: Page) => surfacePanel(page, "garden");
const agentCell = (page: Page) => garden(page).locator(`[data-garden-cell="agent:${GARDEN_AGENT}"]`);
const breadcrumb = (page: Page) => garden(page).getByRole("navigation", { name: "Garden breadcrumb" });
async function box(locator: Locator) {
  const value = await locator.boundingBox();
  if (!value) throw new Error("World cell has no screen bounds");
  return value;
}
async function wheel(page: Page, cell: Locator, delta: number) {
  const before = await box(cell);
  const viewport = await box(garden(page).locator(".garden-canvas"));
  const x = Math.max(viewport.x + 20, Math.min(viewport.x + viewport.width - 20, before.x + before.width / 2));
  const y = Math.max(viewport.y + 100, Math.min(viewport.y + viewport.height - 100, before.y + before.height / 2));
  await page.mouse.move(x, y);
  await page.mouse.wheel(0, delta);
  await expect.poll(async () => (await box(cell)).width).not.toBe(before.width);
  return { before, after: await box(cell), x, y };
}

test.describe("Garden continuous world zoom", () => {
  let bridge: Awaited<ReturnType<typeof installGardenCompositionMock>>;
  test.beforeEach(async ({ page }) => {
    bridge = await installGardenCompositionMock(page);
    await page.goto("/");
    await expect(garden(page).locator("canvas").first()).toBeVisible();
    await expect(garden(page).locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`)).toBeAttached();
  });

  test("unselected agent crosses detail bands with fixed world bounds and reversible pixel growth", async ({ page }, testInfo) => {
    const cell = agentCell(page);
    // Start from the canvas signal and reach the coarse DOM shell using only wheel.
    const target = garden(page).locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`);
    const signal = await box(target);
    await page.mouse.move(signal.x + signal.width / 2, signal.y + signal.height / 2);
    for (let step = 0; step < 100 && !(await cell.count()); step++) {
      await page.mouse.wheel(0, -120);
      await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    }
    await expect(cell).toBeAttached();
    expect(await bridge.calls("memory_list")).toHaveLength(0);
    expect(await bridge.calls("list_conversations")).toHaveLength(0);
    const world = await cell.getAttribute("data-garden-world");
    const start = await box(cell);
    const initialZoom = await garden(page).getByTestId("garden-zoom-level").textContent();
    const samples: { width: number; detail: number }[] = [];
    let steps = 0;
    while ((await box(cell)).width < 800 && steps < 100) {
      await expect(garden(page).locator('[data-garden-object][aria-pressed="true"]')).toHaveCount(0);
      const { before, after, x, y } = await wheel(page, cell, -120);
      await expect(cell).toHaveAttribute("data-garden-world", world!);
      // A notch is a small magnification about the pointer, including the entry boundary.
      expect(after.width / before.width).toBeCloseTo(1.05, 2);
      expect(after.x).toBeCloseTo(x + (before.x - x) * 1.05, 0);
      expect(after.y).toBeCloseTo(y + (before.y - y) * 1.05, 0);
      samples.push({ width: after.width, detail: Number(await cell.getAttribute("data-garden-detail")) });
      steps++;
    }
    expect(samples.some((sample) => sample.detail > 0 && sample.detail < 1)).toBe(true);
    await expect(breadcrumb(page)).toContainText("Moss Designer");
    await expect(garden(page).getByRole("region", { name: "Memory", exact: true })).toBeVisible();
    await expect(garden(page).getByTestId("garden-zoom-level")).not.toHaveText(initialZoom!);
    for (let index = 0; index < steps; index++) {
      const { before, after } = await wheel(page, cell, 120);
      expect(after.width / before.width).toBeCloseTo(1 / 1.05, 2);
      await expect(cell).toHaveAttribute("data-garden-world", world!);
    }
    expect((await box(cell)).width).toBeCloseTo(start.width, 0);
    await expect(breadcrumb(page)).not.toContainText("Moss Designer");
    await testInfo.attach("continuous-agent-bands", { body: JSON.stringify(samples), contentType: "application/json" });
  });

  test("selected memory reading plane grows around its world anchor and reverse wheel restores agent", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await garden(page).locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`).press("Enter");
    const memory = garden(page).getByRole("button", { name: /Keep the five agent regions/ });
    await memory.click();
    expect(await bridge.calls("memory_get")).toHaveLength(0);
    const cell = garden(page).locator('[data-garden-cell^="memory:"]');
    await expect(cell).toBeAttached();
    const seed = await box(memory.locator(".garden-object-mark-memory"));
    const initialPlane = await box(cell);
    expect(initialPlane.width).toBeCloseTo(seed.width, 0);
    expect(initialPlane.x + initialPlane.width / 2).toBeCloseTo(seed.x + seed.width / 2, 0);
    expect(initialPlane.y + initialPlane.height / 2).toBeCloseTo(seed.y + seed.height / 2, 0);
    const world = await cell.getAttribute("data-garden-world");
    const initialAspect = (await box(cell)).height / (await box(cell)).width;
    const aspects: number[] = [];
    let steps = 0;
    while ((await box(cell)).width < 750 && steps < 100) {
      const { before, after, x, y } = await wheel(page, cell, -120);
      expect(after.width / before.width).toBeCloseTo(1.05, 2);
      expect(after.x).toBeCloseTo(x + (before.x - x) * 1.05, 0);
      // The reading plane expands vertically about the source anchor, not its top edge.
      expect(after.y + after.height / 2).toBeCloseTo(y + (before.y + before.height / 2 - y) * 1.05, 0);
      expect(after.height).toBeGreaterThan(before.height);
      expect(after.height - before.height).toBeLessThan(after.width * .12);
      if (after.width >= 420) await expect(cell.locator(":scope > .garden-spatial-caption")).toHaveCSS("opacity", "0");
      aspects.push(after.height / after.width);
      await expect(cell).toHaveAttribute("data-garden-world", world!);
      steps++;
    }
    const morphProgress = aspects.map((aspect) => (aspect - initialAspect) / (.78 - initialAspect));
    expect(morphProgress.filter((progress) => progress > .1 && progress < .9).length).toBeGreaterThan(1);
    expect(aspects.at(-1)).toBeCloseTo(.78, 2);
    await expect(garden(page).getByRole("article", { name: "memory record" })).toContainText("conversation-design:turn:4");
    // Selection must not keep an enlarged parent caption visible behind its record.
    await expect(garden(page).locator('[data-garden-cell^="agent:"] [data-garden-ref^="memory:"][aria-pressed="true"] strong')).toHaveCSS("opacity", "0");
    await expect(garden(page).locator('[data-garden-cell^="agent:"] [data-garden-ref^="memory:"][aria-pressed="true"]')).toHaveCSS("opacity", "0");
    await expect(garden(page).getByTestId("garden-selection-summary").getByRole("button", { name: "Open record", exact: true })).toHaveCount(0);
    // The disappearing child cannot be measured after the final reverse notch.
    for (let index = 0; index < 100 && await cell.count(); index++) {
      const bounds = await box(cell);
      await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
      await page.mouse.wheel(0, 120);
      await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    }
    await expect(cell).toHaveCount(0);
    await expect(breadcrumb(page).getByRole("button").last()).toHaveText("Moss Designer");
    await expect(garden(page).getByRole("region", { name: "Memory", exact: true })).toBeVisible();
  });
});

test("dense memory objects retain individual identity and scroll inside their organelle", async ({ page }) => {
  await installGardenCompositionMock(page, { memoryCount: 34 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await garden(page).locator(`[data-garden-object="agent:${GARDEN_AGENT}"]`).press("Enter");
  const cell = agentCell(page);
  const region = cell.getByRole("region", { name: "Memory", exact: true });
  await expect(region.locator('[data-garden-ref^="memory:"]')).toHaveCount(34);
  const before = await box(cell);
  const last = region.locator('[data-garden-ref="memory:dense-memory-33"]');
  await last.scrollIntoViewIfNeeded();
  await last.click();
  await expect(last).toHaveAttribute("aria-pressed", "true");
  expect((await box(cell)).x).toBeCloseTo(before.x, 0);
  expect((await box(cell)).width).toBeCloseTo(before.width, 0);
  const first = region.getByRole("button", { name: /Keep the five agent regions/ });
  await first.scrollIntoViewIfNeeded();
  await page.screenshot({ path: "e2e/screenshots/garden/2026-09-08-unfolding/dense-memory-objects.png" });
  await first.press("Enter");
  await expect(garden(page).getByRole("article", { name: "memory record" })).toContainText("conversation-design:turn:4");
});
