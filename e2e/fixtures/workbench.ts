import type { Locator, Page } from "@playwright/test";

export type CoreWorkbenchSurfaceType =
  | "agents-overview"
  | "dashboard"
  | "queue"
  | "graph"
  | "garden"
  | "library"
  | "workflows"
  | "agent-session";

export type PointerTarget = { x: number; y: number };
export type ElementBounds = PointerTarget & { width: number; height: number };

function boundsAreEqual(first: ElementBounds, second: ElementBounds): boolean {
  return Math.abs(first.x - second.x) <= 0.05
    && Math.abs(first.y - second.y) <= 0.05
    && Math.abs(first.width - second.width) <= 0.05
    && Math.abs(first.height - second.height) <= 0.05;
}

/** Wait until every locator keeps the same bounding rect across animation frames. */
export async function waitForStableBoundingBoxes(
  page: Page,
  locators: readonly Locator[],
  consecutiveFrames = 4,
): Promise<ElementBounds[]> {
  await Promise.all(locators.map((locator) => locator.waitFor({ state: "visible" })));
  let previous: ElementBounds[] | null = null;
  let stableFrames = 0;
  for (let frame = 0; frame < 240; frame += 1) {
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
    const current = await Promise.all(locators.map((locator) => locator.boundingBox()));
    if (current.some((bounds) => bounds === null)) {
      previous = null;
      stableFrames = 0;
      continue;
    }
    const present = current as ElementBounds[];
    stableFrames = previous?.every((bounds, index) => boundsAreEqual(bounds, present[index]))
      ? stableFrames + 1
      : 0;
    previous = present;
    if (stableFrames >= consecutiveFrames) return present;
  }
  throw new Error(`Layout did not stabilize: ${JSON.stringify(previous)}`);
}

function dataAttribute(name: string, value: string): string {
  return `[data-${name}=${JSON.stringify(value)}]`;
}

function withSurfaceIdentity(
  page: Page,
  locator: Locator,
  surfaceType: CoreWorkbenchSurfaceType,
  resourceKey?: string,
): Locator {
  let identified = locator.and(page.locator(dataAttribute("surface-type", surfaceType)));
  if (resourceKey !== undefined) {
    identified = identified.and(page.locator(dataAttribute("resource-key", resourceKey)));
  }
  return identified;
}

/** The group that currently owns workbench keyboard and launcher actions. */
export function activeWorkbenchGroup(page: Page): Locator {
  return page
    .getByTestId("workbench-group")
    .and(page.locator(dataAttribute("active", "true")));
}

/** Every tab matching a semantic surface identity, across all workbench groups. */
export function surfaceTab(
  page: Page,
  surfaceType: CoreWorkbenchSurfaceType,
  resourceKey?: string,
): Locator {
  return withSurfaceIdentity(page, page.getByRole("tab"), surfaceType, resourceKey);
}

/** Every mounted panel matching a semantic surface identity. */
export function surfacePanel(
  page: Page,
  surfaceType: CoreWorkbenchSurfaceType,
  resourceKey?: string,
): Locator {
  return withSurfaceIdentity(
    page,
    page.getByTestId("surface-panel"),
    surfaceType,
    resourceKey,
  );
}

/** Drag a visible workbench tab with Playwright's real pointer coordinates. */
export async function dragSurfaceTab(
  page: Page,
  tab: Locator,
  target: PointerTarget,
  beforeDrop?: () => Promise<void>,
): Promise<void> {
  const [bounds] = await waitForStableBoundingBoxes(page, [tab]);
  const start = {
    x: bounds.x + bounds.width / 2,
    y: bounds.y + bounds.height / 2,
  };

  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  try {
    await page.mouse.move(start.x + 8, start.y, { steps: 4 });
    await page.mouse.move(target.x, target.y, { steps: 16 });
    await beforeDrop?.();
  } finally {
    await page.mouse.up();
  }
}

/** Open a core surface from the active group's pane-local launcher. */
export async function openSurface(
  page: Page,
  surfaceType: CoreWorkbenchSurfaceType,
  resourceKey?: string,
): Promise<void> {
  const group = activeWorkbenchGroup(page);
  await group.getByLabel("Open Surface", { exact: true }).click();

  const inlineHome = group.getByRole("heading", { name: "Choose a surface", exact: true });
  const dialog = page.getByRole("dialog", { name: "Open Surface", exact: true });
  await inlineHome.or(dialog).waitFor({ state: "visible" });
  if (await inlineHome.isVisible()) {
    await group.getByRole("button", { name: "Browse all surfaces", exact: true }).click();
  }
  await dialog.waitFor({ state: "visible" });
  const option = dialog
    .getByRole("option")
    .and(page.locator(dataAttribute("surface-type", surfaceType)));
  await option.click();
  await surfaceTab(page, surfaceType, resourceKey).first().waitFor({ state: "visible" });
}
