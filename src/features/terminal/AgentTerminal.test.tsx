import { render, waitFor, cleanup, act, screen, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { Terminal as HeadlessTerminal } from "@xterm/headless";
import { SerializeAddon } from "@xterm/addon-serialize";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ComponentProps } from "react";
import {
  AgentTerminal as BrokerAgentTerminal,
  __terminalTesting,
  shouldExposeTerminalDebug,
} from "./AgentTerminal";
import { defaultTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";
import { useQueueStore } from "../../store/useQueueStore";
import { resetTerminalSessionClientsForTesting } from "./terminalSessionClient";
import { terminalRendererBudget } from "./terminalRendererBudget";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockTerminal = vi.mocked(Terminal);
const mockHeadlessTerminal = vi.mocked(HeadlessTerminal);
const mockSerializeAddon = vi.mocked(SerializeAddon);
const mockFitAddon = vi.mocked(FitAddon);
const mockWebglAddon = vi.mocked(WebglAddon);

type TestAgentTerminalProps = Omit<
  ComponentProps<typeof BrokerAgentTerminal>,
  "presentationId" | "visibility" | "renderState" | "requestedInteraction"
> &
  Partial<
    Pick<
      ComponentProps<typeof BrokerAgentTerminal>,
      "presentationId" | "visibility" | "renderState" | "requestedInteraction"
    >
  >;

function AgentTerminal({
  sessionId,
  presentationId = sessionId,
  visibility = "visible",
  renderState = "mounted",
  requestedInteraction = "interactive",
  ...props
}: TestAgentTerminalProps) {
  return (
    <BrokerAgentTerminal
      {...props}
      sessionId={sessionId}
      presentationId={presentationId}
      visibility={visibility}
      renderState={renderState}
      requestedInteraction={requestedInteraction}
    />
  );
}

function getLatestTerminalInstance() {
  return mockTerminal.mock.results[mockTerminal.mock.results.length - 1]?.value as any;
}

function getLatestHeadlessTerminalInstance() {
  return mockHeadlessTerminal.mock.results[mockHeadlessTerminal.mock.results.length - 1]?.value as any;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function modernBrokerState(ownerPresentationId: string | null = null) {
  return {
    session_id: "modern-agent",
    runtime_generation: 1,
    lease_epoch: ownerPresentationId ? 1 : 0,
    stream_sequence: 0,
    interaction_sequence: 0,
    geometry: { cols: 80, rows: 24 },
    owner_presentation_id: ownerPresentationId,
    pending_activation: null,
    runtime_state: "live" as const,
  };
}

function modernSnapshot() {
  return {
    snapshot_id: "modern-snapshot",
    session_id: "modern-agent",
    runtime_generation: 1,
    sequence_barrier: 0,
    geometry: { cols: 80, rows: 24 },
    terminal_state_base64: "",
    visible_grid: "",
    scrollback: [],
  };
}

function modernRegistrationResult(presentationId: string, ownerPresentationId: string | null = null) {
  return {
    presentation: {
      presentation_id: presentationId,
      client_kind: "desktop" as const,
      desired_geometry: { cols: 80, rows: 24 },
      visibility: "visible" as const,
      render_state: "mounted" as const,
      interaction_capability: "interactive" as const,
      interaction_sequence: 1,
      requires_resync: false,
    },
    broker_state: modernBrokerState(ownerPresentationId),
    initial_snapshot: modernSnapshot(),
  };
}

function modernCaughtUpBatch() {
  return {
    status: "caught_up" as const,
    runtime_generation: 1,
    events: [],
    next_sequence: 0,
    latest_sequence: 0,
    recovery_snapshot: null,
  };
}

describe("AgentTerminal scrollback", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>;
  let fitDimensions: { cols: number; rows: number };
  let openConnectedStates: boolean[];

  beforeEach(() => {
    vi.clearAllMocks();
    useSettingsStore.setState({ terminalFontSize: 14, terminalFontFamily: "" });
    useQueueStore.setState({ items: [], _agentBuffers: {}, _workflowLastOutput: {} });
    openConnectedStates = [];
    rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        width: 900,
        height: 600,
        top: 0,
        left: 0,
        right: 900,
        bottom: 600,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);


    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0 ? "hello from codex\n" : null;
        case "resize_agent_terminal":
          return null;
        case "terminal_link_target_exists":
          return true;
        default:
          return null;
      }
    });

    mockListen.mockResolvedValue(() => {});
    fitDimensions = { cols: 80, rows: 24 };

    mockTerminal.mockImplementation(function MockTerminal(options?: ConstructorParameters<typeof Terminal>[0]) {
      const state = { serializedState: "" };
      let resizeHandler: ((size: { cols: number; rows: number }) => void) | undefined;
      let scrollHandler: ((position: number) => void) | undefined;
      const terminal = {
        element: undefined as HTMLElement | undefined,
        open: vi.fn((element: HTMLElement) => {
          openConnectedStates.push(element.isConnected);
          terminal.element = element;
        }),
        write: vi.fn((data: string, callback?: () => void) => {
          state.serializedState += data;
          callback?.();
        }),
        resize: vi.fn((cols: number, rows: number) => {
          terminal.cols = cols;
          terminal.rows = rows;
          resizeHandler?.({ cols, rows });
        }),
        clear: vi.fn(),
        onData: vi.fn(),
        onBinary: vi.fn(),
        onTitleChange: vi.fn(),
        onResize: vi.fn((handler: (size: { cols: number; rows: number }) => void) => {
          resizeHandler = handler;
        }),
        onScroll: vi.fn((handler: (position: number) => void) => {
          scrollHandler = handler;
        }),
        reset: vi.fn(),
        dispose: vi.fn(),
        focus: vi.fn(),
        attachCustomKeyEventHandler: vi.fn(),
        registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
        selectAll: vi.fn(),
        scrollToBottom: vi.fn(),
        scrollToLine: vi.fn((line: number) => {
          terminal.buffer.active.viewportY = line;
        }),
        scrollLines: vi.fn((lines: number) => {
          terminal.buffer.active.viewportY = Math.max(
            0,
            Math.min(terminal.buffer.active.baseY, terminal.buffer.active.viewportY + lines),
          );
        }),
        scrollToTop: vi.fn(() => {
          terminal.buffer.active.viewportY = 0;
        }),
        buffer: {
          active: {
            baseY: 10,
            viewportY: 10,
            getLine: vi.fn(() => ({ translateToString: () => "src/App.tsx:12" })),
          },
        },
        refresh: vi.fn(),
        cols: 80,
        rows: 24,
        options: { ...(options ?? {}) },
        unicode: { activeVersion: "" },
        loadAddon: vi.fn((addon: { __termState?: typeof state }) => {
          if (addon && "__termState" in addon) {
            addon.__termState = state;
          }
        }),
        __emitScroll: (position: number) => scrollHandler?.(position),
      } as any;
      return terminal;
    });

    mockHeadlessTerminal.mockImplementation(function MockHeadlessTerminal(
      options?: ConstructorParameters<typeof HeadlessTerminal>[0],
    ) {
      const terminal = {
        open: vi.fn(),
        write: vi.fn((_data: string, callback?: () => void) => callback?.()),
        loadAddon: vi.fn(),
        dispose: vi.fn(),
        resize: vi.fn(),
        onData: vi.fn(),
        onBinary: vi.fn(),
        onTitleChange: vi.fn(),
        onResize: vi.fn(),
        onScroll: vi.fn(),
        scrollToTop: vi.fn(),
        scrollToLine: vi.fn((line: number) => {
          terminal.buffer.active.viewportY = line;
        }),
        buffer: {
          active: {
            cursorX: 0,
            cursorY: 0,
            baseY: 0,
            viewportY: 0,
          },
        },
        options: { ...(options ?? {}) },
        cols: options?.cols ?? 80,
        rows: options?.rows ?? 24,
      } as any;
      return terminal;
    });

    mockSerializeAddon.mockImplementation(function MockSerializeAddon() {
      return {
        __termState: undefined as { serializedState: string } | undefined,
        serialize: vi.fn(function (this: { __termState?: { serializedState: string } }) {
          return this.__termState?.serializedState ?? "";
        }),
        dispose: vi.fn(),
      } as any;
    });

    mockFitAddon.mockImplementation(function MockFitAddon() {
      return {
        fit: vi.fn(),
        proposeDimensions: vi.fn(() => fitDimensions),
        dispose: vi.fn(),
      } as any;
    });

    mockWebglAddon.mockImplementation(function MockWebglAddon() {
      return {
        onContextLoss: vi.fn(),
        clearTextureAtlas: vi.fn(),
        dispose: vi.fn(),
      } as any;
    });
  });

  afterEach(async () => {
    rectSpy.mockRestore();
    cleanup();
    await resetTerminalSessionClientsForTesting();
    terminalRendererBudget.clear();
  });

  it("only exposes terminal debug hooks in dev or with an explicit debug flag", () => {
    expect(shouldExposeTerminalDebug({ DEV: false, VITE_WARDIAN_TERMINAL_DEBUG: undefined })).toBe(false);
    expect(shouldExposeTerminalDebug({ DEV: true, VITE_WARDIAN_TERMINAL_DEBUG: undefined })).toBe(true);
    expect(shouldExposeTerminalDebug({ DEV: false, VITE_WARDIAN_TERMINAL_DEBUG: "1" })).toBe(true);
  });

  it("renders two independent broker presentations with one desktop feed consumer", async () => {
    const onPresentationStateChange = vi.fn();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(request?.presentation_id ?? "missing");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(request?.presentation_id ?? "missing").presentation;
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(request?.presentation_id ?? "missing");
      }
      if (command === "request_terminal_snapshot") return modernSnapshot();
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      if (command === "terminal_link_target_exists") {
        return true;
      }
      return null;
    });

    const view = render(
      <div>
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-a"
          provider="codex"
          theme="dark"
          onPresentationStateChange={onPresentationStateChange}
        />
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-b"
          provider="codex"
          theme="dark"
        />
      </div>,
    );

    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.filter(([command]) => command === "register_terminal_presentation"),
      ).toHaveLength(2);
    });
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "subscribe_terminal_events"),
    ).toHaveLength(1);
    expect(mockTerminal).toHaveBeenCalledTimes(2);
    const terminalHosts = screen.getAllByTestId("agent-terminal-host");
    expect(terminalHosts).toHaveLength(2);
    expect(terminalHosts[0]).toHaveAttribute("data-terminal-presentation-id", "pane-a");
    expect(terminalHosts[0]).toHaveAttribute("data-terminal-session-id", "modern-agent");
    expect(terminalHosts[1]).toHaveAttribute("data-terminal-presentation-id", "pane-b");
    expect(terminalHosts[1]).toHaveAttribute("data-terminal-session-id", "modern-agent");
    expect(window.__wardianTerminalDebug?.presentationIds()).toEqual(
      expect.arrayContaining(["pane-a", "pane-b"]),
    );
    expect(window.__wardianTerminalDebug?.snapshot("pane-a")?.renderer).toBeTruthy();
    expect(window.__wardianTerminalDebug?.snapshot("pane-b")?.renderer).toBeTruthy();
    expect(window.__wardianTerminalDebug?.snapshot("pane-a")?.broker).toEqual(
      expect.objectContaining({ ownerPresentationId: null, runtimeGeneration: 1 }),
    );
    expect(window.__wardianTerminalDebug?.snapshot("modern-agent")).toBeNull();
    expect(onPresentationStateChange).toHaveBeenCalledWith(
      expect.objectContaining({ session_id: "modern-agent" }),
      expect.objectContaining({ presentation_id: "pane-a" }),
    );

    const updatedObserver = vi.fn();
    view.rerender(
      <div>
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-a"
          provider="codex"
          theme="dark"
          workspacePath="/next-worktree"
          onPresentationStateChange={updatedObserver}
        />
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-b"
          provider="codex"
          theme="dark"
        />
      </div>,
    );
    await waitFor(() => expect(updatedObserver).toHaveBeenCalledWith(
      expect.objectContaining({ session_id: "modern-agent" }),
      expect.objectContaining({ presentation_id: "pane-a" }),
    ));
  });

  it("keeps the broker presentation registered while its viewport renderer is suspended", async () => {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "pane-viewport";
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "request_terminal_snapshot") return modernSnapshot();
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId).presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    const view = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-viewport"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));
    mockInvoke.mockClear();

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-viewport"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="dark"
      />,
    );
    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-viewport"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "request_terminal_snapshot",
      expect.anything(),
    ));
    expect(mockInvoke).not.toHaveBeenCalledWith("register_terminal_presentation", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("unregister_terminal_presentation", expect.anything());
  });

  it("retains a hidden mounted renderer without registering or snapshotting again", async () => {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "pane-retained";
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "request_terminal_snapshot") return modernSnapshot();
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId).presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    const view = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-retained"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
    });
    const renderer = getLatestTerminalInstance();
    const registrationCount = mockInvoke.mock.calls.filter(
      ([command]) => command === "register_terminal_presentation",
    ).length;
    const snapshotCount = mockInvoke.mock.calls.filter(
      ([command]) => command === "request_terminal_snapshot",
    ).length;

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-retained"
        visibility="hidden"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "hidden" });
      expect(mockInvoke).toHaveBeenCalledWith(
        "update_terminal_presentation",
        expect.objectContaining({
          request: expect.objectContaining({
            presentation_id: "pane-retained",
            visibility: "hidden",
            render_state: "mounted",
          }),
        }),
      );
    });
    expect(getLatestTerminalInstance()).toBe(renderer);
    expect(renderer.dispose).not.toHaveBeenCalled();

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-retained"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
    });
    expect(getLatestTerminalInstance()).toBe(renderer);
    expect(renderer.dispose).not.toHaveBeenCalled();
    expect(mockInvoke.mock.calls.filter(
      ([command]) => command === "register_terminal_presentation",
    )).toHaveLength(registrationCount);
    expect(mockInvoke.mock.calls.filter(
      ([command]) => command === "request_terminal_snapshot",
    )).toHaveLength(snapshotCount);
  });

  it("settles an in-flight broker snapshot write before disposing its retired renderer", async () => {
    const registrationGate = deferred<ReturnType<typeof modernRegistrationResult>>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "pane-snapshot-retirement";
      if (command === "register_terminal_presentation") {
        return registrationGate.promise;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const view = render(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-snapshot-retirement"
          provider="codex"
          theme="dark"
        />,
      );
      await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
        "register_terminal_presentation",
        expect.anything(),
      ));

      const renderer = getLatestTerminalInstance();
      const lifecycle: string[] = [];
      let settleRendererWrite: (() => void) | undefined;
      renderer.write.mockImplementation((data: string, callback?: () => void) => {
        if (data.includes("delayed broker snapshot")) {
          lifecycle.push("write-started");
          settleRendererWrite = () => {
            lifecycle.push("write-settled");
            callback?.();
          };
          return;
        }
        callback?.();
      });
      renderer.dispose.mockImplementation(() => lifecycle.push("disposed"));

      const registration = modernRegistrationResult("pane-snapshot-retirement");
      registration.initial_snapshot = {
        ...modernSnapshot(),
        visible_grid: "delayed broker snapshot",
      };
      await act(async () => {
        registrationGate.resolve(registration);
        await Promise.resolve();
      });
      await waitFor(() => expect(settleRendererWrite).toBeTypeOf("function"));

      view.rerender(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-snapshot-retirement"
          visibility="hidden"
          renderState="suspended"
          provider="codex"
          theme="dark"
        />,
      );

      await act(async () => {
        settleRendererWrite?.();
        await Promise.resolve();
      });
      await waitFor(() => expect(renderer.dispose).toHaveBeenCalledTimes(1));

      expect(screen.queryByText("Terminal Initialization Fatal Error:")).not.toBeInTheDocument();
      expect(lifecycle).toEqual(["write-started", "write-settled", "disposed"]);
      expect(consoleError).not.toHaveBeenCalledWith(
        "AgentTerminal Init Error:",
        expect.anything(),
      );
    } finally {
      consoleError.mockRestore();
    }
  });

  it("keeps post-write scroll inside the renderer lease when retirement wins the outer continuation", async () => {
    const registrationGate = deferred<ReturnType<typeof modernRegistrationResult>>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "pane-post-write-retirement";
      if (command === "register_terminal_presentation") {
        return registrationGate.promise;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const view = render(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="pane-post-write-retirement"
          provider="codex"
          theme="dark"
        />,
      );
      await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
        "register_terminal_presentation",
        expect.anything(),
      ));

      const renderer = getLatestTerminalInstance();
      const lifecycle: string[] = [];
      let writeSettled = false;
      let settleRendererWrite: (() => void) | undefined;
      renderer.write.mockImplementation((data: string, callback?: () => void) => {
        if (data.includes("post-write retirement snapshot")) {
          lifecycle.push("write-started");
          settleRendererWrite = () => {
            writeSettled = true;
            lifecycle.push("write-settled");
            callback?.();
          };
          return;
        }
        callback?.();
      });
      renderer.dispose.mockImplementation(() => lifecycle.push("disposed"));
      renderer.scrollToBottom.mockImplementation(() => {
        if (!writeSettled) return;
        view.rerender(
          <AgentTerminal
            sessionId="modern-agent"
            presentationId="pane-post-write-retirement"
            visibility="hidden"
            renderState="suspended"
            provider="codex"
            theme="dark"
          />,
        );
        if (renderer.dispose.mock.calls.length > 0) {
          throw new Error("post-write scroll used a disposed renderer");
        }
        lifecycle.push("post-write-scroll");
      });

      const registration = modernRegistrationResult("pane-post-write-retirement");
      registration.initial_snapshot = {
        ...modernSnapshot(),
        visible_grid: "post-write retirement snapshot",
      };
      await act(async () => {
        registrationGate.resolve(registration);
        await Promise.resolve();
      });
      await waitFor(() => expect(settleRendererWrite).toBeTypeOf("function"));

      await act(async () => {
        settleRendererWrite?.();
        await Promise.resolve();
      });
      await waitFor(() => expect(renderer.dispose).toHaveBeenCalledTimes(1));

      expect(screen.queryByText("Terminal Initialization Fatal Error:")).not.toBeInTheDocument();
      expect(lifecycle).toEqual([
        "write-started",
        "write-settled",
        "post-write-scroll",
        "disposed",
      ]);
      expect(consoleError).not.toHaveBeenCalledWith(
        "AgentTerminal Init Error:",
        expect.anything(),
      );
    } finally {
      consoleError.mockRestore();
    }
  });

  it("does not refit, resize, or reset the renderer for an ordinary broker output update", async () => {
    const listeners = new Map<string, (event: { payload: unknown }) => void>();
    let readCount = 0;
    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: unknown }) => void);
      return () => listeners.delete(eventName);
    });
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "pane-output";
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "read_terminal_events") {
        readCount += 1;
        return readCount === 1 ? {
          status: "events",
          runtime_generation: 1,
          events: [{ sequence: 1, runtime_generation: 1, type: "output", bytes: [65] }],
          next_sequence: 1,
          latest_sequence: 1,
          recovery_snapshot: null,
        } : modernCaughtUpBatch();
      }
      if (command === "ack_terminal_events") return undefined;
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId).presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-output"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));
    await act(async () => new Promise((resolve) => setTimeout(resolve, 350)));
    const renderer = getLatestTerminalInstance();
    renderer.reset.mockClear();
    renderer.write.mockClear();
    mockInvoke.mockClear();

    const eventsReady = listeners.get("terminal-session-events-ready");
    if (!eventsReady) throw new Error("expected broker event listener");
    act(() => eventsReady({
      payload: { session_id: "modern-agent", runtime_generation: 1, latest_sequence: 1 },
    }));

    await waitFor(() => expect(renderer.write).toHaveBeenCalledWith("A", expect.any(Function)));
    expect(renderer.reset).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalledWith("resize_terminal_presentation", expect.anything());
  });

  it("compensates an unmount while broker registration is still in flight", async () => {
    const registrationGate = deferred<ReturnType<typeof modernRegistrationResult>>();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "register_terminal_presentation") {
        return registrationGate.promise;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    const mounted = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-race"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));

    mounted.unmount();
    registrationGate.resolve(modernRegistrationResult("pane-race"));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "unregister_terminal_presentation",
      expect.anything(),
    ));
    expect(mockInvoke).toHaveBeenCalledWith("unsubscribe_terminal_events", expect.anything());
  });

  it("reconciles lifecycle props that change while broker registration is in flight", async () => {
    const registrationGate = deferred<ReturnType<typeof modernRegistrationResult>>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        return registrationGate.promise;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "update_terminal_presentation") {
        const updated = modernRegistrationResult(request?.presentation_id ?? "pane-lifecycle-race");
        return {
          ...updated,
          presentation: {
            ...updated.presentation,
            visibility: "hidden",
            render_state: "suspended",
          },
        };
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    const mounted = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-lifecycle-race"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.objectContaining({
        request: expect.objectContaining({
          visibility: "visible",
          render_state: "mounted",
        }),
      }),
    ));

    mounted.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-lifecycle-race"
        visibility="hidden"
        renderState="suspended"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );
    registrationGate.resolve(modernRegistrationResult("pane-lifecycle-race"));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "update_terminal_presentation",
      expect.objectContaining({
        request: expect.objectContaining({
          presentation_id: "pane-lifecycle-race",
          visibility: "hidden",
          render_state: "suspended",
          requested_interaction: "interactive",
        }),
      }),
    ));
  });

  it("retries a deferred registration with lifecycle props changed while the runtime was missing", async () => {
    const listeners = new Map<string, (event: { payload: unknown }) => void>();
    const registrationRequests: Array<Record<string, unknown>> = [];
    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: unknown }) => void);
      return () => listeners.delete(eventName);
    });
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: Record<string, unknown> } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        registrationRequests.push(request ?? {});
        if (registrationRequests.length === 1) {
          throw new Error("SessionNotFound");
        }
        const recovered = modernRegistrationResult("pane-missing-runtime");
        return {
          ...recovered,
          presentation: {
            ...recovered.presentation,
            visibility: request?.visibility,
            render_state: request?.render_state,
          },
        };
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "read_terminal_events") {
        return modernCaughtUpBatch();
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("pane-missing-runtime").presentation;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      if (command === "terminal_link_target_exists") {
        return true;
      }
      return null;
    });

    const mounted = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-missing-runtime"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(registrationRequests).toHaveLength(1));

    mounted.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-missing-runtime"
        visibility="hidden"
        renderState="suspended"
        requestedInteraction="read_only"
        provider="codex"
        theme="dark"
      />,
    );
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    const lifecycle = listeners.get("terminal-session-lifecycle");
    expect(lifecycle).toBeDefined();
    act(() => {
      lifecycle?.({
        payload: {
          session_id: "modern-agent",
          runtime_generation: 1,
          lifecycle: "runtime_replaced",
        },
      });
    });

    await waitFor(() => expect(registrationRequests).toHaveLength(2));
    expect(registrationRequests[1]).toEqual(expect.objectContaining({
      presentation_id: "pane-missing-runtime",
      visibility: "hidden",
      render_state: "suspended",
      requested_interaction: "read_only",
    }));
  });

  it("does not retain a parser when unmounted during provider resolution", async () => {
    const providerGate = deferred<never[]>();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_agents") {
        return providerGate.promise;
      }
      return null;
    });

    const mounted = render(
      <AgentTerminal
        sessionId="provider-race"
        presentationId="pane-provider-race"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("list_agents"));

    mounted.unmount();
    await act(async () => {
      providerGate.resolve([]);
      await providerGate.promise;
    });

    expect(mockHeadlessTerminal).not.toHaveBeenCalled();
    expect(window.__wardianTerminalDebug?.snapshot("pane-provider-race")).toBeNull();
  });

  it("does not mount a renderer when a delayed visible attach becomes hidden", async () => {
    const providerGate = deferred<never[]>();
    mockInvoke.mockImplementation(async (command: string) => {
      if (command === "list_agents") {
        return providerGate.promise;
      }
      return null;
    });

    const mounted = render(
      <AgentTerminal
        sessionId="provider-hide-race"
        presentationId="pane-provider-hide-race"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("list_agents"));

    mounted.rerender(
      <AgentTerminal
        sessionId="provider-hide-race"
        presentationId="pane-provider-hide-race"
        visibility="hidden"
        renderState="suspended"
        requestedInteraction="interactive"
        theme="dark"
      />,
    );
    await act(async () => {
      providerGate.resolve([]);
      await providerGate.promise;
    });

    expect(mockTerminal).not.toHaveBeenCalled();
    expect(terminalRendererBudget.size("xterm")).toBe(0);
  });

  async function assertFocusIsPassiveBeforeExplicitActivation(
    activation: "click" | "keyboard",
  ) {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(request?.presentation_id ?? "pane-focus");
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("pane-focus").presentation;
      }
      if (command === "begin_terminal_activation") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: null,
          },
          activation_id: "activation-1",
          snapshot: modernSnapshot(),
          sequence_barrier: 0,
        };
      }
      if (command === "ack_terminal_activation") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-focus",
          },
          broker_state: modernBrokerState("pane-focus"),
          snapshot: modernSnapshot(),
        };
      }
      if (command === "read_terminal_events") {
        return modernCaughtUpBatch();
      }
      if (command === "ack_terminal_events") {
        return undefined;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-focus"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));
    expect(mockInvoke).not.toHaveBeenCalledWith("begin_terminal_activation", expect.anything());

    const host = screen.getByTestId("agent-terminal-host");
    fireEvent.focus(host);
    expect(mockInvoke).not.toHaveBeenCalledWith("begin_terminal_activation", expect.anything());

    if (activation === "click") {
      fireEvent.click(host);
    } else {
      fireEvent.keyDown(host, { key: "Enter" });
    }
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("begin_terminal_activation", expect.anything());
      expect(mockInvoke).toHaveBeenCalledWith("ack_terminal_activation", expect.anything());
    });
  }

  it.each(["click", "keyboard"] as const)(
    "keeps DOM focus passive, then activates through the two-phase handshake by %s",
    assertFocusIsPassiveBeforeExplicitActivation,
  );

  it("keeps an Agents terminal hidden until its unowned session is activated and fitted", async () => {
    const activationAck = deferred<{
      decision: {
        status: "accepted";
        reason: null;
        runtime_generation: number;
        lease_epoch: number;
        owner_presentation_id: string;
      };
      broker_state: ReturnType<typeof modernBrokerState>;
      snapshot: ReturnType<typeof modernSnapshot>;
    }>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "agents-pane";
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "begin_terminal_activation") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: null,
          },
          activation_id: "agents-activation",
          snapshot: modernSnapshot(),
          sequence_barrier: 0,
        };
      }
      if (command === "ack_terminal_activation") {
        return activationAck.promise;
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("agents-pane", "agents-pane").presentation;
      }
      if (command === "read_terminal_events") return modernCaughtUpBatch();
      if (command === "ack_terminal_events") return undefined;
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="agents-pane"
        provider="codex"
        theme="dark"
        autoActivateWhenUnowned
      />,
    );

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "ack_terminal_activation",
      expect.anything(),
    ));
    expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "hidden" });

    activationAck.resolve({
      decision: {
        status: "accepted",
        reason: null,
        runtime_generation: 1,
        lease_epoch: 1,
        owner_presentation_id: "agents-pane",
      },
      broker_state: modernBrokerState("agents-pane"),
      snapshot: modernSnapshot(),
    });

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
    });
    const latestFitAddon = mockFitAddon.mock.results[mockFitAddon.mock.results.length - 1]?.value;
    expect(latestFitAddon.proposeDimensions).toHaveBeenCalled();
  });

  it("fits an unowned presentation locally instead of rendering it as a scaled mirror", async () => {
    const unownedBroker = modernBrokerState();
    unownedBroker.geometry = { cols: 240, rows: 80 };
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "unowned-pane";
      if (command === "register_terminal_presentation") {
        const result = modernRegistrationResult(presentationId);
        result.broker_state = unownedBroker;
        return result;
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: unownedBroker, initial_snapshot: modernSnapshot() };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId).presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="unowned-pane"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
    });
    expect(getLatestTerminalInstance().element.style.transform).toBe("");
    expect(getLatestTerminalInstance().cols).toBe(80);
    expect(getLatestTerminalInstance().rows).toBe(24);
  });

  it("resyncs a restored hidden owner after its renderer mounts", async () => {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        const result = modernRegistrationResult(
          request?.presentation_id ?? "pane-resync",
          "pane-resync",
        );
        result.presentation.requires_resync = true;
        return result;
      }
      if (command === "subscribe_terminal_events") {
        return {
          broker_state: modernBrokerState("pane-resync"),
          initial_snapshot: modernSnapshot(),
        };
      }
      if (command === "begin_terminal_owner_resync") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-resync",
          },
          resync_id: "resync-1",
          snapshot: modernSnapshot(),
          sequence_barrier: 0,
        };
      }
      if (command === "ack_terminal_owner_resync") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-resync",
          },
          broker_state: modernBrokerState("pane-resync"),
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("pane-resync", "pane-resync").presentation;
      }
      if (command === "resize_terminal_presentation") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-resync",
          },
          geometry_sequence: 1,
          geometry: { cols: 80, rows: 24 },
          snapshot: null,
        };
      }
      if (command === "read_terminal_events") {
        return modernCaughtUpBatch();
      }
      if (command === "ack_terminal_events") {
        return undefined;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-resync"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("begin_terminal_owner_resync", expect.anything());
      expect(mockInvoke).toHaveBeenCalledWith("ack_terminal_owner_resync", expect.anything());
    });
  });

  it("automatically restores a visible budget-evicted owner without unregistering its presentation", async () => {
    terminalRendererBudget.clear();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (
        args as {
          request?: { presentation_id?: string; render_state?: "mounted" | "suspended" };
        } | undefined
      )?.request;
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(
          request?.presentation_id ?? "pane-evicted",
          "pane-evicted",
        );
      }
      if (command === "subscribe_terminal_events") {
        return {
          broker_state: modernBrokerState("pane-evicted"),
          initial_snapshot: modernSnapshot(),
        };
      }
      if (command === "update_terminal_presentation") {
        const presentation = modernRegistrationResult("pane-evicted", "pane-evicted").presentation;
        return {
          presentation: {
            ...presentation,
            render_state: request?.render_state ?? "mounted",
            requires_resync: request?.render_state === "mounted",
          },
          broker_state: modernBrokerState("pane-evicted"),
        };
      }
      if (command === "request_terminal_snapshot") {
        return modernSnapshot();
      }
      if (command === "begin_terminal_owner_resync") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-evicted",
          },
          resync_id: "resync-evicted",
          snapshot: modernSnapshot(),
          sequence_barrier: 0,
        };
      }
      if (command === "ack_terminal_owner_resync") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-evicted",
          },
          broker_state: modernBrokerState("pane-evicted"),
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("pane-evicted", "pane-evicted").presentation;
      }
      if (command === "resize_terminal_presentation") {
        return {
          decision: {
            status: "accepted",
            reason: null,
            runtime_generation: 1,
            lease_epoch: 1,
            owner_presentation_id: "pane-evicted",
          },
          geometry_sequence: 1,
          geometry: { cols: 80, rows: 24 },
          snapshot: null,
        };
      }
      if (command === "read_terminal_events") {
        return modernCaughtUpBatch();
      }
      if (command === "ack_terminal_events") {
        return undefined;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-evicted"
        visibility="visible"
        renderState="mounted"
        requestedInteraction="interactive"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));

    for (let index = 0; index < 24; index += 1) {
      terminalRendererBudget.acquire("xterm", `other-${index}`, () => undefined);
    }
    terminalRendererBudget.release("xterm", "other-0");

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("request_terminal_snapshot", expect.anything());
      expect(mockInvoke).toHaveBeenCalledWith("begin_terminal_owner_resync", expect.anything());
      expect(mockInvoke).toHaveBeenCalledWith("ack_terminal_owner_resync", expect.anything());
    });
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "register_terminal_presentation"),
    ).toHaveLength(1);
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "unregister_terminal_presentation",
      expect.anything(),
    );
  });

  it("waits for nonzero measured bounds before automatically restoring an evicted renderer", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;
    let measuredWidth = 900;
    let measuredHeight = 600;
    rectSpy.mockImplementation(() => ({
      width: measuredWidth,
      height: measuredHeight,
      top: 0,
      left: 0,
      right: measuredWidth,
      bottom: measuredHeight,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect));
    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="zero-bounds-restore" provider="codex" theme="dark" />);
      await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(1));

      measuredWidth = 0;
      measuredHeight = 0;
      for (let index = 0; index < 24; index += 1) {
        terminalRendererBudget.acquire("xterm", `zero-other-${index}`, () => undefined);
      }
      await act(async () => undefined);
      expect(mockTerminal).toHaveBeenCalledTimes(1);

      measuredWidth = 900;
      measuredHeight = 600;
      terminalRendererBudget.release("xterm", "zero-other-0");
      act(() => resizeCallback?.([], {} as ResizeObserver));

      await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(2));
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("shows Retry only after automatic renderer restoration genuinely fails", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      render(<AgentTerminal sessionId="restore-retry" provider="codex" theme="dark" />);
      await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(1));

      mockTerminal.mockImplementationOnce(() => {
        throw new Error("renderer construction failed");
      });
      act(() => {
        for (let index = 0; index < 24; index += 1) {
          terminalRendererBudget.acquire("xterm", `retry-other-${index}`, () => undefined);
        }
        terminalRendererBudget.release("xterm", "retry-other-0");
      });

      const retry = await screen.findByRole("button", { name: "Retry" });
      expect(screen.getByText("Terminal renderer failed to restore.")).toBeInTheDocument();

      fireEvent.click(retry);

      await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(3));
      await waitFor(() => expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument());
    } finally {
      consoleError.mockRestore();
    }
  });

  it("does not mount a hidden presentation and mounts it automatically once visible", async () => {
    const view = render(
      <AgentTerminal
        sessionId="hidden-restore"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="dark"
      />,
    );
    await act(async () => undefined);
    expect(mockTerminal).not.toHaveBeenCalled();
    expect(terminalRendererBudget.has("xterm", "hidden-restore")).toBe(false);

    act(() => {
      for (let index = 0; index < 24; index += 1) {
        terminalRendererBudget.acquire("xterm", `hidden-other-${index}`, () => undefined);
      }
    });
    await act(async () => undefined);
    expect(mockTerminal).not.toHaveBeenCalled();

    view.rerender(
      <AgentTerminal
        sessionId="hidden-restore"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );
    terminalRendererBudget.release("xterm", "hidden-other-0");

    await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(1));
    expect(terminalRendererBudget.has("xterm", "hidden-restore")).toBe(true);
  });

  it("keeps more than the renderer budget of hidden cards from evicting one visible terminal", async () => {
    render(
      <>
        {Array.from({ length: 30 }, (_, index) => (
          <AgentTerminal
            key={`hidden-${index}`}
            sessionId={`hidden-budget-${index}`}
            visibility="hidden"
            renderState="suspended"
            provider="codex"
            theme="dark"
          />
        ))}
        <AgentTerminal sessionId="visible-budget-owner" provider="codex" theme="dark" />
      </>,
    );

    await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(1));
    expect(terminalRendererBudget.size("xterm")).toBe(1);
    expect(terminalRendererBudget.has("xterm", "visible-budget-owner")).toBe(true);
    for (let index = 0; index < 30; index += 1) {
      expect(terminalRendererBudget.has("xterm", `hidden-budget-${index}`)).toBe(false);
    }
  });

  it("does not churn competing visible AgentTerminal renderers when the xterm budget is full", async () => {
    render(
      <>
        {Array.from({ length: 25 }, (_, index) => (
          <AgentTerminal
            key={index}
            sessionId={`competing-visible-${index}`}
            provider="codex"
            theme="dark"
          />
        ))}
      </>,
    );

    await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(25));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 100));
    });

    expect(mockTerminal).toHaveBeenCalledTimes(25);
    expect(terminalRendererBudget.size("xterm")).toBe(24);
  });

  it("does not retire a replacement renderer when a stale restore rejects", async () => {
    const restoreSnapshot = deferred<ReturnType<typeof modernSnapshot>>();
    let snapshotRequestCount = 0;
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: Record<string, unknown> } | undefined)?.request;
      const presentationId = String(request?.presentation_id ?? "stale-restore-pane");
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "request_terminal_snapshot") {
        snapshotRequestCount += 1;
        return snapshotRequestCount === 1 ? restoreSnapshot.promise : modernSnapshot();
      }
      if (command === "update_terminal_presentation") {
        const result = modernRegistrationResult("stale-restore-pane");
        return {
          ...result,
          presentation: {
            ...result.presentation,
            visibility: request?.visibility ?? "visible",
            render_state: request?.render_state ?? "mounted",
          },
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("stale-restore-pane").presentation;
      }
      if (command === "read_terminal_events") return modernCaughtUpBatch();
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      return undefined;
    });

    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const view = render(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="stale-restore-pane"
          provider="codex"
          theme="dark"
        />,
      );
      await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
        "register_terminal_presentation",
        expect.anything(),
      ));

      act(() => {
        for (let index = 0; index < 24; index += 1) {
          terminalRendererBudget.acquire("xterm", `stale-restore-other-${index}`, () => undefined);
        }
        terminalRendererBudget.release("xterm", "stale-restore-other-0");
      });
      await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
        "request_terminal_snapshot",
        expect.anything(),
      ));
      expect(mockTerminal).toHaveBeenCalledTimes(2);

      view.rerender(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="stale-restore-pane"
          visibility="hidden"
          renderState="suspended"
          provider="codex"
          theme="dark"
        />,
      );
      view.rerender(
        <AgentTerminal
          sessionId="modern-agent"
          presentationId="stale-restore-pane"
          visibility="visible"
          renderState="mounted"
          provider="codex"
          theme="dark"
        />,
      );

      await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(3));
      const replacement = getLatestTerminalInstance();
      expect(terminalRendererBudget.has("xterm", "stale-restore-pane")).toBe(true);

      await act(async () => {
        restoreSnapshot.reject(new Error("stale restore failed"));
        await Promise.resolve();
      });

      await waitFor(() => expect(snapshotRequestCount).toBe(2));
      expect(consoleError).not.toHaveBeenCalledWith(
        "Terminal renderer restore failed:",
        expect.anything(),
      );
      expect(replacement.dispose).not.toHaveBeenCalled();
      expect(terminalRendererBudget.has("xterm", "stale-restore-pane")).toBe(true);
      expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("does not apply a resolved stale restore snapshot to a replacement renderer", async () => {
    const restoreSnapshot = deferred<ReturnType<typeof modernSnapshot>>();
    const queuedUpdateStarted = deferred<void>();
    const queuedUpdateRelease = deferred<ReturnType<typeof modernRegistrationResult>>();
    let snapshotRequestCount = 0;
    let holdNextUpdate = false;
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: Record<string, unknown> } | undefined)?.request;
      const presentationId = String(request?.presentation_id ?? "resolved-stale-restore-pane");
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "request_terminal_snapshot") {
        snapshotRequestCount += 1;
        return snapshotRequestCount === 1 ? restoreSnapshot.promise : modernSnapshot();
      }
      if (command === "update_terminal_presentation") {
        if (holdNextUpdate) {
          holdNextUpdate = false;
          queuedUpdateStarted.resolve();
          return queuedUpdateRelease.promise;
        }
        const result = modernRegistrationResult("resolved-stale-restore-pane");
        return {
          ...result,
          presentation: {
            ...result.presentation,
            visibility: request?.visibility ?? "visible",
            render_state: request?.render_state ?? "mounted",
          },
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("resolved-stale-restore-pane").presentation;
      }
      if (command === "read_terminal_events") return modernCaughtUpBatch();
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      return undefined;
    });

    const view = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="resolved-stale-restore-pane"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));

    act(() => {
      for (let index = 0; index < 24; index += 1) {
        terminalRendererBudget.acquire(
          "xterm",
          `resolved-stale-restore-other-${index}`,
          () => undefined,
        );
      }
      terminalRendererBudget.release("xterm", "resolved-stale-restore-other-0");
    });
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "request_terminal_snapshot",
      expect.anything(),
    ));
    expect(mockTerminal).toHaveBeenCalledTimes(2);

    holdNextUpdate = true;
    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="resolved-stale-restore-pane"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="dark"
      />,
    );
    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="resolved-stale-restore-pane"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => expect(mockTerminal).toHaveBeenCalledTimes(3));
    const replacement = getLatestTerminalInstance();
    replacement.reset.mockClear();
    replacement.write.mockClear();
    replacement.refresh.mockClear();

    const staleSnapshot = {
      ...modernSnapshot(),
      visible_grid: "stale restore must not reach the replacement",
    };
    await act(async () => {
      restoreSnapshot.resolve(staleSnapshot);
      await queuedUpdateStarted.promise;
    });

    expect(replacement.reset).not.toHaveBeenCalled();
    expect(replacement.write).not.toHaveBeenCalled();
    expect(replacement.refresh).not.toHaveBeenCalled();
    expect(terminalRendererBudget.has("xterm", "resolved-stale-restore-pane")).toBe(true);

    await act(async () => {
      queuedUpdateRelease.resolve(modernRegistrationResult("resolved-stale-restore-pane"));
      await Promise.resolve();
    });
    await waitFor(() => expect(snapshotRequestCount).toBe(2));
  });

  it("cancels an in-flight restore when the presentation becomes hidden", async () => {
    const restoreSnapshot = deferred<ReturnType<typeof modernSnapshot>>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: Record<string, unknown> } | undefined)?.request;
      const presentationId = String(request?.presentation_id ?? "in-flight-pane");
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "request_terminal_snapshot") {
        return restoreSnapshot.promise;
      }
      if (command === "update_terminal_presentation") {
        const result = modernRegistrationResult("in-flight-pane");
        return {
          ...result,
          presentation: {
            ...result.presentation,
            visibility: request?.visibility ?? "visible",
            render_state: request?.render_state ?? "mounted",
          },
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("in-flight-pane").presentation;
      }
      if (command === "read_terminal_events") return modernCaughtUpBatch();
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      return undefined;
    });

    const view = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="in-flight-pane"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("register_terminal_presentation", expect.anything()));
    mockInvoke.mockClear();

    act(() => {
      for (let index = 0; index < 24; index += 1) {
        terminalRendererBudget.acquire("xterm", `in-flight-other-${index}`, () => undefined);
      }
      terminalRendererBudget.release("xterm", "in-flight-other-0");
    });
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("request_terminal_snapshot", expect.anything()));
    const restoringTerminal = getLatestTerminalInstance();
    expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "hidden" });

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="in-flight-pane"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="dark"
      />,
    );
    restoreSnapshot.resolve(modernSnapshot());

    await waitFor(() => expect(restoringTerminal.dispose).toHaveBeenCalled());
    await waitFor(() => {
      const lifecycleUpdates = mockInvoke.mock.calls
        .filter(([command]) => command === "update_terminal_presentation")
        .map(([, payload]) => (payload as { request: { visibility: string; render_state: string } }).request);
      expect(lifecycleUpdates[lifecycleUpdates.length - 1]).toMatchObject({
        visibility: "hidden",
        render_state: "suspended",
      });
      expect(lifecycleUpdates).not.toContainEqual(expect.objectContaining({ visibility: "visible", render_state: "mounted" }));
    });
  });

  it("suspends the broker again when restoration fails after mounting the presentation", async () => {
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: Record<string, unknown> } | undefined)?.request;
      const presentationId = String(request?.presentation_id ?? "failure-after-mount");
      if (command === "register_terminal_presentation") return modernRegistrationResult(presentationId);
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: modernSnapshot() };
      }
      if (command === "request_terminal_snapshot") return modernSnapshot();
      if (command === "update_terminal_presentation") {
        const result = modernRegistrationResult("failure-after-mount");
        return {
          ...result,
          presentation: {
            ...result.presentation,
            visibility: request?.visibility ?? "visible",
            render_state: request?.render_state ?? "mounted",
          },
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("failure-after-mount").presentation;
      }
      if (command === "read_terminal_events") return modernCaughtUpBatch();
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      return undefined;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="failure-after-mount"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("register_terminal_presentation", expect.anything()));
    mockInvoke.mockClear();
    fitDimensions = null as never;

    act(() => {
      for (let index = 0; index < 24; index += 1) {
        terminalRendererBudget.acquire("xterm", `failure-other-${index}`, () => undefined);
      }
      terminalRendererBudget.release("xterm", "failure-other-0");
    });

    await screen.findByRole("button", { name: "Retry" });
    const renderStates = mockInvoke.mock.calls
      .filter(([command]) => command === "update_terminal_presentation")
      .map(([, payload]) => (payload as { request: { render_state: string } }).request.render_state);
    expect(renderStates).toContain("mounted");
    expect(renderStates[renderStates.length - 1]).toBe("suspended");
  });

  it("reports mirror viewport geometry without resizing the native PTY", async () => {
    fitDimensions = { cols: 100, rows: 30 };
    const mirrorBroker = modernBrokerState("pane-owner");
    mirrorBroker.geometry = { cols: 240, rows: 80 };
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        const result = modernRegistrationResult(request?.presentation_id ?? "pane-mirror", "pane-owner");
        result.broker_state = mirrorBroker;
        result.initial_snapshot.geometry = mirrorBroker.geometry;
        return result;
      }
      if (command === "subscribe_terminal_events") {
        return {
          broker_state: mirrorBroker,
          initial_snapshot: { ...modernSnapshot(), geometry: mirrorBroker.geometry },
        };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("pane-mirror", "pane-owner").presentation;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState("pane-owner");
      }
      if (command === "unsubscribe_terminal_events") {
        return undefined;
      }
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="pane-mirror"
        provider="codex"
        theme="dark"
      />,
    );

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "report_terminal_presentation_viewport",
      expect.anything(),
    ));
    const mirrorRenderer = getLatestTerminalInstance();
    expect(mirrorRenderer.cols).toBe(100);
    expect(mirrorRenderer.rows).toBe(30);
    expect(mirrorRenderer.resize).not.toHaveBeenCalledWith(240, 80);
    expect(mirrorRenderer.element.style.transform).toBe("");
    expect(mockInvoke).not.toHaveBeenCalledWith("resize_terminal_presentation", expect.anything());
  });

  it("normalizes a restored Codex broker snapshot through the composer theme path", async () => {
    const rawComposer = "\u001b[48;2;41;41;41mcomposer\u001b[m";
    const encodedComposer = btoa(String.fromCharCode(...new TextEncoder().encode(rawComposer)));
    const snapshot = {
      ...modernSnapshot(),
      terminal_state_base64: encodedComposer,
    };
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      if (command === "register_terminal_presentation") {
        return {
          ...modernRegistrationResult(request?.presentation_id ?? "codex-snapshot-theme"),
          initial_snapshot: snapshot,
        };
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: snapshot };
      }
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult("codex-snapshot-theme").presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="codex-snapshot-theme"
        provider="codex"
        theme="light"
      />,
    );

    await waitFor(() => {
      const renderer = getLatestTerminalInstance();
      expect(renderer.write).toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;242;240;235mcomposer"),
        expect.any(Function),
      );
      expect(renderer.write).not.toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;41;41;41mcomposer"),
        expect.any(Function),
      );
    });
  });

  it("does not consume OpenCode's live focus reply while normalizing a restored snapshot", async () => {
    const listeners = new Map<string, (event: { payload: unknown }) => void>();
    const focusMode = "\u001b[?1004h";
    const encodedFocusMode = btoa(String.fromCharCode(...new TextEncoder().encode(focusMode)));
    const snapshot = { ...modernSnapshot(), terminal_state_base64: encodedFocusMode };
    let readCount = 0;
    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: unknown }) => void);
      return () => listeners.delete(eventName);
    });
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "opencode-focus-snapshot";
      if (command === "register_terminal_presentation") {
        return { ...modernRegistrationResult(presentationId), initial_snapshot: snapshot };
      }
      if (command === "subscribe_terminal_events") {
        return { broker_state: modernBrokerState(), initial_snapshot: snapshot };
      }
      if (command === "read_terminal_events") {
        readCount += 1;
        return readCount === 1 ? {
          status: "events",
          runtime_generation: 1,
          events: [{
            sequence: 1,
            runtime_generation: 1,
            type: "output",
            bytes: Array.from(new TextEncoder().encode(focusMode)),
          }],
          next_sequence: 1,
          latest_sequence: 1,
          recovery_snapshot: null,
        } : modernCaughtUpBatch();
      }
      if (command === "send_terminal_presentation_input") {
        return {
          status: "accepted",
          reason: null,
          runtime_generation: 1,
          lease_epoch: 0,
          owner_presentation_id: null,
        };
      }
      if (command === "ack_terminal_events") return undefined;
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId).presentation;
      }
      if (command === "unregister_terminal_presentation") return modernBrokerState();
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="opencode-focus-snapshot"
        provider="opencode"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "send_terminal_presentation_input",
      expect.anything(),
    );

    const eventsReady = listeners.get("terminal-session-events-ready");
    if (!eventsReady) throw new Error("expected broker event listener");
    act(() => eventsReady({
      payload: { session_id: "modern-agent", runtime_generation: 1, latest_sequence: 1 },
    }));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "send_terminal_presentation_input",
      expect.objectContaining({ request: expect.objectContaining({ input: "\u001b[I" }) }),
    ));
  });

  it("installs conservative terminal shortcuts on the renderer", async () => {
    render(<AgentTerminal sessionId="codex-shortcuts" theme="dark" />);

    await waitFor(() => {
      expect(getLatestTerminalInstance().attachCustomKeyEventHandler).toHaveBeenCalledTimes(1);
    });
  });

  it("keeps OpenCode terminal stdin enabled when the card is not selected", async () => {
    const view = render(<AgentTerminal sessionId="opencode-passive" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(getLatestTerminalInstance().options.disableStdin).toBe(false);
    });

    view.rerender(<AgentTerminal sessionId="opencode-passive" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(getLatestTerminalInstance().options.disableStdin).toBe(false);
    });
  });

  it("installs terminal link handling that opens files with the configured external editor", async () => {
    useSettingsStore.setState({
      externalEditor: "vscode",
      externalEditorCustomExecutable: "",
    });
    render(
      <AgentTerminal
        sessionId="codex-links"
        theme="dark"
        workspacePath="C:\\repo"
      />,
    );

    await waitFor(() => {
      expect(getLatestTerminalInstance().registerLinkProvider).toHaveBeenCalledTimes(1);
    });

    const provider = getLatestTerminalInstance().registerLinkProvider.mock.calls[0][0];
    const links = await new Promise<any[] | undefined>((resolve) => {
      provider.provideLinks(1, resolve);
    });
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("open_in_external_editor", {
        path: "C:\\repo\\src\\App.tsx",
        editor: {
          external_editor: "vscode",
          external_editor_custom_executable: null,
        },
      });
    });
  });

  it("reuses the live renderer on a quick remount without recreating the WebGL context", async () => {
    // A quick unmount/remount (grid maximize then minimize, tab switch) must not
    // tear down and recreate the renderer — recreating WebGL contexts in a burst
    // trips Chrome's context cap and flashes the lost-context placeholder.
    const firstRender = render(
      <AgentTerminal sessionId="codex-1" theme="dark" />,
    );

    await waitFor(() => {
      const firstInstance = getLatestTerminalInstance();
      expect(firstInstance.write).toHaveBeenCalledWith("hello from codex\n", expect.any(Function));
    });

    const firstInstance = getLatestTerminalInstance();
    firstRender.unmount();
    // Within the grace window the renderer is kept alive.
    expect(firstInstance.dispose).not.toHaveBeenCalled();
    expect(window.__wardianTerminalDebug?.snapshot("codex-1")?.renderer).toBeTruthy();

    render(<AgentTerminal sessionId="codex-1" theme="dark" />);

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("codex-1")?.renderer).toBeTruthy();
    });

    // No second xterm was constructed; the original instance was reused.
    expect(mockTerminal).toHaveBeenCalledTimes(1);
    expect(firstInstance.dispose).not.toHaveBeenCalled();
  });

  it("ignores context loss from a retired WebGL generation after replacement", async () => {
    const contextLossCallbacks: Array<() => void> = [];
    mockWebglAddon.mockImplementation(function MockWebglAddon() {
      return {
        onContextLoss: vi.fn((callback: () => void) => contextLossCallbacks.push(callback)),
        clearTextureAtlas: vi.fn(),
        dispose: vi.fn(),
      } as any;
    });

    const view = render(
      <AgentTerminal sessionId="stale-webgl-loss" provider="codex" theme="dark" />,
    );
    await waitFor(() => expect(contextLossCallbacks).toHaveLength(1));

    const retiredTerminal = getLatestTerminalInstance();
    const retiredAddon = mockWebglAddon.mock.results[0]?.value as { dispose: ReturnType<typeof vi.fn> };
    view.rerender(
      <AgentTerminal
        sessionId="stale-webgl-loss"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="dark"
      />,
    );
    view.rerender(
      <AgentTerminal sessionId="stale-webgl-loss" provider="codex" theme="dark" />,
    );

    await waitFor(() => expect(contextLossCallbacks).toHaveLength(2));
    const replacementTerminal = getLatestTerminalInstance();
    expect(replacementTerminal).not.toBe(retiredTerminal);
    const replacementAddon = mockWebglAddon.mock.results[1]?.value as {
      dispose: ReturnType<typeof vi.fn>;
    };
    const retiredDisposeCalls = retiredAddon.dispose.mock.calls.length;
    const retiredRefreshCalls = retiredTerminal.refresh.mock.calls.length;
    replacementTerminal.refresh.mockClear();
    replacementAddon.dispose.mockClear();
    expect(terminalRendererBudget.has("webgl", "stale-webgl-loss")).toBe(true);

    act(() => contextLossCallbacks[0]?.());

    expect(retiredAddon.dispose).toHaveBeenCalledTimes(retiredDisposeCalls);
    expect(retiredTerminal.refresh).toHaveBeenCalledTimes(retiredRefreshCalls);
    expect(replacementAddon.dispose).not.toHaveBeenCalled();
    expect(replacementTerminal.refresh).not.toHaveBeenCalled();
    expect(terminalRendererBudget.has("webgl", "stale-webgl-loss")).toBe(true);
  });

  it("disposes the presentation entry once it stays unmounted past the grace window", async () => {
    const firstRender = render(
      <AgentTerminal sessionId="codex-grace" provider="codex" theme="dark" />,
    );

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("codex-grace")?.renderer).toBeTruthy();
    });

    const instance = getLatestTerminalInstance();

    // Switch to fake timers only for the unmount + grace-window advance, so the
    // async mount above stays on real timers (avoids RTL/fake-timer deadlocks).
    vi.useFakeTimers();
    try {
      firstRender.unmount();
      expect(instance.dispose).not.toHaveBeenCalled();
      expect(window.__wardianTerminalDebug?.snapshot("codex-grace")?.renderer).toBeTruthy();

      vi.advanceTimersByTime(30_000);

      expect(instance.dispose).toHaveBeenCalled();
      expect(window.__wardianTerminalDebug?.snapshot("codex-grace")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("force-loses the WebGL context when a renderer is disposed", async () => {
    // @xterm/addon-webgl never calls WEBGL_lose_context on dispose, so Chromium
    // keeps counting evicted contexts as active until GC. Disposal must lose the
    // context explicitly or churn still trips the browser's context cap.
    const firstRender = render(
      <AgentTerminal sessionId="webgl-lose" provider="codex" theme="dark" />,
    );

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("webgl-lose")?.renderer?.webglActive).toBe(true);
    });

    const instance = getLatestTerminalInstance();
    const loseContext = vi.fn();
    const canvas = document.createElement("canvas");
    canvas.getContext = vi.fn(
      () => ({
        getExtension: vi.fn((name: string) =>
          name === "WEBGL_lose_context" ? { loseContext } : null,
        ),
      }),
    ) as never;
    (instance.element as HTMLElement).appendChild(canvas);

    vi.useFakeTimers();
    try {
      firstRender.unmount();
      vi.advanceTimersByTime(30_000);

      expect(instance.dispose).toHaveBeenCalled();
      expect(loseContext).toHaveBeenCalledTimes(1);
      expect(canvas.getContext).toHaveBeenCalledWith("webgl2");
    } finally {
      vi.useRealTimers();
    }
  });

  it("computes columns from the cell size without the FitAddon overview-ruler gutter", () => {
    // FitAddon reserves a flat 14px gutter when scrollback is enabled, costing
    // ~2 columns even though we never render an overview ruler. Our computation
    // must use the full host width / cell width instead.
    const fallback = vi.fn(() => ({ cols: 1, rows: 1 }));
    const renderer = {
      term: {
        _core: { _renderService: { dimensions: { css: { cell: { width: 8, height: 16 } } } } },
      },
      host: { clientWidth: 800, clientHeight: 320 },
      fitAddon: { proposeDimensions: fallback },
    } as never;

    const dims = __terminalTesting.proposeTerminalDimensions(renderer);

    // 800 / 8 = 100 cols (FitAddon would give floor((800-14)/8) = 98), 320/16 = 20.
    expect(dims).toEqual({ cols: 100, rows: 20 });
    expect(fallback).not.toHaveBeenCalled();
  });

  it("falls back to FitAddon when xterm cell internals are unavailable", () => {
    const fallback = vi.fn(() => ({ cols: 77, rows: 19 }));
    const renderer = {
      term: {},
      host: { clientWidth: 800, clientHeight: 320 },
      fitAddon: { proposeDimensions: fallback },
    } as never;

    expect(__terminalTesting.proposeTerminalDimensions(renderer)).toEqual({ cols: 77, rows: 19 });
    expect(fallback).toHaveBeenCalledTimes(1);
  });

  it("uses rendered row geometry when stale xterm internals would leave blank bottom rows", () => {
    const fallback = vi.fn(() => ({ cols: 1, rows: 1 }));
    const rowOne = document.createElement("div");
    const rowTwo = document.createElement("div");
    rowOne.getBoundingClientRect = vi.fn(
      () =>
        ({
          width: 800,
          height: 16,
          top: 0,
          left: 0,
          right: 800,
          bottom: 16,
          x: 0,
          y: 0,
          toJSON: () => ({}),
        }) as DOMRect,
    );
    rowTwo.getBoundingClientRect = vi.fn(
      () =>
        ({
          width: 800,
          height: 16,
          top: 16,
          left: 0,
          right: 800,
          bottom: 32,
          x: 0,
          y: 16,
          toJSON: () => ({}),
        }) as DOMRect,
    );
    const rows = document.createElement("div");
    rows.className = "xterm-rows";
    rows.append(rowOne, rowTwo);
    const element = document.createElement("div");
    element.append(rows);
    const renderer = {
      term: {
        element,
        _core: { _renderService: { dimensions: { css: { cell: { width: 8, height: 20 } } } } },
      },
      host: { clientWidth: 800, clientHeight: 320 },
      fitAddon: { proposeDimensions: fallback },
    } as never;

    expect(__terminalTesting.proposeTerminalDimensions(renderer)).toEqual({ cols: 100, rows: 20 });
    expect(fallback).not.toHaveBeenCalled();
  });

  it("releases WebGL1 fallback contexts (not just webgl2) on disposal", async () => {
    // @xterm/addon-webgl silently falls back to a WebGL1 context when webgl2 is
    // unavailable (common once the browser nears its context cap). Probing only
    // "webgl2" returns null for those canvases, so their context never gets
    // WEBGL_lose_context — they accumulate as zombies and re-trip the cap. The
    // release path must try "webgl" too.
    const firstRender = render(
      <AgentTerminal sessionId="webgl1-lose" provider="codex" theme="dark" />,
    );

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("webgl1-lose")?.renderer?.webglActive).toBe(true);
    });

    const instance = getLatestTerminalInstance();
    const loseContext = vi.fn();
    const canvas = document.createElement("canvas");
    // Emulate a WebGL1-only canvas: getContext("webgl2") → null, "webgl" → ctx.
    canvas.getContext = vi.fn((type: string) =>
      type === "webgl"
        ? {
            getExtension: (name: string) =>
              name === "WEBGL_lose_context" ? { loseContext } : null,
          }
        : null,
    ) as never;
    (instance.element as HTMLElement).appendChild(canvas);

    vi.useFakeTimers();
    try {
      firstRender.unmount();
      vi.advanceTimersByTime(30_000);

      expect(instance.dispose).toHaveBeenCalled();
      expect(canvas.getContext).toHaveBeenCalledWith("webgl2");
      expect(canvas.getContext).toHaveBeenCalledWith("webgl");
      expect(loseContext).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("recovers a headless parser when an xterm reflow resize throws", async () => {
    // xterm 6.0.0 reflow can throw mid-resize on large headless buffers,
    // leaving the buffer half-resized so every later write/serialize throws.
    // resizeParser must catch the throw, reset the parser, and re-apply the
    // size instead of letting the uncaught error cascade.
    render(<AgentTerminal sessionId="parser-recover" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("parser-recover")?.renderer).toBeTruthy();
    });

    const parser = getLatestHeadlessTerminalInstance();
    parser.resize = vi.fn(() => {
      throw new TypeError("Cannot read properties of undefined (reading 'resize')");
    });
    parser.reset = vi.fn();

    expect(() =>
      __terminalTesting.resizeParser(
        { parser, lastReportedSize: null } as never,
        120,
        40,
      ),
    ).not.toThrow();

    // After the failed reflow it reset the parser and retried the resize.
    expect(parser.reset).toHaveBeenCalledTimes(1);
    expect(parser.resize).toHaveBeenCalledTimes(2);
  });

  it("caps live WebGL renderers without promoting a focused terminal onto the GPU", async () => {
    // Render more terminals than the WebGL pool cap (12). The pool must stay at
    // 12 live contexts; the rest fall back to the DOM renderer. Newly mounted
    // terminals evict the least-recently-used, so older sessions left over from
    // earlier tests are squeezed out first — these ids end up holding the pool.
    const ids = Array.from({ length: 14 }, (_, index) => `pool-${index}`);
    const view = render(
      <>
        {ids.map((id) => (
          <AgentTerminal key={id} sessionId={id} theme="dark" />
        ))}
      </>,
    );

    const webglActiveCount = () =>
      ids.filter((id) => window.__wardianTerminalDebug?.snapshot(id)?.renderer?.webglActive).length;

    await waitFor(() => {
      expect(ids.every((id) => window.__wardianTerminalDebug?.snapshot(id)?.renderer)).toBe(true);
    });

    // Exactly the cap is GPU-accelerated; the remainder render on DOM (no context).
    expect(webglActiveCount()).toBe(12);

    const domId = ids.find(
      (id) => !window.__wardianTerminalDebug?.snapshot(id)?.renderer?.webglActive,
    );
    expect(domId).toBeDefined();

    const hosts = view.container.querySelectorAll<HTMLElement>('[data-testid="agent-terminal-host"]');
    const domHost = hosts[ids.indexOf(domId!)];
    act(() => {
      fireEvent.focusIn(domHost);
    });

    // Focus must not swap renderers; renderer changes alter terminal text rasterization.
    expect(window.__wardianTerminalDebug?.snapshot(domId!)?.renderer?.webglActive).toBe(false);
    expect(webglActiveCount()).toBe(12);
  });

  it("applies cursor-home provider redraws in place after resize", async () => {
    const createLine = (text: string) => ({
      clone: () => createLine(text),
      translateToString: () => text,
    });
    function createHeadlessTerm(options?: ConstructorParameters<typeof HeadlessTerminal>[0]) {
      const lines: ReturnType<typeof createLine>[] = [];
      const internalBuffer = {
        x: 0,
        y: 0,
        ybase: 10,
        ydisp: 10,
        lines: {
          get: (index: number) => lines[index],
          set: (index: number, value: ReturnType<typeof createLine>) => {
            lines[index] = value;
            terminal.buffer.active.length = Math.max(terminal.buffer.active.length, index + 1);
          },
        },
      };
      const terminal = {
        cols: options?.cols ?? 80,
        rows: options?.rows ?? 24,
        options: { ...(options ?? {}) },
        buffer: {
          active: {
            baseY: internalBuffer.ybase,
            viewportY: internalBuffer.ydisp,
            length: 0,
            getLine: (index: number) => lines[index],
          },
        },
        _core: { _bufferService: { buffer: internalBuffer } },
        write: vi.fn((data: string, callback?: () => void) => {
          const rendered = data
            .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "")
            .split(/\r?\n/)
            .map((line) => line.trimEnd());
          rendered.forEach((line, row) => {
            if (line.length > 0) {
              internalBuffer.lines.set(internalBuffer.ybase + row, createLine(line));
            }
          });
          callback?.();
        }),
        resize: vi.fn((cols: number, rows: number) => {
          terminal.cols = cols;
          terminal.rows = rows;
        }),
        dispose: vi.fn(),
      } as any;
      return terminal;
    }
    mockHeadlessTerminal.mockImplementation(createHeadlessTerm);
    const term = createHeadlessTerm({ cols: 120, rows: 40 });
    for (let index = 0; index < 40; index += 1) {
      term._core._bufferService.buffer.lines.set(10 + index, createLine(` stale ${index + 1}`));
    }
    const beforeBaseY = term.buffer.active.baseY;

    const applied = await __terminalTesting.applyViewportRedrawInPlace(
      term,
      "\u001b[H" + Array.from({ length: 40 }, (_, index) => ` ${index + 1}\u001b[K`).join("\r\n"),
      { preserveExistingViewport: false },
    );

    expect(applied).toBe(true);
    expect(term.buffer.active.baseY).toBe(beforeBaseY);
    expect(term.buffer.active.getLine(10)?.translateToString()).toBe(" 1");
    expect(term.buffer.active.getLine(49)?.translateToString()).toBe(" 40");
    expect(term.buffer.active.getLine(9)).toBeUndefined();
  });

  it("never treats provider frames as viewport redraws", () => {
    expect(__terminalTesting.isProviderViewportRedraw("codex", "\u001b[1;1H1\r\n2")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("codex", "\u001b[H1\r\n2")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("codex", "\u001b[2J\u001b[H")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("codex", "\u001b[12;1Hstatus")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("opencode", "\u001b[1;1H1\r\n2")).toBe(false);
  });

  it("never routes Claude or Gemini frames through viewport redraws", () => {
    // Claude/Gemini are diff renderers that assume the terminal retained their
    // previous frame; replacing their viewport via a scratch screen corrupts
    // cells (blank rows with a fresh scratch, merged rows with a preserved
    // one — observed live against Claude Code 2.1.173). Their streams are
    // written natively.
    const esc = String.fromCharCode(27);
    expect(__terminalTesting.isProviderViewportRedraw("claude", `${esc}[H1` + "\r\n2")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("claude", `${esc}[2J${esc}[H`)).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("gemini", `${esc}[H1` + "\r\n2")).toBe(false);
  });

  it("keeps rows a partial home-anchored redraw did not repaint when preserving the viewport", async () => {
    // Unit coverage for applyViewportRedrawInPlace's preserveExistingViewport
    // option: with it enabled, rows the frame does not write must keep their
    // prior content instead of going blank.
    const createLine = (text: string) => ({
      clone: () => createLine(text),
      translateToString: () => text,
    });
    function createHeadlessTerm(
      ybase: number,
      options?: ConstructorParameters<typeof HeadlessTerminal>[0],
    ) {
      const lines: ReturnType<typeof createLine>[] = [];
      const internalBuffer = {
        x: 0,
        y: 0,
        ybase,
        ydisp: ybase,
        lines: {
          get: (index: number) => lines[index],
          set: (index: number, value: ReturnType<typeof createLine>) => {
            lines[index] = value;
            terminal.buffer.active.length = Math.max(terminal.buffer.active.length, index + 1);
          },
        },
      };
      const terminal = {
        cols: options?.cols ?? 80,
        rows: options?.rows ?? 24,
        options: { ...(options ?? {}) },
        buffer: {
          active: {
            baseY: internalBuffer.ybase,
            viewportY: internalBuffer.ydisp,
            length: 0,
            getLine: (index: number) => lines[index],
          },
        },
        _core: { _bufferService: { buffer: internalBuffer } },
        write: vi.fn((data: string, callback?: () => void) => {
          const rendered = data
            .replace(new RegExp("\\u001b\\[[0-?]*[ -/]*[@-~]", "g"), "")
            .split(new RegExp("\\r?\\n"))
            .map((line) => line.trimEnd());
          rendered.forEach((line, row) => {
            if (line.length > 0) {
              internalBuffer.lines.set(internalBuffer.ybase + row, createLine(line));
            }
          });
          callback?.();
        }),
        resize: vi.fn(),
        dispose: vi.fn(),
      } as any;
      return terminal;
    }
    // Scratch terminals are created with scrollback 0, so their base stays 0.
    mockHeadlessTerminal.mockImplementation(function MockScratchTerm(
      options?: ConstructorParameters<typeof HeadlessTerminal>[0],
    ) {
      return createHeadlessTerm(0, options);
    });
    const term = createHeadlessTerm(10, { cols: 80, rows: 24 });
    for (let index = 0; index < 24; index += 1) {
      term._core._bufferService.buffer.lines.set(10 + index, createLine(` stale ${index + 1}`));
    }

    const esc = String.fromCharCode(27);
    const applied = await __terminalTesting.applyViewportRedrawInPlace(
      term,
      `${esc}[H header${esc}[K\r\n 130989 tokens${esc}[K`,
      { preserveExistingViewport: true },
    );

    expect(applied).toBe(true);
    expect(term.buffer.active.getLine(10)?.translateToString()).toBe(" header");
    expect(term.buffer.active.getLine(11)?.translateToString()).toBe(" 130989 tokens");
    expect(term.buffer.active.getLine(12)?.translateToString()).toBe(" stale 3");
    expect(term.buffer.active.getLine(33)?.translateToString()).toBe(" stale 24");
  });

  it("journals new substantive rows from provider redraw frames before applying them in place", () => {
    const rows = __terminalTesting.syntheticScrollbackRowsForViewportRedraw(
      "\u001b[H› Print 50 lines\r\n\u001b[38;2;0;0;0m●\u001b[m 1\u001b[K\r\n  2\u001b[K\r\n  3\u001b[K\r\n  gpt-5.5 high · Context 99% left",
      new Set(["number:2"]),
    );

    expect(rows).toEqual(["  1", "  3"]);
    expect(__terminalTesting.appendSyntheticScrollbackRows("", rows)).toBe("\u001b[999;1H  1\r\n  3\r\n");
  });

  it("moves the viewport boundary over duplicate resize repaint rows", () => {
    const createLine = (text: string) => ({
      clone: () => createLine(text),
      translateToString: () => text,
    });
    const lines = [
      ...Array.from({ length: 50 }, (_, index) => createLine(`  ${index + 1}`)),
      createLine(""),
      createLine("✻ Brewed for 1s"),
      ...Array.from({ length: 19 }, (_, index) => createLine(`  ${index + 32}`)),
      createLine(""),
      createLine("✻ Brewed for 1s"),
    ];
    const internalBuffer = {
      ybase: 52,
      ydisp: 52,
      lines: {
        get: (index: number) => lines[index],
        set: (index: number, value: ReturnType<typeof createLine>) => {
          lines[index] = value;
        },
        splice: (start: number, deleteCount: number) => {
          lines.splice(start, deleteCount);
        },
      },
    };
    const term = {
      rows: 21,
      buffer: { active: { baseY: 52, viewportY: 52 } },
      _core: { _bufferService: { buffer: internalBuffer } },
    } as any;

    const trimmed = __terminalTesting.trimOverlappingScrollbackBeforeViewport(term);

    expect(trimmed).toBe(21);
    expect(internalBuffer.ybase).toBe(31);
    expect(internalBuffer.ydisp).toBe(31);
    expect(lines[31].translateToString()).toBe("  32");
    expect(lines[49].translateToString()).toBe("  50");
  });

  it("replaces a duplicated repaint tail even when status rows separate it from the viewport", () => {
    const createLine = (text: string) => ({
      clone: () => createLine(text),
      translateToString: () => text,
    });
    const lines = [
      ...Array.from({ length: 50 }, (_, index) => createLine(`  ${index + 1}`)),
      createLine("✻ Brewed for 1s"),
      createLine("❯ "),
      ...Array.from({ length: 6 }, (_, index) => createLine(`  ${index + 45}`)),
      createLine("✻ Brewed for 1s"),
      createLine("❯ "),
      ...Array.from({ length: 6 }, (_, index) => createLine(`  ${index + 45}`)),
      createLine("✻ Brewed for 1s"),
    ];
    const internalBuffer = {
      ybase: 60,
      ydisp: 60,
      lines: {
        get: (index: number) => lines[index],
        splice: (start: number, deleteCount: number) => {
          lines.splice(start, deleteCount);
        },
      },
    };
    const term = {
      rows: 7,
      buffer: { active: { baseY: 60, viewportY: 60 } },
      _core: { _bufferService: { buffer: internalBuffer } },
    } as any;

    const trimmed = __terminalTesting.trimOverlappingScrollbackBeforeViewport(term);

    expect(trimmed).toBe(8);
    expect(internalBuffer.ybase).toBe(52);
    expect(lines[52].translateToString()).toBe("  45");
    expect(lines[57].translateToString()).toBe("  50");
  });

  it("opens xterm after its host is attached to the document", async () => {
    render(<AgentTerminal sessionId="codex-connected-open" theme="dark" />);

    await waitFor(() => {
      expect(openConnectedStates).toContain(true);
    });

    expect(openConnectedStates).toEqual([true]);
  });

  it("fits the connected terminal before activating the WebGL renderer", async () => {
    render(<AgentTerminal sessionId="codex-fit-before-webgl" theme="dark" />);

    await waitFor(() => {
      expect(mockWebglAddon).toHaveBeenCalled();
    });

    const fitAddon = mockFitAddon.mock.results[mockFitAddon.mock.results.length - 1]?.value as {
      proposeDimensions: ReturnType<typeof vi.fn>;
    };
    expect(fitAddon.proposeDimensions).toHaveBeenCalled();
    expect(fitAddon.proposeDimensions.mock.invocationCallOrder[0]).toBeLessThan(
      mockWebglAddon.mock.invocationCallOrder[0],
    );
  });

  it("waits for physical intersection before promoting a revealed renderer to WebGL", async () => {
    const originalIntersectionObserver = globalThis.IntersectionObserver;
    let intersectionCallback: IntersectionObserverCallback | undefined;
    globalThis.IntersectionObserver = class IntersectionObserver {
      root = null;
      rootMargin = "";
      thresholds = [];
      constructor(callback: IntersectionObserverCallback) {
        intersectionCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() { return []; }
    } as unknown as typeof IntersectionObserver;

    try {
      render(<AgentTerminal sessionId="physical-webgl" theme="dark" />);
      await waitFor(() => expect(mockTerminal).toHaveBeenCalled());
      expect(mockWebglAddon).not.toHaveBeenCalled();

      const host = screen.getByTestId("agent-terminal-host");
      act(() => intersectionCallback?.([{
        isIntersecting: true,
        target: host,
      } as unknown as IntersectionObserverEntry], {} as IntersectionObserver));

      await waitFor(() => expect(mockWebglAddon).toHaveBeenCalledTimes(1));
    } finally {
      globalThis.IntersectionObserver = originalIntersectionObserver;
    }
  });

  it("captures readable terminal output for queue summaries", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0
            ? "\u001b[10;6HTest received.\u001b[15;6H"
            : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="opencode-summary" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(useQueueStore.getState()._agentBuffers["opencode-summary"]).toBe("Test received.");
    });
  });

  it("does not capture Gemini terminal redraws for queue summaries", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0
            ? "⁝ Thinking... (esc to cancel, 8s) press tab twice for more"
            : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="gemini-summary" provider="gemini" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith(
        "⁝ Thinking... (esc to cancel, 8s) press tab twice for more",
        expect.any(Function),
      );
    });

    expect(useQueueStore.getState()._agentBuffers["gemini-summary"]).toBeUndefined();
  });

  it("forwards xterm binary input through the byte-preserving PTY path", async () => {
    render(<AgentTerminal sessionId="codex-2" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onBinary = instance.onBinary.mock.calls[0]?.[0] as ((data: string) => void);

    onBinary(String.fromCharCode(96, 97, 98));

    expect(mockInvoke).toHaveBeenCalledWith("send_binary_input_to_agent", {
      sessionId: "codex-2",
      input: [96, 97, 98],
    });
  });

  it("does not forward OpenCode passive mouse-motion bytes into the provider composer", async () => {
    render(<AgentTerminal sessionId="opencode-mouse-motion" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onBinary = instance.onBinary.mock.calls[0]?.[0] as ((data: string) => void);
    mockInvoke.mockClear();

    onBinary(String.fromCharCode(67, 70, 69));

    expect(mockInvoke).not.toHaveBeenCalledWith("send_binary_input_to_agent", {
      sessionId: "opencode-mouse-motion",
      input: [67, 70, 69],
    });
  });

  it("forwards xterm text input directly through send_input_to_agent", async () => {
    render(<AgentTerminal sessionId="codex-text" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("abc");

    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-text",
      input: "abc",
    });
  });

  it("scrolls to the bottom before forwarding text input when the user has scrolled up", async () => {
    render(<AgentTerminal sessionId="codex-scroll-input" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    instance.buffer.active.viewportY = 2;
    instance.buffer.active.baseY = 10;
    instance.scrollToBottom.mockClear();
    mockInvoke.mockClear();
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("h");

    expect(instance.scrollToBottom).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-scroll-input",
      input: "h",
    });
  });

  it("does not force OpenCode terminals to scroll when forwarding text input", async () => {
    render(<AgentTerminal sessionId="opencode-scroll-input" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    instance.buffer.active.viewportY = 2;
    instance.buffer.active.baseY = 10;
    instance.scrollToBottom.mockClear();
    mockInvoke.mockClear();
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("h");

    expect(instance.scrollToBottom).not.toHaveBeenCalled();
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-scroll-input",
      input: "h",
    });
  });

  it("marks OpenCode terminals as TUI-owned scroll surfaces", async () => {
    render(<AgentTerminal sessionId="opencode-scroll-owner" provider="opencode" theme="dark" />);

    await waitFor(() => {
      expect(screen.getByTestId("agent-terminal-host")).toHaveClass("wardian-terminal--tui-owned-scroll");
    });
  });

  it("reports terminal focus while still focusing the xterm instance on click", async () => {
    const onTerminalFocus = vi.fn();
    render(<AgentTerminal sessionId="codex-focus" theme="dark" onTerminalFocus={onTerminalFocus} />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const host = screen.getByTestId("agent-terminal-host");
    host.focus();
    host.click();

    expect(onTerminalFocus).toHaveBeenCalledTimes(1);
    expect(getLatestTerminalInstance().focus).toHaveBeenCalledTimes(1);
  });

  it("keeps the OpenCode xterm viewport scrollable while hiding terminal scroll chrome", async () => {
    const { readFileSync } = await import("node:fs");
    const { cwd } = await import("node:process");
    const appStyles = readFileSync(`${cwd()}/src/styles/App.css`, "utf8") as string;
    const selector = ".wardian-terminal--tui-owned-scroll .xterm-viewport";
    const ruleStart = appStyles.indexOf(selector);
    const ruleEnd = appStyles.indexOf("}", ruleStart);
    const viewportRule = appStyles.slice(ruleStart, ruleEnd);

    expect(ruleStart).toBeGreaterThanOrEqual(0);
    expect(viewportRule).toContain("scrollbar-width: none");
    expect(viewportRule).not.toContain("overflow-y: hidden");
  });

  it("scrolls provider redraw terminals through the user wheel surface", async () => {
    render(<AgentTerminal sessionId="codex-wheel-scroll" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    instance.buffer.active.baseY = 27;
    instance.buffer.active.viewportY = 27;
    instance.scrollLines.mockClear();
    instance.refresh.mockClear();
    const parser = getLatestHeadlessTerminalInstance();
    parser.scrollToLine.mockClear();

    fireEvent.wheel(screen.getByTestId("agent-terminal-host"), {
      deltaY: -240,
      deltaMode: 0,
    });

    expect(instance.scrollLines).toHaveBeenCalledWith(expect.any(Number));
    expect(instance.scrollLines.mock.calls[0][0]).toBeLessThan(0);
    expect(instance.buffer.active.viewportY).toBeLessThan(27);
    expect(parser.scrollToLine).toHaveBeenCalledWith(instance.buffer.active.viewportY);
    expect(instance.refresh).not.toHaveBeenCalled();
  });

  it("forwards codex enter as a plain carriage return", async () => {
    render(<AgentTerminal sessionId="codex-enter" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("\r");

    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-enter",
      input: "\r",
    });
  });

  it("does not scroll Codex erase-in-display clears into scrollback (avoids composer snapshots)", async () => {
    // Codex's conversation history reaches scrollback natively via its
    // top-anchored DECSTBM scroll region; scrollOnEraseInDisplay would also push
    // the pinned composer/status viewport into scrollback on every ESC[2J
    // repaint, leaving frozen composer snapshots in history. It must stay off.
    render(<AgentTerminal sessionId="codex-3" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.reflowCursorLine).toBe(false);
    expect(terminalOptions.scrollOnEraseInDisplay).toBe(false);
  });

  it("keeps non-Codex erase-in-display behavior at xterm defaults", async () => {
    render(<AgentTerminal sessionId="claude-erase" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.reflowCursorLine).toBe(false);
    expect(terminalOptions.scrollOnEraseInDisplay).toBe(false);
  });

  it("applies the configured terminal font size and refits when it changes", async () => {
    useSettingsStore.setState({ terminalFontSize: 16 });

    render(<AgentTerminal sessionId="codex-font-size" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.fontSize).toBe(16);

    const instance = getLatestTerminalInstance();
    const fitAddon = mockFitAddon.mock.results[mockFitAddon.mock.results.length - 1]?.value as {
      proposeDimensions: ReturnType<typeof vi.fn>;
    };
    await waitFor(() => {
      expect(fitAddon.proposeDimensions).toHaveBeenCalled();
    });
    const baselineFitCalls = fitAddon.proposeDimensions.mock.calls.length;

    act(() => {
      useSettingsStore.getState().setTerminalFontSize(12);
    });

    await waitFor(() => {
      expect(instance.options.fontSize).toBe(12);
      expect(instance.refresh).toHaveBeenCalledWith(0, 23);
      expect(fitAddon.proposeDimensions.mock.calls.length).toBeGreaterThan(baselineFitCalls);
    });
  });

  it("uses the current platform's VS Code-style terminal font stack by default", async () => {
    render(<AgentTerminal sessionId="codex-default-font" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.fontFamily).toBe(defaultTerminalFontFamily());
  });

  it("applies the configured terminal font family and refits when it changes", async () => {
    useSettingsStore.setState({ terminalFontFamily: "JetBrains Mono, monospace" });

    render(<AgentTerminal sessionId="codex-font-family" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.fontFamily).toBe("JetBrains Mono, monospace");

    const instance = getLatestTerminalInstance();
    const fitAddon = mockFitAddon.mock.results[mockFitAddon.mock.results.length - 1]?.value as {
      proposeDimensions: ReturnType<typeof vi.fn>;
    };
    await waitFor(() => {
      expect(fitAddon.proposeDimensions).toHaveBeenCalled();
    });
    const baselineFitCalls = fitAddon.proposeDimensions.mock.calls.length;

    act(() => {
      useSettingsStore.getState().setTerminalFontFamily("Consolas, monospace");
    });

    await waitFor(() => {
      expect(instance.options.fontFamily).toBe("Consolas, monospace");
      expect(instance.refresh).toHaveBeenCalledWith(0, 23);
      expect(fitAddon.proposeDimensions.mock.calls.length).toBeGreaterThan(baselineFitCalls);
    });
  });

  it("loads the WebGL renderer so xterm custom glyphs render block art consistently", async () => {
    render(<AgentTerminal sessionId="claude-webgl" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockWebglAddon).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const webglAddon = mockWebglAddon.mock.results[0]?.value;
    expect(instance.loadAddon).toHaveBeenCalledWith(webglAddon);
  });

  it("does not retry WebGL activation after a startup failure", async () => {
    const webglAddon = {
      onContextLoss: vi.fn(),
      clearTextureAtlas: vi.fn(),
      dispose: vi.fn(),
    };
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockWebglAddon
      .mockImplementationOnce(() => {
        throw new Error("temporary webgl failure");
      })
      .mockImplementationOnce(() => webglAddon as never);

    try {
      render(<AgentTerminal sessionId="claude-webgl-retry" provider="claude" theme="dark" />);

      await waitFor(() => {
        expect(mockWebglAddon).toHaveBeenCalledTimes(1);
      });
      const instance = getLatestTerminalInstance();
      expect(instance.loadAddon).not.toHaveBeenCalledWith(webglAddon);
    } finally {
      warnSpy.mockRestore();
    }
  });

  it("strips Codex scrollback erase while preserving surrounding PTY output", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0 ? "before\u001b[3Jafter\n" : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="codex-4" provider="codex" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("beforeafter\n", expect.any(Function));
    });
  });

  it("waits for usable bounds before mounting and then starts automatically", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;
    let width = 0;
    let height = 0;
    rectSpy.mockImplementation(() => ({
      width,
      height,
      top: 0,
      left: 0,
      right: width,
      bottom: height,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect));
    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="codex-early-poll" theme="dark" />);
      await act(async () => undefined);
      expect(mockTerminal).not.toHaveBeenCalled();
      expect(terminalRendererBudget.has("xterm", "codex-early-poll")).toBe(false);

      width = 900;
      height = 600;
      act(() => resizeCallback?.([], {} as ResizeObserver));

      await waitFor(() => {
        const instance = getLatestTerminalInstance();
        expect(instance.write).toHaveBeenCalledWith("hello from codex\n", expect.any(Function));
      });
      expect(terminalRendererBudget.has("xterm", "codex-early-poll")).toBe(true);
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("renders a small initial PTY tail before draining the full retained scrollback", async () => {
    const fullBackfill = deferred<string | null>();
    const readCalls: Array<Parameters<typeof invoke>[1]> = [];
    const appendTerminalOutput = vi.spyOn(useQueueStore.getState(), "appendAgentTerminalOutput");

    mockInvoke.mockImplementation(async (cmd: string, args?: Parameters<typeof invoke>[1]) => {
      switch (cmd) {
        case "read_agent_pty":
          readCalls.push(args);
          if (readCalls.length === 1) {
            return "recent codex frame\n";
          }
          if (readCalls.length === 2) {
            return fullBackfill.promise;
          }
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="codex-lazy-history" provider="codex" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("recent codex frame\n", expect.any(Function));
    });

    const instance = getLatestTerminalInstance();
    expect(readCalls[0]).toMatchObject({
      sessionId: "codex-lazy-history",
      options: {
        max_bytes: 131_072,
        peek: true,
      },
    });
    expect(instance.write).not.toHaveBeenCalledWith(
      "older retained scrollback\nrecent codex frame\n",
      expect.any(Function),
    );
    expect(appendTerminalOutput).not.toHaveBeenCalled();

    fullBackfill.resolve("older retained scrollback\nrecent codex frame\n");

    await waitFor(() => {
      expect(instance.reset).toHaveBeenCalled();
      expect(instance.write).toHaveBeenCalledWith(
        "older retained scrollback\nrecent codex frame\n",
        expect.any(Function),
      );
    });
    expect(appendTerminalOutput).toHaveBeenCalledTimes(1);
    expect(appendTerminalOutput).toHaveBeenCalledWith(
      "codex-lazy-history",
      "older retained scrollback\nrecent codex frame\n",
      "codex",
    );
  });

  it("drains additional PTY output when the backend emits an output-ready event", async () => {
    let readCount = 0;
    let outputReadyListener: ((event: { payload: { session_id: string } }) => void) | undefined;

    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          if (readCount === 1) return "first frame\n";
          if (readCount === 2) return null;
          if (readCount === 3) return "latest frame\n";
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    mockListen.mockImplementation(async (_eventName, handler) => {
      outputReadyListener = handler as (event: { payload: { session_id: string } }) => void;
      return () => {};
    });

    render(<AgentTerminal sessionId="codex-event" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("first frame\n", expect.any(Function));
    });

    expect(outputReadyListener).toBeDefined();
    outputReadyListener!({ payload: { session_id: "codex-event" } });

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("latest frame\n", expect.any(Function));
    });
  });

  it("clears the cached renderer when the backend replaces the agent terminal", async () => {
    let readCount = 0;
    const listeners = new Map<string, (event: { payload: { session_id: string } }) => void>();
    const onTitleChange = vi.fn();

    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          if (readCount === 1) return "old provider frame\n";
          if (readCount === 2) return null;
          if (readCount === 3) return "fresh provider frame\n";
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: { session_id: string } }) => void);
      return () => {};
    });

    render(<AgentTerminal sessionId="agent-reset" provider="claude" theme="dark" onTitleChange={onTitleChange} />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("old provider frame\n", expect.any(Function));
    });

    const instance = getLatestTerminalInstance();
    instance.clear.mockClear();
    instance.reset.mockClear();
    instance.scrollToBottom.mockClear();
    instance.refresh.mockClear();
    instance.write.mockClear();

    act(() => {
      listeners.get("agent-terminal-cleared")?.({ payload: { session_id: "agent-reset" } });
    });

    expect(instance.reset).toHaveBeenCalledTimes(1);
    expect(instance.scrollToBottom).toHaveBeenCalledTimes(1);
    expect(instance.refresh).toHaveBeenCalledWith(0, 23);
    expect(instance.clear).not.toHaveBeenCalled();
    expect(instance.write).not.toHaveBeenCalledWith("\u001bc");
    expect(onTitleChange).toHaveBeenLastCalledWith("");

    act(() => {
      listeners.get("agent-pty-output-ready")?.({ payload: { session_id: "agent-reset" } });
    });

    await waitFor(() => {
      expect(instance.write).toHaveBeenCalledWith("fresh provider frame\n", expect.any(Function));
    });
  });

  it("force-resizes the new PTY on its first output after a backend terminal clear", async () => {
    const listeners = new Map<string, (event: { payload: { session_id: string } }) => void>();

    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: { session_id: string } }) => void);
      return () => {};
    });

    render(<AgentTerminal sessionId="gemini-clear-size" provider="gemini" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(listeners.has("agent-terminal-cleared")).toBe(true);
    });

    mockInvoke.mockClear();
    act(() => {
      listeners.get("agent-terminal-cleared")?.({ payload: { session_id: "gemini-clear-size" } });
    });

    // Clear must not resize against the dead PTY — backend hasn't spawned the
    // replacement yet, so any invoke here would silently fail and poison state.
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "resize_agent_terminal",
      expect.objectContaining({ sessionId: "gemini-clear-size" }),
    );

    act(() => {
      listeners.get("agent-pty-output-ready")?.({ payload: { session_id: "gemini-clear-size" } });
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
        sessionId: "gemini-clear-size",
        cols: 80,
        rows: 24,
      });
    });
  });

  it("does not write stale PTY output that resolves after a terminal clear", async () => {
    const listeners = new Map<string, (event: { payload: { session_id: string } }) => void>();
    let readCount = 0;
    let resolveStaleRead: ((value: string) => void) | undefined;

    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          if (readCount === 1) {
            return new Promise<string>((resolve) => {
              resolveStaleRead = resolve;
            });
          }
          if (readCount === 2) return null;
          if (readCount === 3) return "fresh frame after restart\n";
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    mockListen.mockImplementation(async (eventName, handler) => {
      listeners.set(eventName, handler as (event: { payload: { session_id: string } }) => void);
      return () => {};
    });

    render(<AgentTerminal sessionId="codex-clear-race" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(resolveStaleRead).toBeDefined();
    });

    const instance = getLatestTerminalInstance();
    instance.write.mockClear();

    act(() => {
      listeners.get("agent-terminal-cleared")?.({ payload: { session_id: "codex-clear-race" } });
      listeners.get("agent-pty-output-ready")?.({ payload: { session_id: "codex-clear-race" } });
    });

    resolveStaleRead!("stale frame from old process\n");

    await waitFor(() => {
      expect(instance.write).toHaveBeenCalledWith("fresh frame after restart\n", expect.any(Function));
    });
    expect(instance.write).not.toHaveBeenCalledWith("stale frame from old process\n", expect.any(Function));
  });

  it("exposes parser scroll geometry in terminal debug snapshots", async () => {
    render(<AgentTerminal sessionId="debug-geometry" theme="dark" />);

    await waitFor(() => {
      const snapshot = window.__wardianTerminalDebug?.snapshot("debug-geometry");
      expect(snapshot).toMatchObject({
        cols: 80,
        rows: 24,
        baseY: expect.any(Number),
        bufferLength: expect.any(Number),
        viewportY: expect.any(Number),
        renderer: {
          cols: 80,
          rows: 24,
          fontFamily: defaultTerminalFontFamily(),
          fontSize: 14,
          webglActive: true,
          webglAttempted: true,
          cssCellWidth: null,
          cssCellHeight: null,
          deviceCellWidth: null,
          deviceCellHeight: null,
        },
      });
    });
  });

  it("exposes a debug scroll-to-top action for rendering audits", async () => {
    render(<AgentTerminal sessionId="debug-scroll" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    instance.buffer.active.viewportY = 10;

    expect(window.__wardianTerminalDebug?.scrollToTop("debug-scroll")).toBe(true);
    expect(instance.scrollToTop).toHaveBeenCalledTimes(1);
    expect(instance.refresh).toHaveBeenCalledWith(0, 23);
  });

  it("exposes debug scroll actions for rendering audit history capture", async () => {
    render(<AgentTerminal sessionId="debug-scroll-history" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    instance.buffer.active.baseY = 80;
    instance.buffer.active.viewportY = 0;
    vi.mocked(instance.scrollToBottom).mockClear();

    expect(window.__wardianTerminalDebug?.scrollToViewportLine("debug-scroll-history", 40)).toBe(true);
    expect(instance.scrollToLine).toHaveBeenCalledWith(40);
    expect(instance.refresh).toHaveBeenCalledWith(0, 23);

    expect(window.__wardianTerminalDebug?.scrollToBottom("debug-scroll-history")).toBe(true);
    expect(instance.scrollToBottom).toHaveBeenCalledTimes(1);
  });

  it("does not resize repeatedly when ResizeObserver fires without a real size change", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="gemini-resize" provider="gemini" theme="dark" />);

      await waitFor(() => {
        expect(mockFitAddon).toHaveBeenCalled();
      });
      await new Promise((resolve) => setTimeout(resolve, 320));

      const terminalInstance = getLatestTerminalInstance();
      const baselineCalls = terminalInstance.resize.mock.calls.length;

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);
      resizeCallback!([], {} as ResizeObserver);
      await new Promise((resolve) => setTimeout(resolve, 40));

      expect(terminalInstance.resize.mock.calls.length).toBe(baselineCalls);
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("coalesces bursty ResizeObserver fits before resizing the xterm grid", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="codex-bursty-fit" provider="codex" theme="dark" />);

      await waitFor(() => {
        expect(mockTerminal).toHaveBeenCalled();
      });

      const instance = getLatestTerminalInstance();
      await new Promise((resolve) => setTimeout(resolve, 80));
      instance.resize.mockClear();
      mockInvoke.mockClear();

      let hostRect = {
        width: 630,
        height: 340,
        top: 0,
        left: 0,
        right: 630,
        bottom: 340,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect;
      rectSpy.mockImplementation(() => hostRect);

      fitDimensions = { cols: 90, rows: 20 };
      hostRect = { ...hostRect, width: 640, height: 350, right: 640, bottom: 350 };
      resizeCallback!([], {} as ResizeObserver);
      fitDimensions = { cols: 100, rows: 24 };
      hostRect = { ...hostRect, width: 720, height: 410, right: 720, bottom: 410 };
      resizeCallback!([], {} as ResizeObserver);
      fitDimensions = { cols: 110, rows: 28 };
      hostRect = { ...hostRect, width: 800, height: 480, right: 800, bottom: 480 };
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(instance.resize).toHaveBeenCalledWith(110, 28);
      });
      expect(instance.resize).not.toHaveBeenCalledWith(90, 20);
      expect(instance.resize).not.toHaveBeenCalledWith(100, 24);
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("re-reports the PTY size when host pixels change but fitted columns stay the same", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="codex-same-grid-refit" provider="codex" theme="dark" />);

      await waitFor(() => {
        expect(mockTerminal).toHaveBeenCalled();
      });
      await new Promise((resolve) => setTimeout(resolve, 80));
      mockInvoke.mockClear();

      fitDimensions = { cols: 80, rows: 24 };
      rectSpy.mockReturnValue({
        width: 812,
        height: 420,
        top: 0,
        left: 0,
        right: 812,
        bottom: 420,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
          sessionId: "codex-same-grid-refit",
          cols: 80,
          rows: 24,
        });
      });
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("does not report the stale terminal grid before a ResizeObserver refit", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="claude-no-stale-grid-report" provider="claude" theme="dark" />);

      await waitFor(() => {
        expect(mockTerminal).toHaveBeenCalled();
      });
      await new Promise((resolve) => setTimeout(resolve, 80));
      mockInvoke.mockClear();

      fitDimensions = { cols: 100, rows: 30 };
      rectSpy.mockReturnValue({
        width: 720,
        height: 420,
        top: 0,
        left: 0,
        right: 720,
        bottom: 420,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
          sessionId: "claude-no-stale-grid-report",
          cols: 100,
          rows: 30,
        });
      });

      const resizeCalls = mockInvoke.mock.calls.filter(
        ([command, payload]) =>
          command === "resize_agent_terminal" &&
          (payload as { sessionId?: string })?.sessionId === "claude-no-stale-grid-report",
      );
      expect(resizeCalls.map(([, payload]) => payload)).toEqual([
        { sessionId: "claude-no-stale-grid-report", cols: 100, rows: 30 },
      ]);
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("floors fitted terminal columns and rows to a reportable PTY size", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="codex-min-size" provider="codex" theme="dark" />);

      await waitFor(() => {
        expect(mockTerminal).toHaveBeenCalled();
      });

      const instance = getLatestTerminalInstance();
      await new Promise((resolve) => setTimeout(resolve, 80));
      instance.resize.mockClear();
      mockInvoke.mockClear();
      fitDimensions = { cols: 7, rows: 4 };
      rectSpy.mockReturnValue({
        width: 64,
        height: 80,
        top: 0,
        left: 0,
        right: 64,
        bottom: 80,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(instance.resize).toHaveBeenCalledWith(20, 8);
      });
      await waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
          sessionId: "codex-min-size",
          cols: 20,
          rows: 8,
        });
      });
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("reports each distinct backend PTY row resize during bursty terminal resize events", async () => {
    render(<AgentTerminal sessionId="claude-resize" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);

    // Drop any initial-mount fit report so the assertion isolates the three
    // deliberate onResize events below (matches the sibling resize tests).
    mockInvoke.mockClear();
    onResize({ cols: 120, rows: 40 });
    onResize({ cols: 121, rows: 41 });
    onResize({ cols: 122, rows: 42 });

    await waitFor(() => {
      const resizeCalls = mockInvoke.mock.calls.filter(
        ([command, payload]) =>
          command === "resize_agent_terminal" &&
          (payload as { sessionId?: string })?.sessionId === "claude-resize",
      );
      expect(resizeCalls.map(([, payload]) => payload)).toEqual([
        { sessionId: "claude-resize", cols: 120, rows: 40 },
        { sessionId: "claude-resize", cols: 121, rows: 41 },
        { sessionId: "claude-resize", cols: 122, rows: 42 },
      ]);
    });
  });

  it.each(["codex", "claude"] as const)(
    "does not blank %s during resize while waiting for the provider to repaint",
    async (provider) => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0 ? "\u001b[Hheader\u001b[K\r\nprompt\u001b[K" : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    try {
      render(<AgentTerminal sessionId={`${provider}-visible-resize`} provider={provider} theme="dark" />);

      await waitFor(() => {
        const instance = getLatestTerminalInstance();
        expect(instance.write).toHaveBeenCalledWith(
          expect.stringContaining("header"),
          expect.any(Function),
        );
      });

      const instance = getLatestTerminalInstance();
      instance.write.mockClear();
      fitDimensions = { cols: 100, rows: 30 };
      rectSpy.mockReturnValue({
        width: 720,
        height: 420,
        top: 0,
        left: 0,
        right: 720,
        bottom: 420,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(instance.resize).toHaveBeenCalledWith(100, 30);
      });
      expect(instance.write).not.toHaveBeenCalledWith("\u001b[2J\u001b[H", expect.any(Function));
      expect(instance.write).not.toHaveBeenCalledWith(expect.stringContaining("\u001b[3J"), expect.anything());
      } finally {
        globalThis.ResizeObserver = originalResizeObserver;
      }
    },
  );

  it("does not blank Gemini during resize while waiting for the provider to repaint", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return readCount++ === 0 ? "\u001b[Hheader\u001b[K\r\nprompt\u001b[K" : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    try {
      render(<AgentTerminal sessionId="gemini-visible-resize" provider="gemini" theme="dark" />);

      await waitFor(() => {
        const instance = getLatestTerminalInstance();
        expect(instance.write).toHaveBeenCalledWith(
          expect.stringContaining("header"),
          expect.any(Function),
        );
      });

      const instance = getLatestTerminalInstance();
      instance.write.mockClear();
      fitDimensions = { cols: 100, rows: 30 };
      rectSpy.mockReturnValue({
        width: 720,
        height: 420,
        top: 0,
        left: 0,
        right: 720,
        bottom: 420,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(instance.resize).toHaveBeenCalledWith(100, 30);
      });
      expect(instance.write).not.toHaveBeenCalledWith("\u001b[2J\u001b[H", expect.any(Function));
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("reports each distinct backend PTY column-only resize", async () => {
    render(<AgentTerminal sessionId="claude-column-resize" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);

    onResize({ cols: 120, rows: 24 });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
        sessionId: "claude-column-resize",
        cols: 120,
        rows: 24,
      });
    });
    mockInvoke.mockClear();

    onResize({ cols: 121, rows: 24 });
    onResize({ cols: 122, rows: 24 });

    await waitFor(() => {
      const resizeCalls = mockInvoke.mock.calls.filter(
        ([command, payload]) =>
          command === "resize_agent_terminal" &&
          (payload as { sessionId?: string })?.sessionId === "claude-column-resize",
      );
      expect(resizeCalls.map(([, payload]) => payload)).toEqual([
        { sessionId: "claude-column-resize", cols: 121, rows: 24 },
        { sessionId: "claude-column-resize", cols: 122, rows: 24 },
      ]);
    });
  });

  it("reports backend PTY resize while the user is inspecting scrollback", async () => {
    render(<AgentTerminal sessionId="codex-scrollback-resize" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);
    instance.buffer.active.baseY = 50;
    instance.buffer.active.viewportY = 12;
    mockInvoke.mockClear();

    onResize({ cols: 100, rows: 30 });

    await waitFor(
      () => {
        expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
          sessionId: "codex-scrollback-resize",
          cols: 100,
          rows: 30,
        });
      },
      { timeout: 400 },
    );
  });

  it("does not force the viewport to the bottom during resize while the user is inspecting scrollback", async () => {
    render(<AgentTerminal sessionId="codex-scroll-position-resize" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);
    instance.buffer.active.baseY = 50;
    instance.buffer.active.viewportY = 12;
    instance.scrollToBottom.mockClear();

    onResize({ cols: 100, rows: 30 });

    expect(instance.scrollToBottom).not.toHaveBeenCalled();
    expect(instance.buffer.active.viewportY).toBe(12);
  });

  it("resizes frontend xterm while the user is inspecting scrollback", async () => {
    const originalResizeObserver = globalThis.ResizeObserver;
    let resizeCallback: ResizeObserverCallback | undefined;

    globalThis.ResizeObserver = class ResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        resizeCallback = callback;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;

    try {
      render(<AgentTerminal sessionId="codex-frontend-scroll-resize" provider="codex" theme="dark" />);

      await waitFor(() => {
        expect(mockTerminal).toHaveBeenCalled();
      });

      const instance = getLatestTerminalInstance();
      await new Promise((resolve) => setTimeout(resolve, 80));
      instance.buffer.active.baseY = 50;
      instance.buffer.active.viewportY = 12;
      instance.resize.mockClear();
      mockInvoke.mockClear();
      fitDimensions = { cols: 100, rows: 30 };
      rectSpy.mockReturnValue({
        width: 720,
        height: 420,
        top: 0,
        left: 0,
        right: 720,
        bottom: 420,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect);

      expect(resizeCallback).toBeDefined();
      resizeCallback!([], {} as ResizeObserver);

      await waitFor(() => {
        expect(instance.resize).toHaveBeenCalledWith(100, 30);
      });
      await waitFor(
        () => {
          expect(mockInvoke).toHaveBeenCalledWith("resize_agent_terminal", {
            sessionId: "codex-frontend-scroll-resize",
            cols: 100,
            rows: 30,
          });
        },
        { timeout: 500 },
      );

      const snapshot = window.__wardianTerminalDebug?.snapshot("codex-frontend-scroll-resize");
      expect(snapshot?.fitCount).toBeGreaterThan(0);
      expect(snapshot?.resizeCount).toBeGreaterThan(0);
      expect(snapshot?.lastReportedSize).toEqual({ cols: 100, rows: 30 });
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });

  it("replies to OpenCode terminal capability probes while keeping terminal stdin enabled", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          if (readCount === 1) {
            return "\u001b[6n\u001b[>0q\u001b[?u\u001b[?1016$p\u001b[?1004$p\u001b[?2004$p\u001b[?2027$p\u001b[?2031$p\u001b[?2026$p\u001b[?996n\u001b[?1004h\u001b[14t\u001b]4;0;?\u0007\u001b]10;?\u0007\u001b]11;?\u001b\\";
          }
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="opencode-1" provider="opencode" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.options.disableStdin).toBe(false);
      expect(instance.write).toHaveBeenCalledWith(
        "\u001b[6n\u001b[>0q\u001b[?u\u001b[?996n\u001b[?1004h\u001b[14t\u001b]4;0;?\u0007\u001b]10;?\u0007\u001b]11;?\u001b\\",
        expect.any(Function),
      );
    });

    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[1;1R",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001bP>|xterm.js 6.0.0\u001b\\",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?0u",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?1016;2$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?1004;2$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?2004;2$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?2027;0$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?2031;0$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?2026;0$y",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[?997;1n",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[4;600;900t",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b[I",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b]4;0;rgb:1a/1a/1a\u001b\\",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b]10;rgb:eb/eb/eb\u001b\\",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b]11;rgb:1a/1a/1a\u001b\\",
    });
  });

  it("keeps Antigravity dark-mode terminal default muted while primary text is normalized separately", async () => {
    const probe = "\u001b]10;?\u001b\\";
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          return readCount === 1 ? probe : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="antigravity-terminal-colors" provider="antigravity" theme="dark" />);

    await waitFor(() => {
      const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
        string,
        unknown
      >;
      expect(terminalOptions.theme).toMatchObject({ foreground: "#c9d1d9" });
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "antigravity-terminal-colors",
        input: "\u001b]10;rgb:c9/d1/d9\u001b\\",
      });
    });
  });

  it("answers OpenCode light-dark probes from the Wardian terminal theme", async () => {
    const matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: matchMedia,
    });

    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          return readCount === 1 ? "\u001b[?996n" : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="opencode-light" provider="opencode" theme="light" />);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "opencode-light",
        input: "\u001b[?997;2n",
      });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-light",
      input: "\u001b[?997;1n",
    });
  });

  it("does not reply to Codex OSC 10/11 probes (the modern ConPTY answers them)", async () => {
    const probe = "\u001b]10;?\u001b\\\u001b]11;?\u001b\\";
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      switch (cmd) {
        case "read_agent_pty":
          if (
            typeof args === "object" &&
            args !== null &&
            "options" in args &&
            typeof args.options === "object" &&
            args.options !== null &&
            "peek" in args.options &&
            args.options.peek === true
          ) {
            return probe;
          }
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="codex-theme-probe" provider="codex" theme="light" />);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("read_agent_pty", expect.anything());
    });
    // Codex's OSC 10/11 color probes are answered by the bundled modern ConPTY
    // (OpenConsole); a second Wardian reply would leak into codex's composer.
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-theme-probe",
      input: "\u001b]10;rgb:11/18/27\u001b\\",
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-theme-probe",
      input: "\u001b]11;rgb:fc/fa/f5\u001b\\",
    });
  });

  it("renders Codex's dark composer background as a light-mode fill", async () => {
    const codexComposerFrame = "\u001b[48;2;41;41;41m\n\u001b[K";
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      switch (cmd) {
        case "read_agent_pty":
          if (
            typeof args === "object" &&
            args !== null &&
            "options" in args &&
            typeof args.options === "object" &&
            args.options !== null &&
            "peek" in args.options &&
            args.options.peek === true
          ) {
            return codexComposerFrame;
          }
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="codex-light-composer" provider="codex" theme="light" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith(expect.stringContaining("\u001b[48;2;242;240;235m"), expect.any(Function));
      expect(instance.write).not.toHaveBeenCalledWith(expect.stringContaining("\u001b[48;2;41;41;41m"), expect.any(Function));
    });
  });

  it("recolors Codex via composer replay on a real theme swap without typing color replies into its stdin", async () => {
    const codexComposerFrame = "\u001b[48;2;41;41;41m\n\u001b[K";
    let peekCount = 0;
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      switch (cmd) {
        case "read_agent_pty":
          if (
            typeof args === "object" &&
            args !== null &&
            "options" in args &&
            typeof args.options === "object" &&
            args.options !== null &&
            "peek" in args.options &&
            args.options.peek === true
          ) {
            peekCount += 1;
            return peekCount === 1 ? codexComposerFrame : null;
          }
          return null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    const view = render(<AgentTerminal sessionId="codex-live-theme" provider="codex" theme="light" />);

    await waitFor(() => {
      expect(getLatestTerminalInstance()).toBeTruthy();
    });

    const instance = getLatestTerminalInstance();
    await waitFor(() => {
      expect(instance.write).toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;242;240;235m"),
        expect.any(Function),
      );
    });
    instance.write.mockClear();
    instance.reset.mockClear();
    instance.refresh.mockClear();
    instance.buffer.active.cursorY = 12;
    mockInvoke.mockClear();
    view.rerender(<AgentTerminal sessionId="codex-live-theme" provider="codex" theme="dark" />);

    // Codex recolors via the Wardian-side composer replay, NOT by pushing color
    // replies into its stdin: those are terminal->app responses, so codex's prompt
    // line editor would just type them in as literal "[?997;1n]11;rgb:..." text.
    const codexEsc = String.fromCharCode(27);
    const codexSt = codexEsc + String.fromCharCode(92);
    await waitFor(() => {
      expect(instance.reset).toHaveBeenCalled();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-live-theme",
      input: codexEsc + "[?997;1n",
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-live-theme",
      input: codexEsc + "]11;rgb:1a/1a/1a" + codexSt,
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-live-theme",
      input: codexEsc + "]10;rgb:eb/eb/eb" + codexSt,
    });

    await waitFor(() => {
      expect(instance.reset).toHaveBeenCalled();
      expect(instance.write).toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;41;41;41m"),
        expect.any(Function),
      );
      expect(instance.write).not.toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;242;240;235m"),
        expect.any(Function),
      );
      const darkWriteOrder = instance.write.mock.invocationCallOrder.find(
        (_order: number, index: number) =>
          String(instance.write.mock.calls[index]?.[0] ?? "").includes("\u001b[48;2;41;41;41m"),
      );
      expect(darkWriteOrder).toBeDefined();
      expect(
        instance.refresh.mock.invocationCallOrder.some((order: number) => order > darkWriteOrder!),
      ).toBe(true);
      expect(instance.write).toHaveBeenCalledWith(
        expect.stringContaining("\u001b[48;2;41;41;41m\u001b[38;2;235;235;235m\u001b[2K"),
        expect.any(Function),
      );
      const composerBlockWrite = instance.write.mock.calls.find(([data]: [string]) =>
        data.includes("\u001b[12;1H") &&
        data.includes("\u001b[13;1H") &&
        data.includes("\u001b[14;1H"),
      );
      expect(composerBlockWrite).toBeDefined();
    });
  });

  it("does not replay a delayed Codex preview into a replacement renderer", async () => {
    const previewGate = deferred<string | null>();
    mockInvoke.mockImplementation(async (command: string, args?: unknown) => {
      const request = (args as { request?: { presentation_id?: string } } | undefined)?.request;
      const presentationId = request?.presentation_id ?? "codex-preview-owner";
      if (command === "register_terminal_presentation") {
        return modernRegistrationResult(presentationId, presentationId);
      }
      if (command === "subscribe_terminal_events") {
        return {
          broker_state: modernBrokerState(presentationId),
          initial_snapshot: modernSnapshot(),
        };
      }
      if (command === "update_terminal_presentation") {
        return modernRegistrationResult(presentationId, presentationId);
      }
      if (command === "request_terminal_snapshot") return modernSnapshot();
      if (command === "report_terminal_presentation_viewport") {
        return modernRegistrationResult(presentationId, presentationId).presentation;
      }
      if (command === "read_agent_pty") {
        const options = (args as { options?: { peek?: boolean } } | undefined)?.options;
        return options?.peek ? previewGate.promise : null;
      }
      if (command === "unregister_terminal_presentation") {
        return modernBrokerState();
      }
      if (command === "unsubscribe_terminal_events") return undefined;
      return null;
    });

    const view = render(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="codex-preview-owner"
        provider="codex"
        theme="dark"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "register_terminal_presentation",
      expect.anything(),
    ));

    const originalRenderer = getLatestTerminalInstance();
    for (const result of mockSerializeAddon.mock.results) {
      result.value.serialize.mockReturnValue("");
    }
    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="codex-preview-owner"
        provider="codex"
        theme="light"
      />,
    );
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "read_agent_pty",
      expect.objectContaining({ options: expect.objectContaining({ peek: true }) }),
    ));

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="codex-preview-owner"
        visibility="hidden"
        renderState="suspended"
        provider="codex"
        theme="light"
      />,
    );
    await waitFor(() => expect(originalRenderer.dispose).toHaveBeenCalledTimes(1));

    view.rerender(
      <AgentTerminal
        sessionId="modern-agent"
        presentationId="codex-preview-owner"
        visibility="visible"
        renderState="mounted"
        provider="codex"
        theme="light"
      />,
    );
    await waitFor(() => expect(getLatestTerminalInstance()).not.toBe(originalRenderer));
    const replacementRenderer = getLatestTerminalInstance();
    const parser = getLatestHeadlessTerminalInstance();
    replacementRenderer.write.mockClear();
    replacementRenderer.reset.mockClear();
    replacementRenderer.scrollToBottom.mockClear();
    replacementRenderer.refresh.mockClear();
    parser.write.mockClear();

    await act(async () => {
      previewGate.resolve("delayed codex preview");
      await Promise.resolve();
    });

    await waitFor(() => expect(parser.write).toHaveBeenCalledWith(
      expect.stringContaining("delayed codex preview"),
      expect.any(Function),
    ));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(replacementRenderer.write).not.toHaveBeenCalledWith(
      expect.stringContaining("delayed codex preview"),
      expect.any(Function),
    );
    expect(replacementRenderer.reset).not.toHaveBeenCalled();
    expect(replacementRenderer.scrollToBottom).not.toHaveBeenCalled();
    expect(replacementRenderer.refresh).not.toHaveBeenCalled();
  });

  it("strips OpenCode synchronized-output toggles before writing to xterm", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          return readCount === 1 ? "\u001b[?2026hhello\u001b[?2026l" : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="opencode-sync" provider="opencode" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("hello", expect.any(Function));
    });
  });

  it("strips OpenCode DECRQM queries before writing to xterm", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          return readCount === 1
            ? "\u001b[?1016$ptest\u001b[?1004$p\u001b[?2026$p"
            : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="opencode-decrqm" provider="opencode" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("test", expect.any(Function));
    });
  });

  // jsdom canvases have no real rendering contexts; stub both the 2D context
  // the snapshot overlay draws into and the WebGL context the release path
  // loses.
  function stubCanvasContexts() {
    const drawImage = vi.fn();
    const loseContext = vi.fn();
    const spy = vi
      .spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockImplementation(((type: string) =>
        type === "2d"
          ? ({ drawImage } as unknown as CanvasRenderingContext2D)
          : ({
              getExtension: (name: string) =>
                name === "WEBGL_lose_context" ? { loseContext } : null,
            } as unknown as RenderingContext)) as never);
    return { spy, drawImage, loseContext };
  }

  function querySnapshotOverlay() {
    return document.querySelector('[data-testid="terminal-snapshot-overlay"]') as HTMLCanvasElement | null;
  }

  it("freezes the last WebGL frame as a cosmetic overlay on demotion and removes it on promotion", async () => {
    render(<AgentTerminal sessionId="snap-overlay" theme="dark" />);

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("snap-overlay")?.renderer?.webglActive).toBe(true);
    });

    const instance = getLatestTerminalInstance();
    const sourceCanvas = document.createElement("canvas");
    sourceCanvas.width = 320;
    sourceCanvas.height = 200;
    (instance.element as HTMLElement).appendChild(sourceCanvas);
    const { spy, drawImage } = stubCanvasContexts();

    try {
      act(() => {
        __terminalTesting.demoteSessionToDom("snap-overlay");
      });

      const overlay = querySnapshotOverlay();
      expect(overlay).toBeTruthy();
      // Strictly cosmetic: clicks, wheel, and selection must reach the live
      // terminal underneath.
      expect(overlay?.style.pointerEvents).toBe("none");
      expect(drawImage).toHaveBeenCalledWith(sourceCanvas, 0, 0);
      expect(window.__wardianTerminalDebug?.snapshot("snap-overlay")?.renderer?.webglActive).toBe(false);

      act(() => {
        __terminalTesting.promoteSessionToWebgl("snap-overlay");
      });

      expect(querySnapshotOverlay()).toBeNull();
      expect(window.__wardianTerminalDebug?.snapshot("snap-overlay")?.renderer?.webglActive).toBe(true);
    } finally {
      spy.mockRestore();
    }
  });

  it("refits a reused renderer and removes its stale snapshot before revealing it again", async () => {
    const view = render(
      <AgentTerminal
        sessionId="snap-reveal"
        theme="dark"
        visibility="visible"
        renderState="mounted"
      />,
    );

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("snap-reveal")?.renderer?.webglActive).toBe(true);
      expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
    });

    const instance = getLatestTerminalInstance();
    const sourceCanvas = document.createElement("canvas");
    sourceCanvas.width = 320;
    sourceCanvas.height = 200;
    (instance.element as HTMLElement).appendChild(sourceCanvas);
    const { spy } = stubCanvasContexts();

    try {
      act(() => {
        __terminalTesting.demoteSessionToDom("snap-reveal");
      });
      expect(querySnapshotOverlay()).toBeTruthy();

      view.rerender(
        <AgentTerminal
          sessionId="snap-reveal"
          theme="dark"
          visibility="hidden"
          renderState="suspended"
        />,
      );
      await waitFor(() => {
        expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "hidden" });
        expect(instance.dispose).toHaveBeenCalled();
        expect(querySnapshotOverlay()).toBeNull();
      });

      fitDimensions = { cols: 100, rows: 30 };
      view.rerender(
        <AgentTerminal
          sessionId="snap-reveal"
          theme="dark"
          visibility="visible"
          renderState="mounted"
        />,
      );

      await waitFor(() => {
        const revealedInstance = getLatestTerminalInstance();
        const revealedFitAddon = mockFitAddon.mock.results[mockFitAddon.mock.results.length - 1]?.value as {
          proposeDimensions: ReturnType<typeof vi.fn>;
        };
        expect(screen.getByTestId("agent-terminal-host")).toHaveStyle({ visibility: "visible" });
        expect(querySnapshotOverlay()).toBeNull();
        expect(revealedFitAddon.proposeDimensions).toHaveBeenCalled();
        expect(revealedInstance).not.toBe(instance);
        expect(revealedInstance.cols).toBe(100);
        expect(revealedInstance.rows).toBe(30);
      });
    } finally {
      spy.mockRestore();
    }
  });

  it("drops a stale snapshot overlay when fresh output arrives for the demoted terminal", async () => {
    const listeners = new Map<string, (event: { payload?: { session_id?: string } }) => void>();
    mockListen.mockImplementation((async (event: string, handler: never) => {
      listeners.set(event, handler);
      return () => {};
    }) as never);
    const pendingReads: string[] = ["hello from codex\n"];
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          return pendingReads.shift() ?? null;
        case "terminal_link_target_exists":
          return true;
        default:
          return null;
      }
    });

    render(<AgentTerminal sessionId="snap-stale" theme="dark" />);

    await waitFor(() => {
      expect(window.__wardianTerminalDebug?.snapshot("snap-stale")?.renderer?.webglActive).toBe(true);
    });

    const instance = getLatestTerminalInstance();
    const sourceCanvas = document.createElement("canvas");
    sourceCanvas.width = 320;
    sourceCanvas.height = 200;
    (instance.element as HTMLElement).appendChild(sourceCanvas);
    const { spy } = stubCanvasContexts();

    try {
      act(() => {
        __terminalTesting.demoteSessionToDom("snap-stale");
      });
      expect(querySnapshotOverlay()).toBeTruthy();

      // Output for the demoted terminal must lift the frozen still so the
      // live DOM rendering shows the stream.
      pendingReads.push("late output while demoted\n");
      await act(async () => {
        listeners.get("agent-pty-output-ready")?.({ payload: { session_id: "snap-stale" } });
      });

      await waitFor(() => {
        expect(querySnapshotOverlay()).toBeNull();
      });
      expect(instance.write).toHaveBeenCalledWith(
        expect.stringContaining("late output while demoted"),
        expect.any(Function),
      );
    } finally {
      spy.mockRestore();
    }
  });

  it("releases the WebGL context after a terminal leaves the viewport and re-promotes on re-entry", async () => {
    type ObservationCallback = (
      entries: Array<{ isIntersecting: boolean; target?: Element }>,
      observer: unknown,
    ) => void;
    const ioCallbacks: ObservationCallback[] = [];
    const OriginalIntersectionObserver = globalThis.IntersectionObserver;
    globalThis.IntersectionObserver = class {
      root = null;
      rootMargin = "";
      thresholds = [];
      private callback: ObservationCallback;
      constructor(callback: ObservationCallback) {
        this.callback = callback;
        ioCallbacks.push(callback);
      }
      observe(target: Element) {
        this.callback([{ isIntersecting: true, target }], this);
      }
      unobserve() {}
      disconnect() {}
      takeRecords() {
        return [];
      }
    } as unknown as typeof IntersectionObserver;
    const { spy, loseContext } = stubCanvasContexts();

    try {
      render(<AgentTerminal sessionId="vis-scope" theme="dark" />);

      await waitFor(() => {
        expect(window.__wardianTerminalDebug?.snapshot("vis-scope")?.renderer?.webglActive).toBe(true);
      });

      const instance = getLatestTerminalInstance();
      const sourceCanvas = document.createElement("canvas");
      sourceCanvas.width = 320;
      sourceCanvas.height = 200;
      (instance.element as HTMLElement).appendChild(sourceCanvas);
      const leaveAndReenter = ioCallbacks[ioCallbacks.length - 1];

      vi.useFakeTimers();
      try {
        act(() => {
          leaveAndReenter([{ isIntersecting: false }], null);
        });
        // Still within the grace window: layout churn must not thrash contexts.
        expect(window.__wardianTerminalDebug?.snapshot("vis-scope")?.renderer?.webglActive).toBe(true);

        act(() => {
          vi.advanceTimersByTime(1_000);
        });
        expect(window.__wardianTerminalDebug?.snapshot("vis-scope")?.renderer?.webglActive).toBe(false);
        expect(loseContext).toHaveBeenCalled();
        expect(querySnapshotOverlay()).toBeTruthy();

        act(() => {
          leaveAndReenter([{ isIntersecting: true }], null);
        });
        expect(window.__wardianTerminalDebug?.snapshot("vis-scope")?.renderer?.webglActive).toBe(true);
        expect(querySnapshotOverlay()).toBeNull();
      } finally {
        vi.useRealTimers();
      }
    } finally {
      globalThis.IntersectionObserver = OriginalIntersectionObserver;
      spy.mockRestore();
    }
  });

});
