import { test, expect, type Page } from "@playwright/test";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { seedLibraryFixtures } from "../fixtures/mockAgent";
import {
  buildLibraryContentFixture,
  buildLibraryIndexFixture,
  installLibraryIpcMock,
} from "../fixtures/libraryIpcMock";
import { openSurface, surfacePanel } from "../fixtures/workbench";

/**
 * Library redesign browser E2E tests.
 *
 * These cover browser-layer rendering and interaction only: section rail
 * switching, list rendering from a seeded fixture, search/star, and
 * detail-pane open/edit flows. Like every other browser E2E spec in this
 * suite (see e2e/tests/graph-topology.spec.ts), there is no real Tauri
 * backend behind the Vite dev server the webServer block boots — so the
 * fixture below is installed by mocking `window.__TAURI_INTERNALS__.invoke`
 * directly, rather than by pointing the app at a real seeded WARDIAN_HOME.
 *
 * Real filesystem claims — a deploy creating an actual junction/reparse
 * point, undeploy leaving the source untouched, rename relinking a
 * deployment — cannot be proven at this layer and are covered by
 * e2e-native/tests/library-deployment-native.test.mjs instead.
 *
 * The fixture/mock builders live in `../fixtures/libraryIpcMock` so
 * `e2e/tests/features.spec.ts` can install the exact same bridge rather than
 * hand-rolling a divergent one.
 */

async function openLibraryView(page: Page) {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await openSurface(page, "library");
  await expect(surfacePanel(page, "library").locator('[data-testid="library-view"]')).toBeVisible({ timeout: 10_000 });
}

test.describe("Library Redesign", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;

  test.beforeAll(async ({ browser }) => {
    page = await browser.newPage();
    await installLibraryIpcMock(page, buildLibraryIndexFixture(), buildLibraryContentFixture());
    await openLibraryView(page);
  });

  test.afterAll(async () => {
    await page.close();
  });

  test("renders all five rail sections with entry counts", async () => {
    const rail = page.locator('[data-testid="library-section-rail"]');
    await expect(rail).toBeVisible();

    await expect(page.locator('[data-testid="library-section-skills"]')).toContainText("2");
    await expect(page.locator('[data-testid="library-section-prompts"]')).toContainText("1");
    await expect(page.locator('[data-testid="library-section-classes"]')).toContainText("1");
    await expect(page.locator('[data-testid="library-section-workflows"]')).toContainText("1");
    // MCPs is stubbed with zero entries: no count badge renders at all.
    await expect(page.locator('[data-testid="library-section-mcps"]')).toBeVisible();
    await expect(page.locator('[data-testid="library-section-mcps"] span').nth(1)).toHaveCount(0);
  });

  test("navigates a nested skill folder and opens the entry", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    await expect(page.locator('[data-testid="library-list"]')).toBeVisible();

    // Folders start collapsed: the nested skill row isn't rendered yet.
    await expect(page.locator('[data-testid="library-row-skills/dev/planner"]')).toHaveCount(0);

    await page.locator('[data-testid="library-folder-dev"]').click();
    const row = page.locator('[data-testid="library-row-skills/dev/planner"]');
    await expect(row).toBeVisible();

    await row.click();
    await expect(page.locator('[data-testid="skill-detail"]')).toBeVisible();
    await expect(page.locator('[data-testid="detail-header"]')).toContainText("planner");
    await expect(page.locator('[data-testid="markdown-editor-textarea"]')).toHaveValue(/Planner/);
  });

  test("search flattens results and shows a path subtitle", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    await page.locator('[data-testid="library-search"]').fill("planner");

    const subtitle = page.locator('[data-testid="library-row-subtitle-skills/dev/planner"]');
    await expect(subtitle).toBeVisible();
    await expect(subtitle).toContainText("dev");

    await page.locator('[data-testid="library-search"]').fill("");
  });

  test("toggles the star on a skill row", async () => {
    await page.locator('[data-testid="library-section-skills"]').click();
    const starButton = page.locator('[data-testid="library-star-skills/dev/planner"]');
    await expect(starButton).toHaveAttribute("aria-pressed", "false");

    await starButton.click();
    await expect(starButton).toHaveAttribute("aria-pressed", "true");
  });

  test("edits and saves a prompt via Ctrl+S", async () => {
    await page.locator('[data-testid="library-section-prompts"]').click();
    await page.locator('[data-testid="library-row-prompts/greeting.md"]').click();

    const editor = page.locator('[data-testid="markdown-editor"]');
    await expect(editor).toBeVisible();
    await expect(editor).toContainText("Saved");

    const textarea = page.locator('[data-testid="markdown-editor-textarea"]');
    await textarea.click();
    await textarea.type("\nExtra line.");
    await expect(editor).toContainText("Unsaved changes");
    await expect(page.locator('[data-testid="markdown-editor-dirty-dot"]')).toBeVisible();

    await textarea.press("Control+s");
    await expect(editor).toContainText("Saved");
    await expect(page.locator('[data-testid="markdown-editor-dirty-dot"]')).toHaveCount(0);
  });

  test("opens a class and shows the AGENTS.md editor", async () => {
    await page.locator('[data-testid="library-section-classes"]').click();
    await page.locator('[data-testid="library-row-classes/Architect"]').click();

    await expect(page.locator('[data-testid="class-detail"]')).toBeVisible();
    await expect(page.locator('[data-testid="markdown-editor-textarea"]')).toHaveValue(/Role: Architect/);
  });

  test("shows the MCP stub copy", async () => {
    await page.locator('[data-testid="library-section-mcps"]').click();
    await expect(page.locator('[data-testid="library-mcp-stub"]')).toContainText(
      "MCP servers are coming to the library",
    );
    await expect(page.locator('[data-testid="mcp-stub-detail"]')).toContainText(
      "MCP servers are coming to the library",
    );
  });

  test("deploying a skill creates a real junction/reparse point", () => {
    test.skip(
      true,
      "junction behavior — see library-deployment-native.test.mjs", // @native-only
    );
  });
});

test.describe("seedLibraryFixtures fixture", () => {
  test("writes the nested skills, prompt, workflow, and class the native tests read back", () => {
    // Runs in the Playwright Node context: the browser layer above never
    // reads this directory (there is no real Tauri backend behind the Vite
    // dev server), so the fixture is verified by its on-disk output here —
    // the same split used for `seedTopology` in graph-topology.spec.ts. The
    // native E2E layer is what actually exercises this helper against a
    // live app.
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-library-fixture-"));
    try {
      seedLibraryFixtures(home);

      const planner = fs.readFileSync(
        path.join(home, "library", "skills", "dev", "planner", "SKILL.md"),
        "utf8",
      );
      expect(planner).toContain("description: Plans work");

      const reviewer = fs.readFileSync(
        path.join(home, "library", "skills", "ops", "reviewer", "SKILL.md"),
        "utf8",
      );
      expect(reviewer).toContain("description: Reviews code");

      const greeting = fs.readFileSync(path.join(home, "library", "prompts", "greeting.md"), "utf8");
      expect(greeting).toContain("Say hello to the team");

      const triage = fs.readFileSync(path.join(home, "library", "workflows", "triage.md"), "utf8");
      expect(triage).toContain("description: Triage workflow");

      const agentsMd = fs.readFileSync(path.join(home, "classes", "Architect", "AGENTS.md"), "utf8");
      expect(agentsMd).toContain("Role: Architect");
    } finally {
      fs.rmSync(home, { recursive: true, force: true });
    }
  });
});
