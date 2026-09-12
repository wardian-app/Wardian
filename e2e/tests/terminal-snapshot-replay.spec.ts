import { expect, test } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { installWorkbenchIpcMock, makeWorkbenchDocument, makeWorkbenchSurface } from "../fixtures/workbenchIpcMock";
import { openSurface } from "../fixtures/workbench";
const fixture = JSON.parse(readFileSync("src/features/terminal/fixtures/claudeSnapshotReplay.json", "utf8")) as { history: string[]; state: string };

// Seeded IPC snapshot, actual Wardian terminal UI/xterm. No native/provider claim.
test("normal terminal surface retains snapshot history above the restored grid", async ({ page }) => {
  test.skip(process.env.VITE_WARDIAN_TERMINAL_DEBUG !== "1", "Screenshot probe requires the explicitly instrumented frontend; parser regression runs unconditionally in unit tests");
  const agentId = "snapshot-evidence";
  const document = makeWorkbenchDocument({ surfaces: [makeWorkbenchSurface("start", "dashboard")] });
  await installWorkbenchIpcMock(page, {
    agents: [{ session_id: agentId, session_name: "Snapshot replay", agent_class: "Test", provider: "claude", folder: "/sample-workspace", is_off: false }],
    load_result: { source: "primary", document, notice: null, durable_revision: 0, durable_token: "seed" },
  });
  await page.goto("/");
  await expect(page.getByTestId("workbench-group")).toBeVisible();
  await page.evaluate(({ agentId, fixture }) => {
    type Request = { presentation_id?: string; cols?: number; rows?: number };
    const runtime = window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: { request?: Request }) => Promise<unknown> } };
    const invoke = runtime.__TAURI_INTERNALS__.invoke;
    let geometry = { cols: 80, rows: 54 };
    let presentationId = "pending";
    let owner: string | null = null;
    const state = () => ({ session_id: agentId, runtime_generation: 1, lease_epoch: 1, stream_sequence: 0,
      interaction_sequence: 0, geometry, owner_presentation_id: owner, pending_activation: null, runtime_state: "live" });
    const decision = () => ({ status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 1, owner_presentation_id: owner });
    const presentation = () => ({ presentation_id: presentationId, client_kind: "desktop", desired_geometry: geometry,
      visibility: "visible", render_state: "mounted", interaction_capability: "interactive", interaction_sequence: 1, requires_resync: false });
    const snapshot = () => {
      // This browser-only fixture supplies an absolute frame at the current
      // presentation geometry; the Rust broker/resize handshake is not tested.
      const renderer = (window as unknown as { __wardianTerminalDebug?: { snapshot: (id: string) => {
        renderer?: { cols: number; rows: number };
      } } }).__wardianTerminalDebug?.snapshot(presentationId)?.renderer;
      if (renderer?.cols && renderer.rows) geometry = { cols: renderer.cols, rows: renderer.rows };
      return { snapshot_id: "retained-fixture", session_id: agentId, runtime_generation: 1, sequence_barrier: 0,
      geometry, terminal_state_base64: btoa(String.fromCharCode(...new TextEncoder().encode(fixture.state))),
      visible_grid: "Seeded visible grid", scrollback: fixture.history, formatted_scrollback: fixture.history };
    };
    runtime.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const request = args?.request;
      if (request?.presentation_id) presentationId = request.presentation_id;
      if (request?.cols && request.rows) geometry = { cols: request.cols, rows: request.rows };
      if (["register_terminal_presentation", "update_terminal_presentation"].includes(command)) return { presentation: presentation(), broker_state: state(), initial_snapshot: snapshot() };
      if (command === "report_terminal_presentation_viewport") return presentation();
      if (command === "subscribe_terminal_events") return { broker_state: state(), initial_snapshot: snapshot() };
      if (command === "request_terminal_snapshot") return snapshot();
      if (command === "begin_terminal_activation") return { decision: decision(), activation_id: "seeded-activation", snapshot: snapshot(), sequence_barrier: 0 };
      if (command === "ack_terminal_activation") { owner = presentationId; return { decision: decision(), broker_state: state(), snapshot: null }; }
      if (command === "resize_terminal_presentation") return { decision: decision(), geometry, geometry_sequence: 1, snapshot: snapshot() };
      if (command === "read_terminal_events") return { status: "caught_up", runtime_generation: 1, events: [], next_sequence: 0, latest_sequence: 0, recovery_snapshot: null };
      if (command === "unregister_terminal_presentation") return state();
      if (command === "unsubscribe_terminal_events") return null;
      return invoke(command, args);
    };
  }, { agentId, fixture });
  await openSurface(page, "agents-overview");
  const card = page.locator('[data-testid="agent-card"]').filter({ hasText: "Snapshot replay" });
  await expect(card).toBeVisible();
  const terminal = card.locator(".xterm");
  await expect(terminal).toBeVisible();
  // A normal remount requests the snapshot after viewport geometry is known.
  await openSurface(page, "dashboard");
  await openSurface(page, "agents-overview");
  await terminal.click();
  await page.setViewportSize({ width: 1800, height: 1050 });
  const observed = () => card.locator("[data-terminal-presentation-id]").evaluate((element) => {
    const pid = element.getAttribute("data-terminal-presentation-id")!;
    const debug = (window as unknown as { __wardianTerminalDebug?: { snapshot: (id: string) => {
      renderer?: { allLines?: string[] }; snapshotReplays?: { appliedFormattedState: boolean }[];
    } } }).__wardianTerminalDebug?.snapshot(pid);
    return { formatted: debug?.snapshotReplays?.some((trace) => trace.appliedFormattedState) ?? false,
      numbers: (debug?.renderer?.allLines ?? []).map((line) => line.trim().replace(/^●\s*/, ""))
        .filter((line) => /^\d+$/.test(line)).map(Number) };
  });
  await expect.poll(observed).toEqual({ formatted: true, numbers: Array.from({ length: 50 }, (_, i) => i + 1) });
  await terminal.hover();
  // The terminal deliberately limits one wheel event to a few rows.
  for (let scroll = 0; scroll < 6; scroll += 1) {
    await page.mouse.wheel(0, -120);
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  }
  const visibleRows = () => card.locator("[data-terminal-presentation-id]").evaluate((element) => {
    const pid = element.getAttribute("data-terminal-presentation-id")!;
    return (window as unknown as { __wardianTerminalDebug: { snapshot: (id: string) => {
      renderer: { lines: string[] };
    } } }).__wardianTerminalDebug.snapshot(pid).renderer.lines.join("\n");
  });
  await expect.poll(visibleRows).toContain("Claude");
  await expect.poll(visibleRows).toMatch(/●\s*1/);
  const directory = path.join("e2e", "screenshots", "terminal-snapshot-replay", "seeded-current");
  mkdirSync(directory, { recursive: true });
  await card.screenshot({ path: path.join(directory, "history-restored.png") });
});
