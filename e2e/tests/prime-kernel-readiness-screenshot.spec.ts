/**
 * PR evidence for the Prime Agent kernel readiness states.
 *
 * Captures the two labels a user can now see for an installed provider whose
 * runtime dependency is missing. Before this change both rendered as
 * "Prime Agent - not installed", which told the user to reinstall software they
 * already had.
 */

import { test, type Page } from "@playwright/test";
import * as path from "path";

type ReadinessFixture = {
  provider: string;
  display_name: string;
  available: boolean;
  executable: string | null;
  reason: string | null;
};

const SETTING_UP: ReadinessFixture = {
  provider: "prime",
  display_name: "Prime Agent",
  available: false,
  executable: "C:/Users/dev/AppData/Roaming/npm/prime-agent",
  reason:
    "Wardian is setting up Prime Agent's Python kernel. This runs once and takes a minute; Prime Agent becomes available when it finishes.",
};

const NEEDS_KERNEL: ReadinessFixture = {
  provider: "prime",
  display_name: "Prime Agent",
  available: false,
  executable: "C:/Users/dev/AppData/Roaming/npm/prime-agent",
  reason:
    "Wardian could not set up Prime Agent's Python kernel because `uv` is not installed. Install uv (https://docs.astral.sh/uv/) and restart Wardian, or set PRIME_AGENT_KERNEL_PYTHON to an interpreter that already has ipykernel and prime-agent-runtime.",
};

async function installReadinessMock(page: Page, prime: ReadinessFixture) {
  await page.addInitScript((primeReadiness) => {
    const tauriWindow = window as unknown as Record<string, unknown>;
    const callbacks = new Map<number, unknown>();
    let callbackId = 1;

    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
      transformCallback: (callback: unknown) => {
        const id = callbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => {
        callbacks.delete(id);
      },
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (command: string) => {
        // Prime as the default provider is what makes the panel render the
        // fallback note. Native <select> popups are drawn by the OS and cannot
        // be screenshotted, so the note is where this change is visible.
        if (command === "load_app_settings") {
          return { settings: { default_provider: "prime" }, overrides: {}, persisted: true };
        }
        if (command === "list_provider_readiness") {
          return [
            {
              provider: "claude",
              display_name: "Claude",
              available: true,
              executable: "claude",
              reason: null,
            },
            {
              provider: "codex",
              display_name: "Codex",
              available: false,
              executable: null,
              reason: "Codex is not available because the codex command was not found.",
            },
            primeReadiness,
          ];
        }
        if (command === "list_agents") return [];
        if (command === "list_agent_classes") {
          return [{ name: "Architect", description: "Designs systems", is_default: false }];
        }
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "load_watchlists") return [];
        if (command === "load_queue_items") return [];
        if (command === "list_workflows") return [];
        return null;
      },
    };
  }, prime);
}

async function captureProviderList(page: Page, name: string, outputDir: string) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 15_000 });
  await page.locator('[data-testid="sidebar-tab-agent-config"]').click();
  const select = page.locator('[data-testid="spawn-provider"]');
  await select.waitFor({ timeout: 15_000 });
  await page
    .locator('[data-testid="spawn-provider"] option', { hasText: "Prime Agent" })
    .waitFor({ state: "attached", timeout: 15_000 });

  // A native <select> popup is drawn by the OS and cannot be captured, so the
  // list is expanded in place. The labels are still the component's own.
  await select.evaluate((element) => {
    element.setAttribute("size", "6");
    element.scrollIntoView({ block: "center" });
  });

  await select.screenshot({ path: path.join(outputDir, `${name}.png`) });
}

test.describe("Prime Agent kernel readiness evidence", () => {
  test("captures the provisioning and blocked provider states", async ({ page }, testInfo) => {
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const outputDir = path.join(
      testInfo.project.testDir,
      "..",
      "screenshots",
      "prime-kernel-readiness",
      stamp,
    );

    await installReadinessMock(page, SETTING_UP);
    await captureProviderList(page, "prime-kernel-provisioning", outputDir);

    await page.context().clearCookies();
    await installReadinessMock(page, NEEDS_KERNEL);
    await captureProviderList(page, "prime-kernel-needs-setup", outputDir);

    testInfo.annotations.push({ type: "screenshots", description: outputDir });
    console.log(`screenshots: ${outputDir}`);
  });
});
