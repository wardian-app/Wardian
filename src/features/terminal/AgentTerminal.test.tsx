import { render, waitFor, cleanup, act } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { SerializeAddon } from "@xterm/addon-serialize";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { AgentTerminal } from "./AgentTerminal";
import { defaultTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockTerminal = vi.mocked(Terminal);
const mockSerializeAddon = vi.mocked(SerializeAddon);
const mockFitAddon = vi.mocked(FitAddon);
const mockWebglAddon = vi.mocked(WebglAddon);

function getLatestTerminalInstance() {
  return mockTerminal.mock.results[mockTerminal.mock.results.length - 1]?.value as any;
}

describe("AgentTerminal scrollback", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>;
  let fitDimensions: { cols: number; rows: number };

  beforeEach(() => {
    vi.clearAllMocks();
    useSettingsStore.setState({ terminalFontSize: 14, terminalFontFamily: "" });
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
        default:
          return null;
      }
    });

    mockListen.mockResolvedValue(() => {});
    fitDimensions = { cols: 80, rows: 24 };

    mockTerminal.mockImplementation(() => {
      const state = { serializedState: "" };
      let resizeHandler: ((size: { cols: number; rows: number }) => void) | undefined;
      const terminal = {
        open: vi.fn(),
        write: vi.fn((data: string, callback?: () => void) => {
          state.serializedState += data;
          callback?.();
        }),
        resize: vi.fn((cols: number, rows: number) => {
          terminal.cols = cols;
          terminal.rows = rows;
          resizeHandler?.({ cols, rows });
        }),
        onData: vi.fn(),
        onBinary: vi.fn(),
        onTitleChange: vi.fn(),
        onResize: vi.fn((handler: (size: { cols: number; rows: number }) => void) => {
          resizeHandler = handler;
        }),
        dispose: vi.fn(),
        focus: vi.fn(),
        scrollToBottom: vi.fn(),
        buffer: { active: { baseY: 10, viewportY: 10 } },
        refresh: vi.fn(),
        cols: 80,
        rows: 24,
        options: {},
        unicode: { activeVersion: "" },
        loadAddon: vi.fn((addon: { __termState?: typeof state }) => {
          if (addon && "__termState" in addon) {
            addon.__termState = state;
          }
        }),
      } as any;
      return terminal;
    });

    mockSerializeAddon.mockImplementation(() => {
      return {
        __termState: undefined as { serializedState: string } | undefined,
        serialize: vi.fn(function (this: { __termState?: { serializedState: string } }) {
          return this.__termState?.serializedState ?? "";
        }),
        dispose: vi.fn(),
      } as any;
    });

    mockFitAddon.mockImplementation(() => {
      return {
        fit: vi.fn(),
        proposeDimensions: vi.fn(() => fitDimensions),
        dispose: vi.fn(),
      } as any;
    });

    mockWebglAddon.mockImplementation(() => {
      return {
        onContextLoss: vi.fn(),
        clearTextureAtlas: vi.fn(),
        dispose: vi.fn(),
      } as any;
    });
  });

  afterEach(() => {
    rectSpy.mockRestore();
    cleanup();
  });

  it("reuses the live xterm instance on remount while preserving prior session state", async () => {
    const firstRender = render(
      <AgentTerminal sessionId="codex-1" theme="dark" />,
    );

    await waitFor(() => {
      const firstInstance = getLatestTerminalInstance();
      expect(firstInstance.write).toHaveBeenCalledWith("hello from codex\n", expect.any(Function));
    });

    firstRender.unmount();

    render(<AgentTerminal sessionId="codex-1" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalledTimes(1);
    });

    await waitFor(() => {
      const secondInstance = getLatestTerminalInstance();
      expect(secondInstance.write).toHaveBeenCalledWith("hello from codex\n", expect.any(Function));
    });

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
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("h");

    expect(instance.scrollToBottom).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-scroll-input",
      input: "h",
    });
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

  it("keeps xterm erase-in-display behavior at the default terminal semantics", async () => {
    render(<AgentTerminal sessionId="codex-3" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.reflowCursorLine).toBe(false);
    expect("scrollOnEraseInDisplay" in terminalOptions).toBe(false);
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

  it("forwards codex PTY control sequences untouched", async () => {
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
      expect(instance.write).toHaveBeenCalledWith("before\u001b[3Jafter\n", expect.any(Function));
    });
  });

  it("starts PTY polling even before fit reports a usable layout size", async () => {
    rectSpy.mockReturnValue({
      width: 0,
      height: 0,
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect);

    render(<AgentTerminal sessionId="codex-early-poll" theme="dark" />);

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith("hello from codex\n", expect.any(Function));
    });
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
      await new Promise((resolve) => setTimeout(resolve, 80));

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

  it("homes transient TUI redraws before shrinking rows so resize does not promote the old frame", async () => {
    let readCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "read_agent_pty":
          readCount += 1;
          return readCount === 1
            ? "\u001b[?2026h\u001b[38;2;215;119;87m\u001b[H ▐▛███▜▌   Claude Code v2.1.101\u001b[K\r\n▝▜█████▛▘  Sonnet 4.6 · Claude Pro\u001b[K\u001b[?2026l"
            : null;
        case "resize_agent_terminal":
          return null;
        default:
          return null;
      }
    });

    const { rerender } = render(
      <AgentTerminal sessionId="claude-transient-shrink" provider="claude" isMaximized theme="dark" />,
    );

    await waitFor(() => {
      const instance = getLatestTerminalInstance();
      expect(instance.write).toHaveBeenCalledWith(expect.stringContaining("Claude Code"), expect.any(Function));
    });

    rectSpy.mockReturnValue({
      width: 900,
      height: 300,
      top: 0,
      left: 0,
      right: 900,
      bottom: 300,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect);
    fitDimensions = { cols: 80, rows: 12 };

    rerender(
      <AgentTerminal
        sessionId="claude-transient-shrink"
        provider="claude"
        isMaximized={false}
        theme="dark"
      />,
    );

    await waitFor(
      () => {
        const instance = getLatestTerminalInstance();
        expect(instance.write).toHaveBeenCalledWith("\u001b[H", expect.any(Function));
        expect(instance.resize).toHaveBeenCalledWith(80, 12);
      },
      { timeout: 400 },
    );

    const instance = getLatestTerminalInstance();
    const homeWriteOrder = instance.write.mock.invocationCallOrder.find(
      (_order: number, index: number) => instance.write.mock.calls[index]?.[0] === "\u001b[H",
    );
    const shrinkOrder = instance.resize.mock.invocationCallOrder.find(
      (_order: number, index: number) =>
        instance.resize.mock.calls[index]?.[0] === 80 && instance.resize.mock.calls[index]?.[1] === 12,
    );
    expect(homeWriteOrder).toBeLessThan(shrinkOrder);
  });

  it("debounces backend PTY resize reports during bursty terminal resize events", async () => {
    render(<AgentTerminal sessionId="claude-resize" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);

    onResize({ cols: 120, rows: 40 });
    onResize({ cols: 121, rows: 41 });
    onResize({ cols: 122, rows: 42 });

    await waitFor(
      () => {
        const resizeCalls = mockInvoke.mock.calls.filter(
          ([command, payload]) =>
            command === "resize_agent_terminal" &&
            (payload as { sessionId?: string })?.sessionId === "claude-resize",
        );
        expect(resizeCalls).toHaveLength(1);
        expect(resizeCalls[0]?.[1]).toMatchObject({
          sessionId: "claude-resize",
          cols: 122,
          rows: 42,
        });
      },
      { timeout: 400 },
    );
  });

  it("replies to OpenCode terminal capability probes before rendering the PTY output", async () => {
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
      input: "\u001b]4;0;rgb:02/04/02\u001b\\",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b]10;rgb:EE/F2/EE\u001b\\",
    });
    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "opencode-1",
      input: "\u001b]11;rgb:02/04/02\u001b\\",
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

});
