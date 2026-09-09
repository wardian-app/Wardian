import { test, expect, type Page } from "@playwright/test";
import path from "node:path";
import { openSurface, surfacePanel, surfaceTab } from "../fixtures/workbench";
import { installGardenCompositionMock, GARDEN_AGENT, GARDEN_ROOT, GARDEN_RUN } from "../fixtures/gardenComposition";

const stamp = process.env.WARDIAN_GARDEN_STAMP ?? new Date().toISOString().replace(/[:.]/g, "-");
const garden = (page: Page) => surfacePanel(page, "garden");
const object = (page: Page, key: string) => garden(page).locator(`[data-garden-object="${key}"]`);
async function capture(page: Page, name: string) {
  await expect(page.getByText("Saving workbench changes…", { exact: true })).toBeHidden();
  await garden(page).screenshot({ path: path.resolve("e2e/screenshots/garden", stamp, `${name}.png`), animations: "disabled" });
}
async function enterAgent(page: Page) {
  await object(page, `agent:${GARDEN_AGENT}`).press("Enter");
  await expect(garden(page).getByRole("region", { name: "Identity", exact: true })).toBeVisible();
}

test.describe("Garden semantic composition", () => {
  test.beforeEach(async ({ page }) => {
    await installGardenCompositionMock(page);
    await page.goto("/");
    await expect(garden(page).locator("canvas").first()).toBeVisible({ timeout: 15_000 });
    await expect(object(page, `agent:${GARDEN_AGENT}`)).toBeAttached();
  });

  test("single selection keeps camera; Enter opens five regions; memory evidence and return", async ({ page }, testInfo) => {
    const canvas = garden(page).locator(".garden-canvas");
    const agent = object(page, `agent:${GARDEN_AGENT}`);
    await agent.focus();
    const before = await canvas.getAttribute("data-garden-fit");
    const hit = await agent.boundingBox();
    if (!hit) throw new Error("Agent has no canvas hit target");
    await page.mouse.click(hit.x + hit.width / 2, hit.y + hit.height / 2);
    await expect(agent).toHaveAttribute("aria-pressed", "true");
    await expect(canvas).toHaveAttribute("data-garden-fit", before!);
    await expect(surfaceTab(page, "agent-session")).toHaveCount(0);
    await capture(page, "01-selected-habitat");
    await agent.press("Enter");
    for (const name of ["Identity", "Skills", "Memory", "Automations, Conversations, Inbox", "Workspace, Teams, Agents"]) {
      await expect(garden(page).getByRole("region", { name, exact: true })).toBeVisible();
    }
    for (const name of ["Identity", "Skills", "Memory", "Automations", "Workspace"]) {
      await expect(garden(page).getByRole("heading", { name, exact: true })).toBeInViewport();
    }
    // Transformed edges can produce 0.9999993 from IntersectionObserver even
    // when fully inside; allow subpixel rounding, not a clipped port.
    await expect(garden(page).getByRole("region", { name: "Workspace, Teams, Agents", exact: true }).getByRole("button", { name: /synthetic\/garden/ })).toBeInViewport({ ratio: .9999 });
    await expect(garden(page).getByTestId("garden-selection-summary").getByRole("button", { name: "Enter", exact: true })).toHaveCount(0);
    await expect(garden(page).getByRole("region", { name: "Automations, Conversations, Inbox", exact: true })).toContainText("Draft ready for evidence review.");
    await expect(garden(page).getByRole("region", { name: "Skills" })).toContainText("Interface Review");
    const memory = garden(page).getByRole("button", { name: /Keep the five agent regions/ });
    await expect(memory).toBeVisible();
    await capture(page, "02-agent-cutaway");
    const identityGeometry = await garden(page).getByRole("region", { name: "Identity", exact: true }).evaluate((region) => {
      const style = getComputedStyle(region);
      const heading = region.querySelector("h3")!;
      const range = document.createRange();
      range.selectNodeContents(heading);
      const text = range.getBoundingClientRect();
      const textPoints = [text.left + 1, (text.left + text.right) / 2, text.right - 1].map((x) => {
        const hit = document.elementFromPoint(x, (text.top + text.bottom) / 2);
        return { x, visible: hit === heading || heading.contains(hit) };
      });
      return { bounds: region.getBoundingClientRect().toJSON(), heading: text.toJSON(), textPoints,
        padding: style.padding, borderRadius: style.borderRadius, overflow: style.overflow };
    });
    await testInfo.attach("identity-computed-geometry", { body: JSON.stringify(identityGeometry, null, 2), contentType: "application/json" });
    console.info("Identity computed geometry:", JSON.stringify(identityGeometry));
    expect(identityGeometry.textPoints.every((point) => point.visible), JSON.stringify(identityGeometry)).toBe(true);
    await memory.click();
    await expect(garden(page).getByRole("article", { name: "memory record" })).toHaveCount(0);
    await expect(garden(page).getByTestId("garden-selection-summary").getByRole("button", { name: "Open record", exact: true })).toBeVisible();
    await memory.press("Enter");
    const record = garden(page).getByRole("article", { name: "memory record" });
    await expect(garden(page).getByTestId("garden-selection-summary").getByRole("button", { name: "Open record", exact: true })).toHaveCount(0);
    await expect(record).toContainText("Review confirmed that Memory stays beside Skills");
    await expect(record).toContainText(GARDEN_ROOT);
    await expect(record).toContainText("conversation-design:turn:4");
    await record.getByText("Revision history (2)").click();
    await expect(record).toContainText("Keep agent regions stable.");
    await capture(page, "03-memory-evidence");
    await page.keyboard.press("Escape");
    await expect(garden(page).getByRole("region", { name: "Memory", exact: true })).toBeVisible();
    await garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).getByRole("button", { name: "Habitat", exact: true }).click();
    await expect(canvas).toHaveAttribute("data-garden-fit", before!);
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).getByRole("button", { name: "Moss Designer", exact: true })).toHaveCount(0);
  });

  test("peer Ports link enters Fern at its world location and Escape restores Moss", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await enterAgent(page);
    const moss = garden(page).locator(`[data-garden-cell="agent:${GARDEN_AGENT}"]`);
    const mossWorld = await moss.getAttribute("data-garden-world");
    const ports = moss.getByRole("region", { name: "Workspace, Teams, Agents", exact: true });
    const peer = ports.getByRole("button", { name: /Fern Reviewer/ });
    await peer.click();
    await expect(peer).toHaveAttribute("aria-pressed", "true");
    // Selecting a link to a distant peer must not make wheel over these Ports enter it.
    const hit = await peer.boundingBox();
    if (!hit) throw new Error("Peer port has no bounds");
    const zoom = garden(page).getByTestId("garden-zoom-level");
    const before = await zoom.textContent();
    await page.mouse.move(hit.x + hit.width / 2, hit.y + hit.height / 2);
    await page.mouse.wheel(0, -120);
    await expect(zoom).not.toHaveText(before!);
    const trail = garden(page).getByRole("navigation", { name: "Garden breadcrumb" });
    await expect(trail.getByRole("button").last()).toHaveText("Moss Designer");
    await peer.press("Enter");
    const fern = garden(page).locator('[data-garden-cell="agent:garden-reviewer"]');
    const fernIdentity = fern.getByRole("region", { name: "Identity", exact: true });
    await expect(fernIdentity).toBeInViewport();
    await expect(fernIdentity.getByRole("button", { name: /Fern Reviewer.*Reviewer/ })).toBeVisible();
    await expect(trail.getByRole("button").last()).toHaveText("Fern Reviewer");
    await expect(fern).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(trail.getByRole("button").last()).toHaveText("Moss Designer");
    await expect(moss.getByRole("region", { name: "Identity", exact: true })).toBeInViewport();
    await expect(moss.getByRole("region", { name: "Identity", exact: true })).toContainText("Moss Designer");
    await expect(moss).toHaveAttribute("data-garden-world", mossWorld!);
    await expect(moss).toBeFocused();
  });

  test("narrow memory record supports Tab and PageDown without moving the world camera", async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const label of ["Hide Left Sidebar", "Hide Agent List"]) {
      const toggle = page.getByRole("button", { name: label, exact: true });
      if (await toggle.isVisible()) await toggle.click();
    }
    await garden(page).getByTestId("garden-fit-view").click();
    await object(page, `district:workspace:${GARDEN_ROOT}`).press("Enter");
    await enterAgent(page);
    await garden(page).getByRole("button", { name: /Keep the five agent regions/ }).press("Enter");
    const cell = garden(page).locator('[data-garden-cell^="memory:"]');
    const reading = cell.getByRole("region", { name: /reading area$/ });
    await expect(cell).toBeFocused();
    await expect(reading).toBeInViewport({ ratio: 1 });
    await expect(reading.getByRole("article", { name: "memory record" })).toContainText("conversation-design:turn:4");
    await page.keyboard.press("Tab");
    await expect(reading).toBeFocused();
    // Expand the existing revision evidence so this fixture exercises real overflow.
    await reading.getByText("Revision history (2)", { exact: true }).press("Enter");
    await expect(reading).toContainText("Keep agent regions stable.");
    // Evidence and source now have their own keyboard-accessible disclosures.
    for (const name of ["Sources (1)", "Evidence", "Full source"]) {
      await page.keyboard.press("Shift+Tab");
      await expect(reading.getByText(name, { exact: true })).toBeFocused();
    }
    await page.keyboard.press("Shift+Tab");
    await expect(reading).toBeFocused();
    await page.keyboard.press("Home");
    await expect.poll(() => reading.evaluate((element) => element.scrollTop)).toBe(0);
    expect(await reading.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
    const before = await cell.boundingBox();
    const world = await cell.getAttribute("data-garden-world");
    const zoom = await garden(page).getByTestId("garden-zoom-level").textContent();
    const initialScroll = await reading.evaluate((element) => element.scrollTop);
    expect(await reading.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.keyboard.press("PageDown");
    await expect.poll(() => reading.evaluate((element) => element.scrollTop)).toBeGreaterThan(initialScroll);
    await expect(cell).toHaveAttribute("data-garden-world", world!);
    await expect(garden(page).getByTestId("garden-zoom-level")).toHaveText(zoom!);
    expect(await cell.boundingBox()).toEqual(before);
    await page.keyboard.press("Escape");
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).getByRole("button").last()).toHaveText("Moss Designer");
  });

  test("workspace activity excludes unchanged siblings until full tree; file record carries evidence", async ({ page }) => {
    await enterAgent(page);
    await garden(page).getByRole("region", { name: "Workspace, Teams, Agents" }).getByRole("button", { name: /synthetic\/garden/ }).press("Enter");
    const workspace = garden(page).getByRole("region", { name: "Workspace activity" });
    await expect(workspace.getByRole("button", { name: /src/ })).toBeVisible();
    await expect(workspace.getByRole("button", { name: /README/ })).toHaveCount(0);
    await capture(page, "04-workspace-activity");
    await workspace.getByRole("checkbox", { name: "Show full tree" }).check();
    await expect(workspace.getByRole("button", { name: /README/ })).toBeVisible();
    await capture(page, "05-workspace-full-tree");
    await workspace.getByRole("checkbox", { name: "Show full tree" }).uncheck();
    await workspace.getByRole("button", { name: /src/ }).press("Enter");
    await workspace.getByRole("button", { name: /cutaway.tsx/ }).press("Enter");
    const record = garden(page).getByRole("article", { name: "path record" });
    await expect(record).toContainText("export const regions");
    await expect(record).toContainText("attributed");
    await expect(record).toContainText("garden-reviewer");
    await expect(record.getByRole("button", { name: "Open file", exact: true })).toBeVisible();
    await capture(page, "06-file-evidence");
  });

  test("multi-agent schedule opens ordered run stages and immutable output evidence", async ({ page }) => {
    await enterAgent(page);
    await garden(page).getByRole("region", { name: "Automations, Conversations, Inbox", exact: true }).getByRole("button", { name: /Daily design review/ }).press("Enter");
    const flow = garden(page).getByRole("region", { name: "Automation composition" });
    await expect(flow).toContainText("2 assigned agents");
    const lane = flow.getByRole("region", { name: `Run ${GARDEN_RUN}`, exact: true });
    await expect(lane.locator('[data-garden-ref^="stage:"]')).toHaveText([/1.*Draft interface.*completed.*Moss Designer/, /2.*Review evidence.*running.*Fern Reviewer/]);
    await capture(page, "07-run-flow");
    await lane.getByRole("button", { name: /Draft interface/ }).press("Enter");
    await expect(garden(page).getByRole("article")).toContainText("cutaway-preview");
    await expect(garden(page).getByRole("article")).toContainText(GARDEN_RUN);
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).getByRole("button").last()).toHaveText("draft");
    await capture(page, "08-stage-output");
    await page.keyboard.press("Escape");
    await expect(flow).toBeVisible();
  });

  test("canonical agent action preserves Garden breadcrumb and selection on return", async ({ page }) => {
    await openSurface(page, "agents-overview");
    await expect(surfaceTab(page, "agents-overview")).toHaveCount(1);
    await surfaceTab(page, "garden").click();
    await enterAgent(page);
    await garden(page).getByRole("region", { name: "Identity", exact: true }).getByRole("button", { name: /Moss Designer.*Designer/ }).press("Enter");
    const breadcrumb = await garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).textContent();
    const camera = await garden(page).locator(".garden-canvas").getAttribute("data-garden-fit");
    await garden(page).getByRole("article", { name: "identity record" }).getByRole("button", { name: "Open agent session", exact: true }).click();
    await expect(surfaceTab(page, "agent-session")).toHaveCount(1);
    await expect(surfaceTab(page, "agents-overview")).toHaveCount(1);
    await expect(surfaceTab(page, "agent-session")).toHaveAttribute("aria-selected", "true");
    await expect(surfacePanel(page, "agent-session").getByRole("textbox", { name: "Terminal input" })).toBeAttached();
    await surfaceTab(page, "garden").click();
    await expect(garden(page).getByRole("article", { name: "identity record" })).toBeVisible();
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" })).toHaveText(breadcrumb!);
    await expect(garden(page).locator(".garden-canvas")).toHaveAttribute("data-garden-fit", camera!);
    await surfaceTab(page, "agent-session").click();
    await surfaceTab(page, "agent-session").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Close tab", exact: true }).click();
    await expect(garden(page).getByRole("article", { name: "identity record" })).toBeVisible();
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" })).toHaveText(breadcrumb!);
    await expect(garden(page).locator(".garden-canvas")).toHaveAttribute("data-garden-fit", camera!);
  });

  test("keyboard roving focus, Space selection, narrow layout and reduced motion", async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const label of ["Hide Left Sidebar", "Hide Agent List"]) {
      const toggle = page.getByRole("button", { name: label, exact: true });
      if (await toggle.isVisible()) await toggle.click();
    }
    await expect(garden(page).locator("canvas").first()).toBeVisible();
    await garden(page).getByTestId("garden-fit-view").click();
    await object(page, `district:workspace:${GARDEN_ROOT}`).press("Enter");
    const agent = object(page, `agent:${GARDEN_AGENT}`);
    await agent.focus();
    await agent.press("Space");
    await expect(agent).toHaveAttribute("aria-pressed", "true");
    await agent.press("ArrowRight");
    await expect(agent).not.toBeFocused();
    await agent.press("Enter");
    const cell = garden(page).locator(`[data-garden-cell="agent:${GARDEN_AGENT}"]`);
    const world = await cell.getAttribute("data-garden-world");
    await expect(cell).toHaveCSS("animation-name", "none");
    // Enter focuses the world cell; keyboard zoom must work even when already readable.
    await expect(cell).toBeFocused();
    const beforeZoom = await cell.boundingBox();
    if (!beforeZoom) throw new Error("Agent cell has no bounds");
    await cell.press("+");
    await expect.poll(async () => (await cell.boundingBox())!.width / beforeZoom.width).toBeCloseTo(1.25, 2);
    await cell.press("-");
    await expect.poll(async () => (await cell.boundingBox())!.width).toBeCloseTo(beforeZoom.width, 1);
    await expect.poll(async () => (await cell.boundingBox())!.height).toBeCloseTo(beforeZoom.height, 1);
    const identity = garden(page).getByRole("region", { name: "Identity", exact: true });
    const ports = garden(page).getByRole("region", { name: "Workspace, Teams, Agents", exact: true });
    await expect(identity).toBeVisible();
    await expect(ports).toBeVisible();
    await expect(cell).toHaveAttribute("data-garden-world", world!);
    const identityBounds = await identity.boundingBox();
    const portsBounds = await ports.boundingBox();
    expect(portsBounds!.y).toBeGreaterThan(identityBounds!.y);
    const capabilitiesBounds = await garden(page).getByRole("region", { name: "Skills", exact: true }).boundingBox();
    const memoryBounds = await garden(page).getByRole("region", { name: "Memory", exact: true }).boundingBox();
    expect(capabilitiesBounds!.x).toBeLessThan(identityBounds!.x);
    expect(memoryBounds!.x).toBeGreaterThan(identityBounds!.x);
    expect(await garden(page).evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await capture(page, "09-narrow-reduced-motion");
    await page.keyboard.press("Escape");
    await expect(garden(page).getByRole("navigation", { name: "Garden breadcrumb" }).getByRole("button", { name: "Moss Designer", exact: true })).toHaveCount(0);
  });

  test("workstream shows labelled inhabitants and situated run route", async ({ page }) => {
    await object(page, `district:workspace:${GARDEN_ROOT}`).press("Enter");
    await expect(garden(page).locator(".garden-canvas")).toHaveAttribute("data-focused-district", `workspace:${GARDEN_ROOT}`);
    await expect(object(page, `agent:${GARDEN_AGENT}`)).toBeAttached();
    await expect(object(page, "automation:schedule:daily-design")).toBeAttached();
    await capture(page, "10-workstream-inhabitants");
  });

  for (const action of ["double-click", "keyboard Enter", "summary Enter"] as const) {
    test(`nested canvas directory opens Workspace with ${action}`, async ({ page }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await object(page, `district:workspace:${GARDEN_ROOT}`).press("Enter");
      const directory = garden(page).locator(`[data-garden-object$=":${GARDEN_ROOT}/src"]`);
      await expect(directory).toBeAttached();
      const hit = await directory.boundingBox();
      if (!hit) throw new Error("Nested directory has no canvas target");
      const point = { x: hit.x + hit.width / 2, y: hit.y + hit.height / 2 };
      const camera = await garden(page).locator(".garden-canvas").getAttribute("data-garden-fit");
      await page.mouse.click(point.x, point.y);
      await expect(directory).toHaveAttribute("aria-pressed", "true");
      await expect(directory).toHaveAttribute("data-garden-object", `workspace:${GARDEN_ROOT}/src`);
      await expect(garden(page).locator(".garden-canvas")).toHaveAttribute("data-garden-fit", camera!);
      if (action === "double-click") await page.mouse.dblclick(point.x, point.y);
      else if (action === "keyboard Enter") await directory.press("Enter");
      else await garden(page).getByTestId("garden-selection-summary").getByRole("button", { name: "Enter", exact: true }).click();
      await expect(garden(page).getByRole("region", { name: "Workspace activity" })).toBeVisible();
      await expect(garden(page).getByRole("region", { name: "Workspace activity" })).toContainText(`${GARDEN_ROOT}/src`);
      await expect(garden(page).getByRole("article", { name: "path record" })).toHaveCount(0);
      await expect(garden(page).getByRole("button", { name: /cutaway.tsx/ })).toBeVisible();
    });
  }

  test("composition margins retain camera pan and wheel access", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await enterAgent(page);
    const canvas = garden(page).locator(".garden-canvas");
    const cell = garden(page).locator(`[data-garden-cell="agent:${GARDEN_AGENT}"]`);
    const world = await cell.getAttribute("data-garden-world");
    const bounds = await canvas.boundingBox();
    if (!bounds) throw new Error("Canvas has no bounds");
    const before = await cell.boundingBox();
    const point = { x: bounds.x + 5, y: bounds.y + bounds.height / 2 };
    await page.mouse.move(point.x, point.y);
    await page.mouse.down();
    await page.mouse.move(point.x, point.y + 90, { steps: 8 });
    await page.mouse.up();
    await expect.poll(async () => (await cell.boundingBox())!.y).toBeCloseTo(before!.y + 90, 0);
    const zoom = await garden(page).getByTestId("garden-zoom-level").textContent();
    await page.mouse.wheel(0, -120);
    await expect(garden(page).getByTestId("garden-zoom-level")).not.toHaveText(zoom!);
    await expect(cell).toHaveAttribute("data-garden-world", world!);
    await expect(garden(page).getByRole("region", { name: "Identity", exact: true })).toBeVisible();
  });

  test("canonical run evidence opens Observe and schedule management opens Monitor with Garden return", async ({ page }) => {
    await enterAgent(page);
    await garden(page).getByRole("region", { name: "Automations, Conversations, Inbox", exact: true }).getByRole("button", { name: /Daily design review/ }).press("Enter");
    await garden(page).getByRole("region", { name: `Run ${GARDEN_RUN}`, exact: true }).getByRole("button", { name: /Draft interface/ }).press("Enter");
    await garden(page).getByRole("article").getByRole("button", { name: "Inspect run evidence", exact: true }).click();
    const automations = surfacePanel(page, "automations");
    await expect(automations.getByTestId("automations-observe-mode")).toBeVisible();
    await expect(automations.getByTestId("automations-observe-mode")).toContainText(GARDEN_RUN);
    await surfaceTab(page, "automations").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Close tab", exact: true }).click();
    await expect(garden(page).getByRole("article")).toContainText("cutaway-preview");
    await garden(page).locator('[data-garden-cell^="stage:"]').press("Escape");
    await expect(garden(page).locator('[data-garden-cell^="stage:"]')).toHaveCount(0);
    await garden(page).getByRole("button", { name: "Manage schedules in Monitor", exact: true }).click();
    await expect(automations.getByTestId("automation-monitor")).toBeVisible();
    await surfaceTab(page, "automations").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Close tab", exact: true }).click();
    await expect(garden(page).getByRole("region", { name: "Automation composition" })).toBeVisible();
  });
});
