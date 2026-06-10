import { render, waitFor, cleanup, act, screen, fireEvent } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { Terminal as HeadlessTerminal } from "@xterm/headless";
import { SerializeAddon } from "@xterm/addon-serialize";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { AgentTerminal, __terminalTesting, shouldExposeTerminalDebug } from "./AgentTerminal";
import { defaultTerminalFontFamily, useSettingsStore } from "../../store/useSettingsStore";
import { useQueueStore } from "../../store/useQueueStore";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockTerminal = vi.mocked(Terminal);
const mockHeadlessTerminal = vi.mocked(HeadlessTerminal);
const mockSerializeAddon = vi.mocked(SerializeAddon);
const mockFitAddon = vi.mocked(FitAddon);
const mockWebglAddon = vi.mocked(WebglAddon);

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

  afterEach(() => {
    rectSpy.mockRestore();
    cleanup();
  });

  it("only exposes terminal debug hooks in dev or with an explicit debug flag", () => {
    expect(shouldExposeTerminalDebug({ DEV: false, VITE_WARDIAN_TERMINAL_DEBUG: undefined })).toBe(false);
    expect(shouldExposeTerminalDebug({ DEV: true, VITE_WARDIAN_TERMINAL_DEBUG: undefined })).toBe(true);
    expect(shouldExposeTerminalDebug({ DEV: false, VITE_WARDIAN_TERMINAL_DEBUG: "1" })).toBe(true);
  });

  it("installs conservative terminal shortcuts on the renderer", async () => {
    render(<AgentTerminal sessionId="codex-shortcuts" theme="dark" />);

    await waitFor(() => {
      expect(getLatestTerminalInstance().attachCustomKeyEventHandler).toHaveBeenCalledTimes(1);
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

  it("disposes the renderer once a session stays unmounted past the grace window", async () => {
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
      expect(window.__wardianTerminalDebug?.snapshot("codex-grace")?.renderer).toBeNull();
    } finally {
      vi.useRealTimers();
    }
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

  it("treats only top-left cursor addresses as provider viewport redraws", () => {
    expect(__terminalTesting.isProviderViewportRedraw("claude", "\u001b[1;1H1\r\n2")).toBe(true);
    expect(__terminalTesting.isProviderViewportRedraw("claude", "\u001b[H1\r\n2")).toBe(true);
    expect(__terminalTesting.isProviderViewportRedraw("codex", "\u001b[2J\u001b[H")).toBe(true);
    expect(__terminalTesting.isProviderViewportRedraw("claude", "\u001b[12;1Hstatus")).toBe(false);
    expect(__terminalTesting.isProviderViewportRedraw("opencode", "\u001b[1;1H1\r\n2")).toBe(false);
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
    expect(instance.refresh).toHaveBeenCalledWith(0, 23);
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

  it("preserves Codex primary-buffer clears into scrollback", async () => {
    render(<AgentTerminal sessionId="codex-3" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const terminalOptions = mockTerminal.mock.calls[mockTerminal.mock.calls.length - 1]?.[0] as Record<
      string,
      unknown
    >;
    expect(terminalOptions.reflowCursorLine).toBe(false);
    expect(terminalOptions.scrollOnEraseInDisplay).toBe(true);
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

  it("homes transient TUI redraws before shrinking rows so complex glyph rows do not reflow", async () => {
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

  it("reports each distinct backend PTY row resize during bursty terminal resize events", async () => {
    render(<AgentTerminal sessionId="claude-resize" provider="claude" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = getLatestTerminalInstance();
    const onResize = instance.onResize.mock.calls[0]?.[0] as ((size: { cols: number; rows: number }) => void);

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

  it("replies to Codex OSC 10/11 probes during the initial preview window", async () => {
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
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "codex-theme-probe",
        input: "\u001b]10;rgb:11/18/27\u001b\\",
      });
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "codex-theme-probe",
        input: "\u001b]11;rgb:fc/fa/f5\u001b\\",
      });
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

  it("pushes updated Codex terminal colors when Wardian switches back to dark mode", async () => {
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
    mockInvoke.mockClear();
    view.rerender(<AgentTerminal sessionId="codex-live-theme" provider="codex" theme="dark" />);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "codex-live-theme",
        input: "\u001b[?997;1n",
      });
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "codex-live-theme",
        input: "\u001b]11;rgb:02/04/02\u001b\\",
      });
      expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
        sessionId: "codex-live-theme",
        input: "\u001b]10;rgb:EE/F2/EE\u001b\\",
      });
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
