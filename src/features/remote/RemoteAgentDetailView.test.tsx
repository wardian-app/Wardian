import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage, RemoteAgentSummary } from "../../types";
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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function chatMessage(id: string, text: string, sequence: number, sessionId = "agent-1"): AgentChatEvent {
  return {
    id, session_id: sessionId, provider: "codex", kind: "message", role: "assistant", text,
    title: null, status: null, turn_id: null, source: null, command: null, exit_code: null,
    path: null, language: null, created_at: null, sequence, metadata: {},
  };
}

function olderReadPage(overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  return {
    session_id: "agent-1", conversation_id: "saved-conversation", generation: "saved-generation",
    source_epoch: null, revision: "saved-revision", events: [], next_before: "saved-before",
    unchanged: false, reset: false, progress: "indexing", aliases: [], removed_ids: [], detail: null,
    bytes_read: 16097, records_decoded: 7,
    ...overrides,
  };
}

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
      alternate_screen: false,
      visible_grid: options.state ?? "ready",
      scrollback: [] as string[],
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
        registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
        parser: { registerCsiHandler: vi.fn(() => ({ dispose: vi.fn() })) },
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
      activeAgentId: "agent-1",
      chatLoading: false,
      chatLoadingOlder: false,
      chatBrowsingOlder: false,
      chatHasOlder: false,
      chatNextBefore: null,
      chatPage: null,
      chatError: "",
      sending: false,
      remoteTerminalFontSize: 11,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it.each(["ready", "indexing"] as const)("shows a Chat-local failure and retry without a misleading %s transcript", async (progress) => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const originalRefresh = useRemoteStore.getState().refreshActiveAgentChat;
    useRemoteStore.setState({ activeAgentViewMode: "chat", chatPage: olderReadPage({ progress, next_before: null }),
      chatError: "Chat history did not finish loading.", refreshActiveAgentChat: retry });
    try {
      render(<RemoteAgentDetailView agent={agent} />);
      expect(screen.getByRole("alert")).toHaveTextContent("Chat history did not finish loading.");
      expect(screen.queryByText("No chat transcript yet.")).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Retry Chat" }));
      expect(retry).toHaveBeenCalledTimes(1);
    } finally {
      act(() => useRemoteStore.setState({ refreshActiveAgentChat: originalRefresh }));
    }
  });

  it("shows an empty transcript after a successful ready response", () => {
    useRemoteStore.setState({ activeAgentViewMode: "chat", chatPage: olderReadPage({ progress: "ready", next_before: null }) });
    render(<RemoteAgentDetailView agent={agent} />);
    expect(screen.getByText("No chat transcript yet.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry Chat" })).not.toBeInTheDocument();
  });

  it("shows indexing progress while an empty read is unfinished", () => {
    useRemoteStore.setState({ activeAgentViewMode: "chat", chatPage: olderReadPage({ progress: "indexing", next_before: null }) });
    render(<RemoteAgentDetailView agent={agent} />);
    expect(screen.getByRole("status")).toHaveTextContent("History is updating.");
    expect(screen.queryByText("No chat transcript yet.")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("retains loaded rows and retry when Chat fails", () => {
    const recent = chatMessage("retained", "Retained reply", 1);
    useRemoteStore.setState({ activeAgentViewMode: "chat", chatEvents: [recent],
      chatPage: olderReadPage({ progress: "ready", next_before: null, events: [recent] }),
      chatError: "Chat history did not finish loading." });
    render(<RemoteAgentDetailView agent={agent} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Chat history did not finish loading.");
    expect(screen.getByText("Retained reply")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry Chat" })).toBeEnabled();
    expect(screen.queryByText("No chat transcript yet.")).not.toBeInTheDocument();
  });

  it.each(["empty indexing page", "failed read"])("preserves the prepended viewport through %s continuation", async (outcome) => {
    if (outcome === "empty indexing page") vi.useFakeTimers();
    const firstOlder = deferred<AgentChatPage>();
    const secondOlder = deferred<AgentChatPage>();
    const recent = chatMessage("recent-row", "recent row", 2);
    const load = vi.spyOn(remoteClient, "loadAgentChatPage")
      .mockReturnValueOnce(firstOlder.promise).mockReturnValueOnce(secondOlder.promise);
    useRemoteStore.setState({ status: "ready", activeAgentViewMode: "chat", chatEvents: [recent],
      chatPage: olderReadPage({ events: [recent] }), chatHasOlder: true, chatNextBefore: "saved-before" });
    render(<RemoteAgentDetailView agent={agent} />);
    const scroll = screen.getByRole("region", { name: "Coder chat" });
    const row = scroll.querySelector<HTMLElement>("[data-chat-row-key]")!;
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => ({
      top: screen.queryByText("older row") ? 350 : 50,
      bottom: screen.queryByText("older row") ? 370 : 70,
    } as DOMRect));
    scroll.scrollTop = 100;
    fireEvent.scroll(scroll);
    fireEvent.scroll(scroll);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => {
      if (outcome === "failed read") firstOlder.reject(new Error("Older read failed"));
      else firstOlder.resolve(olderReadPage());
    });
    expect(scroll.scrollTop).toBe(100);
    if (outcome === "failed read") expect(screen.getByRole("alert")).toHaveTextContent("Chat could not be loaded.");
    if (outcome === "empty indexing page") {
      expect(useRemoteStore.getState().chatLoadingOlder).toBe(true);
      await act(async () => { await vi.advanceTimersByTimeAsync(750); });
    } else fireEvent.scroll(scroll);
    expect(load).toHaveBeenCalledTimes(2);
    expect(load.mock.calls[1]?.slice(0, 2)).toEqual(["agent-1", "saved-before"]);
    await act(async () => secondOlder.resolve(olderReadPage({
      events: [chatMessage("older-row", "older row", 1)], next_before: null, progress: "ready",
    })));
    expect(screen.getByText("older row")).toBeInTheDocument();
    expect(scroll.scrollTop).toBe(400);
    vi.useRealTimers();
  });

  it("allows another older scroll after an inactive-agent attempt skips without a loading transition", async () => {
    const recent = chatMessage("recent-row", "recent row", 2);
    const load = vi.spyOn(remoteClient, "loadAgentChatPage").mockResolvedValue(olderReadPage({
      events: [chatMessage("older-row", "older row", 1)], next_before: null,
    }));
    useRemoteStore.setState({ activeAgentViewMode: "chat", activeAgentId: null, chatEvents: [recent],
      chatPage: olderReadPage({ events: [recent] }), chatHasOlder: true, chatNextBefore: "saved-before" });
    render(<RemoteAgentDetailView agent={agent} />);
    const scroll = screen.getByRole("region", { name: "Coder chat" });
    scroll.scrollTop = 100;
    await act(async () => { fireEvent.scroll(scroll); });
    expect(load).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Load older transcript" })).toBeEnabled();
    act(() => useRemoteStore.setState({ activeAgentId: "agent-1" }));
    await act(async () => { fireEvent.scroll(scroll); });
    expect(load).toHaveBeenCalledOnce();
    expect(screen.getByText("older row")).toBeInTheDocument();
  });

  it("ignores an old agent's older completion while the new agent is preserving its viewport", async () => {
    const firstOlder = deferred<AgentChatPage>();
    const secondOlder = deferred<AgentChatPage>();
    const load = vi.spyOn(remoteClient, "loadAgentChatPage")
      .mockReturnValueOnce(firstOlder.promise).mockReturnValueOnce(secondOlder.promise);
    const recent = chatMessage("recent-row", "recent row", 2);
    useRemoteStore.setState({ activeAgentViewMode: "chat", chatEvents: [recent],
      chatPage: olderReadPage({ events: [recent] }), chatHasOlder: true, chatNextBefore: "saved-before" });
    const { rerender } = render(<RemoteAgentDetailView agent={agent} />);
    fireEvent.scroll(screen.getByRole("region", { name: "Coder chat" }));
    const nextRecent = chatMessage("next-recent", "next recent", 2, "agent-2");
    act(() => useRemoteStore.setState({ activeAgentId: "agent-2", chatLoadingOlder: false, chatEvents: [nextRecent],
      chatPage: olderReadPage({ session_id: "agent-2", events: [nextRecent] }) }));
    rerender(<RemoteAgentDetailView agent={{ ...agent, session_id: "agent-2", session_name: "Next" }} />);
    const scroll = screen.getByRole("region", { name: "Next chat" });
    const row = scroll.querySelector<HTMLElement>("[data-chat-row-key]")!;
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => ({
      top: screen.queryByText("next older") ? 350 : 50,
      bottom: screen.queryByText("next older") ? 370 : 70,
    } as DOMRect));
    scroll.scrollTop = 100;
    fireEvent.scroll(scroll);
    await act(async () => firstOlder.resolve(olderReadPage()));
    expect(scroll.scrollTop).toBe(100);
    fireEvent.scroll(scroll);
    expect(load).toHaveBeenCalledTimes(2);
    await act(async () => secondOlder.resolve(olderReadPage({
      session_id: "agent-2", next_before: null, events: [chatMessage("next-older", "next older", 1, "agent-2")],
    })));
    expect(screen.getByText("next older")).toBeInTheDocument();
    expect(scroll.scrollTop).toBe(400);
  });

  it("uses the desktop Antigravity terminal palette and contrast floor on mobile", async () => {
    document.documentElement.style.setProperty("--color-wardian-card", "#1a1a1a");
    document.documentElement.style.setProperty("--color-wardian-text", "#ebebeb");
    const antigravityAgent = { ...agent, provider: "antigravity" as const };
    vi.spyOn(remoteClient, "openTerminalStream").mockResolvedValue(new DetailSocket() as unknown as WebSocket);

    render(<RemoteAgentDetailView agent={antigravityAgent} />);

    await waitFor(() => expect(vi.mocked(Terminal)).toHaveBeenCalled());
    const terminalCalls = vi.mocked(Terminal).mock.calls;
    const terminalOptions = terminalCalls[terminalCalls.length - 1]?.[0] as {
      minimumContrastRatio?: number;
      theme?: { background?: string; foreground?: string };
    };
    expect(terminalOptions.minimumContrastRatio).toBe(7);
    expect(terminalOptions.theme).toMatchObject({
      background: "#1a1a1a",
      foreground: "#c9d1d9",
    });
    expect(terminalOptions).toMatchObject({
      cursorBlink: true,
      cursorStyle: "bar",
      cursorInactiveStyle: "bar",
    });
    const terminal = vi.mocked(Terminal).mock.results[vi.mocked(Terminal).mock.results.length - 1]?.value as Terminal & {
      parser: { registerCsiHandler: ReturnType<typeof vi.fn> };
    };
    expect(terminal.parser.registerCsiHandler).toHaveBeenCalledWith(
      { intermediates: " ", final: "q" },
      expect.any(Function),
    );
  });

  it("implicitly requests terminal ownership when the terminal view opens", async () => {
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

    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminalInstance = terminalResults[terminalResults.length - 1]?.value as Terminal & {
      options: { disableStdin?: boolean };
    };
    const fitResults = vi.mocked(FitAddon).mock.results;
    const fit = (fitResults[fitResults.length - 1]?.value as { fit: ReturnType<typeof vi.fn> }).fit;
    const fitCallsBeforeActivation = fit.mock.calls.length;
    expect(terminalInstance.write).toHaveBeenCalledWith("ready", expect.any(Function));
    expect(terminalInstance.options.disableStdin).toBe(true);
    expect(screen.queryByText("Mirror")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Take terminal control" })).not.toBeInTheDocument();
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

  it.each([
    { alternate: false, expected: "older line\r\nnewer line\r\nvisible row" },
    { alternate: true, expected: "\x1b[?1049holder line\r\nnewer line\r\nvisible row" },
  ])("renders bounded text fallback with alternate screen $alternate", async ({ alternate, expected }) => {
    const socket = new DetailSocket();
    let handlers: Parameters<typeof remoteClient.openTerminalStream>[3] | undefined;
    vi.spyOn(remoteClient, "openTerminalStream").mockImplementation(async (_session, _cols, _rows, nextHandlers) => {
      handlers = nextHandlers;
      nextHandlers.onSocket?.(socket as unknown as WebSocket);
      return socket as unknown as WebSocket;
    });

    render(<RemoteAgentDetailView agent={agent} />);
    await waitFor(() => expect(handlers).toBeDefined());
    const message = registered();
    message.initial_snapshot.terminal_state_base64 = "";
    message.initial_snapshot.alternate_screen = alternate;
    message.initial_snapshot.scrollback = ["older line", "newer line"];
    message.initial_snapshot.visible_grid = "visible row";

    await act(async () => {
      await handlers?.onMessage(message);
    });

    const terminalResults = vi.mocked(Terminal).mock.results;
    const terminal = terminalResults[terminalResults.length - 1]?.value as Terminal;
    expect(terminal.write).toHaveBeenCalledWith(
      expected,
      expect.any(Function),
    );
  });

  it("resets pending Codex SGR before a remote authoritative snapshot", async () => {
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

    const terminal = vi.mocked(Terminal).mock.results[vi.mocked(Terminal).mock.results.length - 1]?.value as Terminal & {
      write: ReturnType<typeof vi.fn>;
    };
    terminal.write.mockClear();
    const partialSgr = "\u001b[48;2;41";
    await act(async () => {
      await handlers?.onMessage({
        type: "events",
        batch: {
          status: "events",
          runtime_generation: 1,
          events: [{
            type: "output",
            sequence: 5,
            runtime_generation: 1,
            bytes_base64: btoa(partialSgr),
          }],
          next_sequence: 5,
          available_from_sequence: 5,
          latest_sequence: 5,
          recovery_snapshot: null,
        },
      });
    });
    expect(terminal.write).toHaveBeenCalledWith("", expect.any(Function));
    terminal.write.mockClear();

    const restoredText = "fresh remote generation snapshot";
    await act(async () => {
      await handlers?.onMessage({
        type: "snapshot",
        snapshot: {
          ...registered().initial_snapshot,
          snapshot_id: "snapshot-2",
          runtime_generation: 2,
          sequence_barrier: 6,
          terminal_state_base64: btoa(restoredText),
          visible_grid: restoredText,
        },
      });
    });

    expect(terminal.write).toHaveBeenLastCalledWith(restoredText, expect.any(Function));
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
        registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
        parser: { registerCsiHandler: vi.fn(() => ({ dispose: vi.fn() })) },
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
