import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteAgentSummary } from "../../types";
import { RemoteAgentDetailView } from "./RemoteAgentDetailView";
import { remoteClient } from "./remoteClient";
import { useRemoteStore } from "./useRemoteStore";

const agent: RemoteAgentSummary = {
  session_id: "agent-1",
  session_name: "Coder",
  agent_class: "Coder",
  provider: "codex",
  workspace: "<absolute-workspace-path>",
  status: "Idle",
  latest_text: null,
};

class DetailSocket {
  readyState = WebSocket.OPEN;
  sent: string[] = [];
  close = vi.fn();

  send(payload: string) {
    this.sent.push(payload);
  }
}

function registered(options: { owner?: boolean; requiresResync?: boolean; state?: string } = {}) {
  return {
    type: "registered" as const,
    protocol_version: 2 as const,
    presentation: {
      presentation_id: "remote:presentation-1",
      client_kind: "remote" as const,
      desired_geometry: { cols: 80, rows: 24 },
      visibility: "visible" as const,
      render_state: "mounted" as const,
      interaction_capability: "interactive" as const,
      interaction_sequence: 1,
      requires_resync: options.requiresResync ?? false,
    },
    broker_state: {
      session_id: "agent-1",
      runtime_generation: 1,
      lease_epoch: 3,
      stream_sequence: 4,
      interaction_sequence: 1,
      geometry: { cols: 80, rows: 24 },
      owner_presentation_id: options.owner ? "remote:presentation-1" : "desktop:presentation-1",
      pending_activation: null,
      runtime_state: "live" as const,
    },
    initial_snapshot: {
      snapshot_id: "snapshot-1",
      session_id: "agent-1",
      runtime_generation: 1,
      sequence_barrier: 4,
      geometry: { cols: 80, rows: 24 },
      terminal_state_base64: btoa(options.state ?? "ready"),
      visible_grid: options.state ?? "ready",
      scrollback: [],
    },
  };
}

describe("RemoteAgentDetailView terminal protocol v2", () => {
  beforeEach(() => {
    Object.defineProperty(Element.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
    vi.mocked(Terminal).mockImplementation(function MockTerminal(options) {
      return {
        open: vi.fn(),
        write: vi.fn((_data: string | Uint8Array, callback?: () => void) => callback?.()),
        resize: vi.fn(),
        onData: vi.fn(),
        onBinary: vi.fn(),
        reset: vi.fn(),
        dispose: vi.fn(),
        attachCustomKeyEventHandler: vi.fn(),
        loadAddon: vi.fn(),
        textarea: document.createElement("textarea"),
        options: { ...(options ?? {}) },
        cols: 80,
        rows: 24,
      } as unknown as Terminal;
    });
    useRemoteStore.setState({
      activeAgentViewMode: "terminal",
      terminalLoading: false,
      terminalError: "",
      chatEvents: [],
      chatLoading: false,
      chatError: "",
      sending: false,
      remoteTerminalFontSize: 11,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("shows a passive Mirror and explicitly requests ownership before enabling input", async () => {
    const socket = new DetailSocket();
    let handlers: Parameters<typeof remoteClient.openTerminalStream>[3] | undefined;
    vi.spyOn(remoteClient, "openTerminalStream").mockImplementation(async (_session, _cols, _rows, nextHandlers) => {
      handlers = nextHandlers;
      nextHandlers.onSocket?.(socket as unknown as WebSocket);
      return socket as unknown as WebSocket;
    });

    render(<RemoteAgentDetailView agent={agent} />);
    await waitFor(() => expect(handlers).toBeDefined());
    await act(async () => {
      await handlers?.onMessage(registered());
    });

    expect(screen.getByTestId("remote-terminal-presentation-mode")).toHaveTextContent("Mirror");
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as Terminal & {
      options: { disableStdin?: boolean };
    };
    const fitResults = vi.mocked(FitAddon).mock.results;
    const fit = (fitResults[fitResults.length - 1]?.value as { fit: ReturnType<typeof vi.fn> }).fit;
    const fitCallsBeforeActivation = fit.mock.calls.length;
    expect(terminalInstance.options.disableStdin).toBe(true);

    await userEvent.click(screen.getByRole("button", { name: "Take terminal control" }));
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "begin_activation",
      runtime_generation: 1,
      observed_lease_epoch: 3,
    });

    await act(async () => {
      await handlers?.onMessage({
        type: "activation_begin",
        result: {
          decision: {
            status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 4,
            owner_presentation_id: "desktop:presentation-1",
          },
          activation_id: "activation-1",
          snapshot: registered().initial_snapshot,
          sequence_barrier: 4,
        },
      });
      await handlers?.onMessage({
        type: "activation_ack",
        result: {
          decision: {
            status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 4,
            owner_presentation_id: "remote:presentation-1",
          },
          broker_state: { ...registered({ owner: true }).broker_state, lease_epoch: 4 },
          snapshot: null,
        },
      });
    });
    expect(fit.mock.calls.length).toBeGreaterThan(fitCallsBeforeActivation);
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "resize", runtime_generation: 1, lease_epoch: 4, geometry_sequence: 1, cols: 80, rows: 24,
    });
  });

  it("detaches and closes a socket that races unmount cleanup", async () => {
    const socket = new DetailSocket();
    vi.spyOn(remoteClient, "openTerminalStream").mockImplementation(async (_session, _cols, _rows, handlers) => {
      handlers.onSocket?.(socket as unknown as WebSocket);
      return socket as unknown as WebSocket;
    });

    const view = render(<RemoteAgentDetailView agent={agent} />);
    await waitFor(() => expect(remoteClient.openTerminalStream).toHaveBeenCalled());
    view.unmount();

    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({ type: "detach" });
    expect(socket.close).toHaveBeenCalled();
  });

  it("keeps resyncing owner stdin disabled and flushes buffered capability replies after ack", async () => {
    const socket = new DetailSocket();
    let handlers: Parameters<typeof remoteClient.openTerminalStream>[3] | undefined;
    vi.spyOn(remoteClient, "openTerminalStream").mockImplementation(async (_session, _cols, _rows, nextHandlers) => {
      handlers = nextHandlers;
      nextHandlers.onSocket?.(socket as unknown as WebSocket);
      return socket as unknown as WebSocket;
    });
    render(<RemoteAgentDetailView agent={{ ...agent, provider: "opencode" }} />);
    await waitFor(() => expect(handlers).toBeDefined());

    await act(async () => {
      await handlers?.onMessage(registered({ owner: true, requiresResync: true, state: "\u001b[6n" }));
    });
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminal = terminalResults[terminalResults.length - 1]?.value as Terminal & {
      options: { disableStdin?: boolean };
    };
    expect(terminal.options.disableStdin).toBe(true);
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "begin_owner_resync", runtime_generation: 1, lease_epoch: 3,
    });
    expect(socket.sent.map((payload) => JSON.parse(payload)).filter((message) => message.type === "input")).toEqual([]);

    await act(async () => {
      await handlers?.onMessage({
        type: "owner_resync_begin",
        result: {
          decision: {
            status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 3,
            owner_presentation_id: "remote:presentation-1",
          },
          resync_id: "resync-1",
          snapshot: registered({ owner: true }).initial_snapshot,
          sequence_barrier: 4,
        },
      });
      await handlers?.onMessage({
        type: "owner_resync_ack",
        result: {
          decision: {
            status: "accepted", reason: null, runtime_generation: 1, lease_epoch: 3,
            owner_presentation_id: "remote:presentation-1",
          },
          broker_state: registered({ owner: true }).broker_state,
        },
      });
    });

    expect(terminal.options.disableStdin).toBe(false);
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "input", runtime_generation: 1, lease_epoch: 3, data: "\u001b[1;1R",
    });
  });

  it("keeps mirror xterm geometry canonical while portrait and landscape viewports only report proposals", async () => {
    vi.mocked(Terminal).mockImplementation(function MockTerminal(options) {
      return {
        open: vi.fn(),
        write: vi.fn((_data: string | Uint8Array, callback?: () => void) => callback?.()),
        resize: vi.fn(function resize(this: { cols: number; rows: number }, cols: number, rows: number) {
          this.cols = cols;
          this.rows = rows;
        }),
        onData: vi.fn(),
        onBinary: vi.fn(),
        reset: vi.fn(),
        dispose: vi.fn(),
        attachCustomKeyEventHandler: vi.fn(),
        loadAddon: vi.fn(),
        textarea: document.createElement("textarea"),
        options: { ...(options ?? {}) },
        cols: 80,
        rows: 24,
        _core: { _renderService: { dimensions: { css: { cell: { width: 10, height: 20 } } } } },
      } as unknown as Terminal;
    });
    let resizeCallback: ResizeObserverCallback | undefined;
    vi.stubGlobal("ResizeObserver", class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    });
    const socket = new DetailSocket();
    let handlers: Parameters<typeof remoteClient.openTerminalStream>[3] | undefined;
    vi.spyOn(remoteClient, "openTerminalStream").mockImplementation(async (_session, _cols, _rows, nextHandlers) => {
      handlers = nextHandlers;
      nextHandlers.onSocket?.(socket as unknown as WebSocket);
      return socket as unknown as WebSocket;
    });

    render(<RemoteAgentDetailView agent={agent} />);
    const surface = await screen.findByTestId("remote-terminal-scroll-surface");
    const host = screen.getByTestId("remote-terminal-attach");
    vi.spyOn(surface, "getBoundingClientRect").mockReturnValue({
      width: 320, height: 640, top: 0, left: 0, right: 320, bottom: 640, x: 0, y: 0, toJSON: () => ({}),
    });
    await waitFor(() => expect(handlers).toBeDefined());
    await act(async () => {
      await handlers?.onMessage(registered());
    });
    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminal = terminalResults[terminalResults.length - 1]?.value as Terminal;
    const fitResults = vi.mocked(FitAddon).mock.results;
    const fit = (fitResults[fitResults.length - 1]?.value as { fit: ReturnType<typeof vi.fn> }).fit;
    const fitCallsAfterOpen = fit.mock.calls.length;

    expect(terminal.cols).toBe(80);
    expect(terminal.rows).toBe(24);
    expect(host.style.transform).toBe("translate(0px, 140px) scale(0.75)");
    expect(surface.style.overflowX).toBe("auto");
    expect(surface.style.overflowY).toBe("hidden");
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "report_viewport", runtime_generation: 1, cols: 32, rows: 32,
    });

    vi.mocked(surface.getBoundingClientRect).mockReturnValue({
      width: 640, height: 320, top: 0, left: 0, right: 640, bottom: 320, x: 0, y: 0, toJSON: () => ({}),
    });
    act(() => resizeCallback?.([], {} as ResizeObserver));

    expect(terminal.cols).toBe(80);
    expect(terminal.rows).toBe(24);
    expect(host.style.transform).toBe("translate(20px, 0px) scale(0.75)");
    expect(surface.style.overflowX).toBe("hidden");
    expect(surface.style.overflowY).toBe("auto");
    expect(fit).toHaveBeenCalledTimes(fitCallsAfterOpen);
    expect(socket.sent.map((payload) => JSON.parse(payload))).toContainEqual({
      type: "report_viewport", runtime_generation: 1, cols: 64, rows: 16,
    });
  });
});
