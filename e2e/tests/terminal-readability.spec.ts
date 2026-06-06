import { expect, test, type Page } from "@playwright/test";

function hexToRgb(hex: string) {
  const cleaned = hex.replace("#", "");
  return {
    r: Number.parseInt(cleaned.slice(0, 2), 16),
    g: Number.parseInt(cleaned.slice(2, 4), 16),
    b: Number.parseInt(cleaned.slice(4, 6), 16),
  };
}

function channelLuminance(value: number) {
  const normalized = value / 255;
  return normalized <= 0.03928
    ? normalized / 12.92
    : ((normalized + 0.055) / 1.055) ** 2.4;
}

function contrastRatio(foreground: string, background: string) {
  const fg = hexToRgb(foreground);
  const bg = hexToRgb(background);
  const fgLum =
    channelLuminance(fg.r) * 0.2126 +
    channelLuminance(fg.g) * 0.7152 +
    channelLuminance(fg.b) * 0.0722;
  const bgLum =
    channelLuminance(bg.r) * 0.2126 +
    channelLuminance(bg.g) * 0.7152 +
    channelLuminance(bg.b) * 0.0722;
  const lighter = Math.max(fgLum, bgLum);
  const darker = Math.min(fgLum, bgLum);
  return (lighter + 0.05) / (darker + 0.05);
}

async function installTerminalReadabilityMock(page: Page) {
  await page.addInitScript(() => {
    let callbackId = 1;
    let ptyReadCount = 0;
    const agent = {
      id: "agent-issue-360",
      session_id: "mock-session-e2e-001",
      session_name: "Dark Terminal Audit",
      agent_class: "Coder",
      provider: "codex",
      folder: "/workspace/wardian",
      is_off: false,
    };
    const terminalOutput = [
      "\x1b[1;33m$\x1b[0m npm run build\r\n",
      "\x1b[36mprocessing\x1b[0m resolving modules and checking terminal theme\r\n",
      "\x1b[32m✓\x1b[0m Built 128 modules in 1.42s\r\n",
      "\x1b[33m! action required\x1b[0m Review changed files before merging\r\n",
      "\x1b[31merror\x1b[0m src/features/terminal/AgentTerminal.tsx: contrast budget failed\r\n",
      "\x1b[90mcompleted summary\x1b[0m 3 tasks done, 1 follow-up remains\r\n",
    ].join("");
    const readiness = ["claude", "codex", "gemini", "antigravity", "opencode"].map((provider) => ({
      provider,
      display_name: provider[0].toUpperCase() + provider.slice(1),
      available: true,
      executable: `${provider}.cmd`,
      reason: null,
    }));
    const tauriWindow = window as Window & {
      __TAURI_INTERNALS__?: Record<string, unknown>;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    };

    tauriWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
    tauriWindow.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      transformCallback: () => callbackId++,
      unregisterCallback: () => undefined,
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (command: string) => {
        if (command === "plugin:window|is_maximized") return false;
        if (command === "load_app_settings") {
          return {
            schema_version: 2,
            settings: {
              theme: "dark",
              auto_patch_gemini: false,
              terminal_font_size: 14,
              terminal_font_family: null,
              grid_card_display_mode: "terminal",
              watchlist_new_agent_position: "top",
              titlebar_telemetry_visible: true,
              external_editor: "system",
              external_editor_custom_executable: null,
              explorer_file_click_action: "preview",
            },
            overrides: { theme: "dark" },
            persisted: true,
          };
        }
        if (command === "load_shell_settings") {
          return {
            schema_version: 2,
            settings: {
              shell_id: "auto",
              custom_executable: null,
              custom_args: null,
              agent_session_persistence: "resume",
              codex_runtime_policy: {
                sandbox_mode: "workspace-write",
                approval_policy: "on-request",
                full_auto: false,
              },
              default_provider: "auto",
            },
            overrides: {},
          };
        }
        if (command === "list_available_shells") return [];
        if (command === "list_provider_readiness") return readiness;
        if (command === "list_agent_classes") {
          return [{ name: "Coder", description: "Writes code", is_default: true }];
        }
        if (command === "list_agents") return [agent];
        if (command === "load_watchlists") {
          return [{ id: "all", name: "All Agents", agent_ids: ["agent-issue-360"] }];
        }
        if (command === "load_watchlist_prefs") return null;
        if (command === "load_agent_interactions") return {};
        if (command === "load_queue_items") return [];
        if (command === "load_queue_preferences") return null;
        if (command === "load_onboarding_hints") {
          return { dismissed_hint_ids: ["spawn-agent-first-run:v1"] };
        }
        if (command === "list_workflows" || command === "list_scheduled_runs") return [];
        if (command === "load_workflow_library") return { folders: [], rootWorkflowIds: [] };
        if (command === "get_library_tree") {
          return { type: "Folder", path: "", name: "Root", children: [] };
        }
        if (command === "list_deployed_skills") return [];
        if (command === "sync_provider_theme_settings") return null;
        if (command === "resize_agent_terminal") return null;
        if (command === "send_input_to_agent") return null;
        if (command === "terminal_link_target_exists") return true;
        if (command === "read_agent_pty") return ptyReadCount++ === 0 ? terminalOutput : null;
        if (command === "plugin:event|listen") return callbackId++;
        if (command === "plugin:event|unlisten") return null;
        return null;
      },
    };
  });
}

test("dark terminal uses readable typography and semantic theme colors", async ({ page }, testInfo) => {
  await installTerminalReadabilityMock(page);
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.locator('[data-testid="app-shell"]').waitFor({ timeout: 20_000 });
  await page.getByRole("button", { name: "Grid" }).click();
  await page.locator('[data-testid="agent-terminal-host"]').waitFor();

  await expect
    .poll(async () =>
      page.evaluate(() =>
        (window as Window & {
          __wardianTerminalDebug?: {
            snapshot?: (sessionId: string) => { renderer?: { lines: string[] } | null } | null;
          };
        }).__wardianTerminalDebug?.snapshot?.("mock-session-e2e-001")?.renderer?.lines.join("\n") ?? "",
      ),
    )
    .toContain("action required");

  const terminal = await page.evaluate(() => {
    const styles = getComputedStyle(document.documentElement);
    const read = (name: string) => styles.getPropertyValue(name).trim();
    const platform = navigator.platform.toLowerCase();
    const userAgent = navigator.userAgent.toLowerCase();
    const expectedFontFamily =
      platform.includes("mac") || userAgent.includes("mac os")
        ? 'Menlo, Monaco, "Courier New", monospace'
        : platform.includes("win") || userAgent.includes("windows")
          ? 'Consolas, "Courier New", monospace'
          : '"Droid Sans Mono", monospace';
    return {
      background: read("--color-wardian-terminal-bg"),
      foreground: read("--color-wardian-terminal-fg"),
      warning: read("--color-wardian-terminal-yellow"),
      error: read("--color-wardian-terminal-red"),
      success: read("--color-wardian-terminal-green"),
      processing: read("--color-wardian-terminal-cyan"),
      expectedFontFamily,
      renderer: (window as Window & {
        __wardianTerminalDebug?: {
          snapshot?: (sessionId: string) => {
            renderer?: {
              fontFamily: string;
              fontSize: number | null;
              cssCellHeight: number | null;
            } | null;
          } | null;
        };
      }).__wardianTerminalDebug?.snapshot?.("mock-session-e2e-001")?.renderer,
    };
  });

  expect(terminal.background).toBe("#08100d");
  expect(terminal.renderer?.fontFamily).toBe(terminal.expectedFontFamily);
  expect(terminal.renderer?.fontSize).toBe(14);
  expect(terminal.renderer?.cssCellHeight ?? 0).toBeGreaterThanOrEqual(17);
  [terminal.foreground, terminal.warning, terminal.error, terminal.success, terminal.processing].forEach(
    (color) => expect(contrastRatio(color, terminal.background)).toBeGreaterThanOrEqual(4.5),
  );

  await page
    .locator('[data-testid="agent-card"]')
    .screenshot({ path: testInfo.outputPath("terminal-readability-dark.png") });
});
