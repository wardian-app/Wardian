import { render, waitFor, cleanup } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { AgentTerminal } from "./AgentTerminal";

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockTerminal = vi.mocked(Terminal);

describe("AgentTerminal scrollback", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.clearAllMocks();
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

    mockTerminal.mockImplementation(() => {
      return {
        open: vi.fn(),
        write: vi.fn((_data: string, callback?: () => void) => callback?.()),
        onData: vi.fn(),
        onBinary: vi.fn(),
        onTitleChange: vi.fn(),
        onResize: vi.fn(),
        dispose: vi.fn(),
        focus: vi.fn(),
        loadAddon: vi.fn(),
        scrollToBottom: vi.fn(),
        refresh: vi.fn(),
        cols: 80,
        rows: 24,
        options: {},
        unicode: { activeVersion: "" },
      } as any;
    });
  });

  afterEach(() => {
    rectSpy.mockRestore();
    cleanup();
  });

  it("reuses the same xterm instance when the terminal remounts", async () => {
    const firstRender = render(
      <AgentTerminal sessionId="codex-1" theme="dark" />,
    );

    await waitFor(() => {
      const firstInstance = mockTerminal.mock.results[0]?.value as any;
      expect(firstInstance.write).toHaveBeenCalledWith("hello from codex\n");
    });

    firstRender.unmount();

    render(<AgentTerminal sessionId="codex-1" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalledTimes(1);
    });
  });

  it("forwards xterm binary input through the byte-preserving PTY path", async () => {
    render(<AgentTerminal sessionId="codex-2" theme="dark" />);

    await waitFor(() => {
      expect(mockTerminal).toHaveBeenCalled();
    });

    const instance = mockTerminal.mock.results[0]?.value as any;
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

    const instance = mockTerminal.mock.results[0]?.value as any;
    const onData = instance.onData.mock.calls[0]?.[0] as ((data: string) => void);

    onData("abc");

    expect(mockInvoke).toHaveBeenCalledWith("send_input_to_agent", {
      sessionId: "codex-text",
      input: "abc",
    });
  });

  it("enables erase-in-display scrollback preservation for codex terminals", () => {
    render(<AgentTerminal sessionId="codex-3" provider="codex" theme="dark" />);

    expect(mockTerminal).toHaveBeenCalledWith(
      expect.objectContaining({
        reflowCursorLine: false,
        scrollOnEraseInDisplay: true,
      }),
    );
  });

  it("strips erase-scrollback control sequences from codex PTY output", async () => {
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
      const instance = mockTerminal.mock.results[0]?.value as any;
      expect(instance.write).toHaveBeenCalledWith("beforeafter\n");
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
      const instance = mockTerminal.mock.results[0]?.value as any;
      expect(instance.write).toHaveBeenCalledWith("hello from codex\n");
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
      const instance = mockTerminal.mock.results[0]?.value as any;
      expect(instance.write).toHaveBeenCalledWith("first frame\n");
    });

    expect(outputReadyListener).toBeDefined();
    outputReadyListener!({ payload: { session_id: "codex-event" } });

    await waitFor(() => {
      const instance = mockTerminal.mock.results[0]?.value as any;
      expect(instance.write).toHaveBeenCalledWith("latest frame\n");
    });
  });

});
