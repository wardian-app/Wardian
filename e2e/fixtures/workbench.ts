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

/** Open a core surface from the active group's pane-local launcher. */
export async function openSurface(
  page: Page,
  surfaceType: CoreWorkbenchSurfaceType,
  resourceKey?: string,
): Promise<void> {
  const group = activeWorkbenchGroup(page);
  await group.getByLabel("Open Surface", { exact: true }).click();

  const homeDialog = page.getByRole("dialog", { name: "Choose a surface", exact: true });
  const dialog = page.getByRole("dialog", { name: "Open Surface", exact: true });
  await homeDialog.or(dialog).waitFor({ state: "visible" });
  if (await homeDialog.isVisible()) {
    await homeDialog.getByRole("button", { name: "Browse all surfaces", exact: true }).click();
  }
  await dialog.waitFor({ state: "visible" });
  const option = dialog
    .getByRole("option")
    .and(page.locator(dataAttribute("surface-type", surfaceType)));
  await option.click();
  await surfaceTab(page, surfaceType, resourceKey).first().waitFor({ state: "visible" });
}
