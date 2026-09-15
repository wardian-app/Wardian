import { mkdirSync } from "node:fs";
import * as path from "node:path";
import { expect, test, type Page, type Locator } from "@playwright/test";
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";
import { openSurface } from "../fixtures/workbench";

const PROVIDERS = ["claude", "codex", "antigravity", "opencode", "pi", "gemini"] as const;
const ESC = "\u001b";
const LINK_LABEL = "Issue 1336";
const ADJACENT_UNLINKED = "adjacent-unlinked";

type Provider = (typeof PROVIDERS)[number];

type HyperlinkFixture = {
  provider: Provider;
  session_id: string;
  session_name: string;
  target: string;
  plain_target: string;
  state: string;
  history: string[];
};

type MockCall = {
  command: string;
  args?: Record<string, unknown>;
};

function makeHyperlinkFixture(provider: Provider, mouseReporting = false): HyperlinkFixture {
  const target = `https://wardian.org/issues/1336/${provider}`;
  const plainTarget = `https://wardian.org/plain-control/${provider}`;
  const osc8 = `${ESC}]8;;${target}${ESC}\\${LINK_LABEL}${ESC}]8;;${ESC}\\`;
  const mouseMode = mouseReporting ? `${ESC}[?1000h${ESC}[?1006h` : "";

  return {
    provider,
    session_id: `terminal-hyperlinks-${provider}`,
    session_name: `Issue 1336 ${provider}`,
    target,
    plain_target: plainTarget,
    state: `${ESC}[H${mouseMode}${osc8} ${ADJACENT_UNLINKED} plain ${plainTarget}\r\n`,
    history: Array.from({ length: 4 }, (_, index) =>
      `SNAPSHOT_SCROLLBACK_${provider.toUpperCase()}_${index + 1}`,
    ),
  };
}

function visibleAgentCard(page: Page, sessionName: string) {
  return page
    .locator('[data-testid="agent-card"]:visible')
    .filter({ hasText: sessionName })
    .last();
}

async function installTerminalSnapshotMock(page: Page, fixture: HyperlinkFixture) {
  await page.evaluate((nextFixture) => {
    type Request = {
      presentation_id?: string;
      session_id?: string;
      desired_geometry?: { cols?: number; rows?: number } | null;
      geometry?: { cols?: number; rows?: number } | null;
      cols?: number;
      rows?: number;
    };
    type Runtime = {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: { request?: Request }) => Promise<unknown>;
      };
    };

    const runtime = window as unknown as Runtime;
    const originalInvoke = runtime.__TAURI_INTERNALS__.invoke;
    let geometry = { cols: 80, rows: 24 };
    let presentationId = "pending";
    let owner: string | null = null;

    const state = () => ({
      session_id: nextFixture.session_id,
      runtime_generation: 1,
      lease_epoch: 1,
      stream_sequence: 0,
      interaction_sequence: 0,
      geometry,
      owner_presentation_id: owner,
      pending_activation: null,
      runtime_state: "live",
    });
    const decision = () => ({
      status: "accepted",
      reason: null,
      runtime_generation: 1,
      lease_epoch: 1,
      owner_presentation_id: owner,
    });
    const presentation = () => ({
      presentation_id: presentationId,
      client_kind: "desktop",
      desired_geometry: geometry,
      visibility: "visible",
      render_state: "mounted",
      interaction_capability: "interactive",
      interaction_sequence: 1,
      requires_resync: false,
    });
    const snapshot = () => {
      const debug = (window as unknown as {
        __wardianTerminalDebug?: {
          snapshot: (id: string) => { renderer?: { cols: number; rows: number } } | null;
        };
      }).__wardianTerminalDebug?.snapshot(presentationId);
      if (debug?.renderer?.cols && debug.renderer.rows) {
        geometry = { cols: debug.renderer.cols, rows: debug.renderer.rows };
      }
      return {
        snapshot_id: `seeded-${nextFixture.provider}`,
        session_id: nextFixture.session_id,
        runtime_generation: 1,
        sequence_barrier: 0,
        geometry,
        terminal_state_base64: btoa(
          String.fromCharCode(...new TextEncoder().encode(nextFixture.state)),
        ),
        visible_grid: nextFixture.state,
        scrollback: nextFixture.history,
        formatted_scrollback: nextFixture.history,
      };
    };

    runtime.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const request = args?.request;
      if (request?.presentation_id) {
        presentationId = request.presentation_id;
      }
      const requestedGeometry = request?.desired_geometry ?? request?.geometry ?? request;
      if (requestedGeometry?.cols && requestedGeometry.rows) {
        geometry = { cols: requestedGeometry.cols, rows: requestedGeometry.rows };
      }

      if (command === "register_terminal_presentation") {
        return {
          presentation: presentation(),
          broker_state: state(),
          initial_snapshot: snapshot(),
        };
      }
      if (command === "update_terminal_presentation") {
        return { presentation: presentation(), broker_state: state() };
      }
      if (command === "report_terminal_presentation_viewport") return presentation();
      if (command === "subscribe_terminal_events") {
        return { broker_state: state(), initial_snapshot: snapshot() };
      }
      if (command === "request_terminal_snapshot") return snapshot();
      if (command === "begin_terminal_activation") {
        return {
          decision: decision(),
          activation_id: "seeded-activation",
          snapshot: snapshot(),
          sequence_barrier: 0,
        };
      }
      if (command === "ack_terminal_activation") {
        owner = presentationId;
        return { decision: decision(), broker_state: state(), snapshot: null };
      }
      if (command === "resize_terminal_presentation") {
        return { decision: decision(), geometry, geometry_sequence: 1, snapshot: snapshot() };
      }
      if (command === "read_terminal_events") {
        return {
          status: "caught_up",
          runtime_generation: 1,
          events: [],
          next_sequence: 0,
          available_from_sequence: 0,
          latest_sequence: 0,
          recovery_snapshot: null,
        };
      }
      if (command === "unregister_terminal_presentation") return state();
      if (command === "unsubscribe_terminal_events") return null;
      return originalInvoke(command, args);
    };
  }, fixture);
}

async function mountAndRemountTerminal(page: Page, fixture: HyperlinkFixture) {
  const document = makeWorkbenchDocument({
    surfaces: [makeWorkbenchSurface("start", "dashboard")],
  });
  const controller = await installWorkbenchIpcMock(page, {
    agents: [{
      session_id: fixture.session_id,
      session_name: fixture.session_name,
      agent_class: "Test",
      provider: fixture.provider,
      folder: "/sample-workspace",
      is_off: false,
    }],
    load_result: {
      source: "primary",
      document,
      notice: null,
      durable_revision: 0,
      durable_token: "seed",
    },
  });

  await page.goto("/");
  await expect(page.getByTestId("workbench-group")).toBeVisible();
  await installTerminalSnapshotMock(page, fixture);
  await openSurface(page, "agents-overview");

  const initialCard = visibleAgentCard(page, fixture.session_name);
  await expect(initialCard).toBeVisible();
  await expect(initialCard.locator('[data-testid="agent-terminal-host"]')).toBeVisible();

  // Switching away and opening the agent surface again forces a fresh
  // AgentTerminal/xterm renderer to consume the canonical snapshot.
  await openSurface(page, "dashboard");
  await openSurface(page, "agents-overview");

  const card = visibleAgentCard(page, fixture.session_name);
  const host = card.locator('[data-testid="agent-terminal-host"]');
  await expect(card).toBeVisible();
  await expect(host).toBeVisible();
  await expect(host.locator(".xterm-screen")).toBeVisible();

  const replay = async () => host.evaluate((element) => {
    const presentationId = element.getAttribute("data-terminal-presentation-id");
    const debug = presentationId
      ? (window as unknown as {
          __wardianTerminalDebug?: {
            snapshot: (id: string) => {
              renderer?: { allLines?: string[]; lines?: string[] };
              snapshotReplays?: { appliedFormattedState: boolean }[] | null;
            };
          };
        }).__wardianTerminalDebug?.snapshot(presentationId)
      : null;
    return {
      formatted: debug?.snapshotReplays?.some((trace) => trace.appliedFormattedState) ?? false,
      lines: debug?.renderer?.lines ?? [],
      allLines: debug?.renderer?.allLines ?? [],
    };
  });
  await expect.poll(replay, { timeout: 15_000 }).toMatchObject({ formatted: true });
  const replayState = await replay();
  expect(replayState.allLines).toEqual(expect.arrayContaining(fixture.history));

  return { controller, card, host };
}

async function terminalTextPosition(page: Page, host: Locator, text: string) {
  const layout = await host.evaluate((element, needle) => {
    const presentationId = element.getAttribute("data-terminal-presentation-id");
    if (!presentationId) throw new Error("terminal presentation id is missing");
    const debug = (window as unknown as {
      __wardianTerminalDebug?: {
        snapshot: (id: string) => {
          renderer?: {
            cols: number;
            rows: number;
            viewportY: number;
            cssCellWidth: number | null;
            cssCellHeight: number | null;
            lines: string[];
          };
        } | null;
      };
    }).__wardianTerminalDebug?.snapshot(presentationId);
    const renderer = debug?.renderer;
    const row = renderer?.lines.findIndex((line) => line.includes(needle)) ?? -1;
    if (!renderer || row < 0) {
      throw new Error(`Could not locate terminal text ${needle}: ${JSON.stringify(renderer?.lines)}`);
    }
    const col = renderer.lines[row].indexOf(needle);
    return {
      row,
      col,
      bufferLineNumber: renderer.viewportY + row + 1,
      cols: renderer.cols,
      rows: renderer.rows,
      cellWidth: renderer.cssCellWidth,
      cellHeight: renderer.cssCellHeight,
    };
  }, text);
  const screen = host.locator(".xterm-screen");
  const bounds = await screen.boundingBox();
  if (!bounds) throw new Error("xterm screen has no bounds");
  const cellWidth = layout.cellWidth ?? bounds.width / layout.cols;
  const cellHeight = layout.cellHeight ?? bounds.height / layout.rows;
  return {
    ...layout,
    x: bounds.x + (layout.col + 0.5) * cellWidth,
    y: bounds.y + (layout.row + 0.5) * cellHeight,
  };
}

async function clickTerminalText(page: Page, host: Locator, text: string) {
  const position = await terminalTextPosition(page, host, text);
  await page.mouse.click(position.x, position.y);
  return position;
}

async function openerUrls(controller: { calls: (command?: string) => Promise<MockCall[]> }) {
  const calls = await controller.calls("plugin:opener|open_url");
  return calls.map((call) => String(call.args?.url ?? ""));
}

async function captureCodexEvidenceScreenshot(card: Locator, provider: Provider) {
  if (provider !== "codex" || process.env.WARDIAN_CAPTURE_TERMINAL_HYPERLINK_SCREENSHOT !== "1") {
    return;
  }
  const directory = process.env.WARDIAN_TERMINAL_HYPERLINK_SCREENSHOT_DIR
    ?? path.join("e2e", "screenshots", "terminal-hyperlinks", new Date().toISOString().replaceAll(":", "-"));
  mkdirSync(directory, { recursive: true });
  await card.screenshot({ path: path.join(directory, "codex-osc8-after-remount.png") });
}

// Seeded browser UI evidence only. This exercises the actual AgentTerminal
// renderer and pointer path; it does not claim real-provider acceptance.
for (const provider of PROVIDERS) {
  test(`Issue #1336 ${provider} terminal OSC8 links survive snapshot remount`, async ({ page }) => {
    const fixture = makeHyperlinkFixture(provider);
    const { controller, host } = await mountAndRemountTerminal(page, fixture);

    const plainLink = await terminalTextPosition(page, host, fixture.plain_target);
    const plainLinks = await page.evaluate(
      ({ presentationId, bufferLineNumber }) => (window as unknown as {
        __wardianTerminalDebug?: {
          terminalLinks: (
            id: string,
            line: number,
          ) => Promise<{ kind: string; target: string; text: string }[] | null>;
        };
      }).__wardianTerminalDebug?.terminalLinks(presentationId, bufferLineNumber),
      {
        presentationId: await host.getAttribute("data-terminal-presentation-id"),
        bufferLineNumber: plainLink.bufferLineNumber,
      },
    );
    expect(plainLinks).toEqual(expect.arrayContaining([
      expect.objectContaining({ kind: "url", target: fixture.plain_target, text: fixture.plain_target }),
    ]));
    expect(await openerUrls(controller)).toEqual([]);

    // A real pointer click on text adjacent to the OSC8 label is the negative
    // control. It must not fall through to the opener.
    await clickTerminalText(page, host, ADJACENT_UNLINKED);
    await page.waitForTimeout(150);
    expect(await openerUrls(controller)).toEqual([]);

    await clickTerminalText(page, host, LINK_LABEL);
    await expect.poll(() => openerUrls(controller), { timeout: 5_000 }).toEqual([fixture.target]);

    await clickTerminalText(page, host, fixture.plain_target);
    await expect.poll(() => openerUrls(controller), { timeout: 5_000 })
      .toEqual([fixture.target, fixture.plain_target]);

    await captureCodexEvidenceScreenshot(visibleAgentCard(page, fixture.session_name), provider);
  });
}

test("Issue #1336 opencode terminal OSC8 link remains clickable with TUI mouse reporting", async ({ page }) => {
  const fixture = makeHyperlinkFixture("opencode", true);
  const { controller, host } = await mountAndRemountTerminal(page, fixture);

  await clickTerminalText(page, host, LINK_LABEL);
  await expect.poll(() => openerUrls(controller), { timeout: 5_000 }).toEqual([fixture.target]);
});
