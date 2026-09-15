import { describe, expect, it, vi } from "vitest";
import {
  findTerminalLinks,
  findValidatedTerminalLinks,
  installTerminalLinkProvider,
  openHttpUrlInBrowser,
} from "./terminalLinks";

describe("findTerminalLinks", () => {
  it("detects URLs and file paths with line suffixes", () => {
    const links = findTerminalLinks("see https://wardian.org and src/App.tsx:12:3");

    expect(links.map((link) => ({ kind: link.kind, text: link.text, target: link.target }))).toEqual([
      { kind: "url", text: "https://wardian.org", target: "https://wardian.org" },
      { kind: "file", text: "src/App.tsx:12:3", target: "src/App.tsx" },
    ]);
  });

  it("resolves relative file links against the provided Windows base path", () => {
    const [link] = findTerminalLinks("src/App.tsx:12", "C:\\repo");

    expect(link.target).toBe("C:\\repo\\src\\App.tsx");
  });

  it("resolves relative file links against the provided POSIX base path", () => {
    const [link] = findTerminalLinks("../README.md", "/home/me/repo/src");

    expect(link.target).toBe("/home/me/repo/README.md");
  });

  it("keeps Windows absolute paths absolute and strips line suffixes", () => {
    const [link] = findTerminalLinks("open C:\\repo\\src\\App.tsx:12:3", "D:\\other");

    expect(link).toMatchObject({
      kind: "file",
      text: "C:\\repo\\src\\App.tsx:12:3",
      target: "C:\\repo\\src\\App.tsx",
    });
  });

  it("keeps POSIX absolute paths absolute and strips line suffixes", () => {
    const [link] = findTerminalLinks("open /home/me/repo/src/App.tsx:12:3", "/tmp/other");

    expect(link).toMatchObject({
      kind: "file",
      text: "/home/me/repo/src/App.tsx:12:3",
      target: "/home/me/repo/src/App.tsx",
    });
  });

  it("maps file URI authorities to UNC paths without rebasing them to the workspace", () => {
    const [unc] = findTerminalLinks("file://server/share/report.md", "C:\\repo");
    const [localhost] = findTerminalLinks("file://localhost/C:/repo/report.md", "D:\\other");
    const [drive] = findTerminalLinks("file:///C:/repo/report.md", "D:\\other");

    expect(unc).toMatchObject({
      kind: "file",
      text: "file://server/share/report.md",
      target: "\\\\server\\share\\report.md",
    });
    expect(unc.target).not.toBe("C:\\repo\\server\\share\\report.md");
    expect(localhost.target).toBe("C:/repo/report.md");
    expect(drive.target).toBe("C:/repo/report.md");
  });

  it("trims trailing sentence punctuation from detected links", () => {
    const links = findTerminalLinks("Open https://wardian.org/docs, then src/App.tsx.");

    expect(links.map((link) => link.text)).toEqual([
      "https://wardian.org/docs",
      "src/App.tsx",
    ]);
  });

  it("does not treat slash-delimited prose as a file path without validation", () => {
    expect(findTerminalLinks("risk fields: stage/reason/risk")).toEqual([]);
  });

  it("does not treat slash-prefixed command names as file paths without validation", () => {
    expect(findTerminalLinks("try /model to change the provider model")).toEqual([]);
  });

  it("detects Visual Studio style line suffixes for known file paths", () => {
    const links = findTerminalLinks("src/App.tsx(12,3): error TS1005");

    expect(links.map((link) => ({ kind: link.kind, text: link.text, target: link.target }))).toEqual([
      { kind: "file", text: "src/App.tsx(12,3)", target: "src/App.tsx" },
    ]);
  });

  it("detects rendered and unsupported file families for shared opening routing", () => {
    const links = findTerminalLinks("Open report.pdf, diagram.png, or contract.docx");

    expect(links.map((link) => ({ kind: link.kind, text: link.text, target: link.target }))).toEqual([
      { kind: "file", text: "report.pdf", target: "report.pdf" },
      { kind: "file", text: "diagram.png", target: "diagram.png" },
      { kind: "file", text: "contract.docx", target: "contract.docx" },
    ]);
  });

  it("links extensionless slash paths only when the resolved target exists", async () => {
    const validateFile = vi.fn(async (path: string) => path === "C:\\repo\\bin\\wardian");

    const links = await findValidatedTerminalLinks("built bin/wardian", "C:\\repo", validateFile);

    expect(validateFile).toHaveBeenCalledWith("C:\\repo\\bin\\wardian");
    expect(links.map((link) => ({ kind: link.kind, text: link.text, target: link.target }))).toEqual([
      { kind: "file", text: "bin/wardian", target: "C:\\repo\\bin\\wardian" },
    ]);
  });

  it("removes known-extension file links when validation fails", async () => {
    const links = await findValidatedTerminalLinks("see src/App.tsx:12", "C:\\repo", vi.fn(async () => false));

    expect(links).toEqual([]);
  });

  it("does not link extensionless slash paths when validation fails", async () => {
    const links = await findValidatedTerminalLinks("risk fields: stage/reason/risk", "C:\\repo", vi.fn(async () => false));

    expect(links).toEqual([]);
  });
});

describe("installTerminalLinkProvider", () => {
  it("reports ordinary URLs synchronously through the first provider", () => {
    const openUrl = vi.fn(async () => {});
    const text = "https://wardian.org/docs";
    let callbackCalled = false;
    type TestLink = { text: string; activate: (...args: unknown[]) => void };
    let links: TestLink[] | undefined;
    const term = {
      registerLinkProvider: vi.fn((provider: { provideLinks: (line: number, callback: (links: TestLink[] | undefined) => void) => void }) => {
        if (!links) {
          provider.provideLinks(1, (nextLinks: TestLink[] | undefined) => {
            callbackCalled = true;
            links = nextLinks;
          });
        }
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: () => ({ translateToString: () => text }),
        },
      },
    } as unknown as Parameters<typeof installTerminalLinkProvider>[0];

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openUrl,
    });

    expect(callbackCalled).toBe(true);
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(links?.[0].text).toBe(text);
    expect(openUrl).toHaveBeenCalledWith(text);
  });

  it("opens file links through the configured external editor", async () => {
    const openFile = vi.fn(async () => {});
    const openUrl = vi.fn(async () => {});
    let provider:
      | { provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }
      | null = null;
    const term = {
      registerLinkProvider: vi.fn((nextProvider) => {
        provider = nextProvider;
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: () => ({ translateToString: () => "src/App.tsx:12" }),
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getBasePath: () => "C:\\repo",
      getExternalEditor: () => ({
        external_editor: "vscode",
        external_editor_custom_executable: null,
      }),
      validateFile: vi.fn(async () => true),
      openFile,
      openUrl,
    });

    const links = await new Promise<any[] | undefined>((resolve) => {
      provider?.provideLinks(1, resolve);
    });
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(openFile).toHaveBeenCalledWith("C:\\repo\\src\\App.tsx", {
      external_editor: "vscode",
      external_editor_custom_executable: null,
    });
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("reads 1-based xterm link rows from the 0-based buffer line", async () => {
    let provider:
      | { provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }
      | null = null;
    const getLine = vi.fn(() => ({ translateToString: () => "src/App.tsx:12" }));
    const term = {
      registerLinkProvider: vi.fn((nextProvider) => {
        provider = nextProvider;
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openFile: vi.fn(async () => {}),
      openUrl: vi.fn(async () => {}),
      validateFile: vi.fn(async () => true),
    });

    const links = await new Promise<any[] | undefined>((resolve) => {
      provider?.provideLinks(7, resolve);
    });

    expect(getLine).toHaveBeenCalledWith(6);
    expect(links?.[0].range).toEqual({
      start: { x: 1, y: 7 },
      end: { x: "src/App.tsx:12".length, y: 7 },
    });
  });

  it("opens URL links through the URL opener", () => {
    const openFile = vi.fn(async () => {});
    const openUrl = vi.fn(async () => {});
    const providers: Array<{ provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }> = [];
    const term = {
      registerLinkProvider: vi.fn((nextProvider) => {
        providers.push(nextProvider);
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: () => ({ translateToString: () => "https://wardian.org/docs" }),
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openFile,
      openUrl,
    });

    let links: any[] | undefined;
    providers[0].provideLinks(1, (nextLinks) => {
      links = nextLinks;
    });
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/docs");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("routes xterm OSC 8 hyperlinks through the URL opener", () => {
    const openUrl = vi.fn(async () => {});
    const term = {
      options: {},
      registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
      buffer: {
        active: {
          getLine: () => undefined,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openUrl,
    });

    term.options.linkHandler.activate(
      new MouseEvent("click"),
      "https://wardian.org/from-osc",
      { start: { x: 1, y: 1 }, end: { x: 24, y: 1 } },
    );

    expect(term.options.linkHandler.allowNonHttpProtocols).toBe(true);
    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/from-osc");
  });

  it("routes only validated OSC 8 file hyperlinks through the external editor", async () => {
    const openFile = vi.fn(async () => {});
    const validateFile = vi.fn(async (path: string) => path === "C:/repo/src/App.tsx");
    const onOpenError = vi.fn();
    const term = {
      options: {},
      registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
      buffer: {
        active: {
          getLine: () => undefined,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getBasePath: () => "C:\\repo",
      getExternalEditor: () => ({
        external_editor: "vscode",
        external_editor_custom_executable: null,
      }),
      openFile,
      onOpenError,
      validateFile,
    });

    term.options.linkHandler.activate(
      new MouseEvent("click"),
      "file:///C:/repo/src/App.tsx",
      { start: { x: 1, y: 1 }, end: { x: 28, y: 1 } },
    );

    await vi.waitFor(() => expect(openFile).toHaveBeenCalled());

    expect(term.options.linkHandler.allowNonHttpProtocols).toBe(true);
    expect(validateFile).toHaveBeenCalledWith("C:/repo/src/App.tsx");
    expect(openFile).toHaveBeenCalledWith("C:/repo/src/App.tsx", {
      external_editor: "vscode",
      external_editor_custom_executable: null,
    });
    expect(onOpenError).not.toHaveBeenCalled();
  });

  it("refuses OSC 8 file hyperlinks when target validation fails", async () => {
    const openFile = vi.fn(async () => {});
    const validateFile = vi.fn(async () => false);
    const onOpenError = vi.fn();
    const term = {
      options: {},
      registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
      buffer: {
        active: {
          getLine: () => undefined,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getBasePath: () => "C:\\repo",
      getExternalEditor: () => ({
        external_editor: "vscode",
        external_editor_custom_executable: null,
      }),
      openFile,
      onOpenError,
      validateFile,
    });

    term.options.linkHandler.activate(
      new MouseEvent("click"),
      "file:///C:/repo/src/secret.tsx",
      { start: { x: 1, y: 1 }, end: { x: 31, y: 1 } },
    );

    await vi.waitFor(() => expect(onOpenError).toHaveBeenCalledWith(
      "Failed to open terminal link: file target was not validated",
    ));

    expect(validateFile).toHaveBeenCalledWith("C:/repo/src/secret.tsx");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("opens a validated OSC 8 UNC file hyperlink without workspace rebasing", async () => {
    const openFile = vi.fn(async () => {});
    const validateFile = vi.fn(async (path: string) => path === "\\\\server\\share\\report.md");
    const onOpenError = vi.fn();
    const term = {
      options: {},
      registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
      buffer: {
        active: {
          getLine: () => undefined,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getBasePath: () => "C:\\repo",
      getExternalEditor: () => ({
        external_editor: "vscode",
        external_editor_custom_executable: null,
      }),
      openFile,
      onOpenError,
      validateFile,
    });

    term.options.linkHandler.activate(
      new MouseEvent("click"),
      "file://server/share/report.md",
      { start: { x: 1, y: 1 }, end: { x: 28, y: 1 } },
    );

    await vi.waitFor(() => expect(openFile).toHaveBeenCalled());

    expect(validateFile).toHaveBeenCalledWith("\\\\server\\share\\report.md");
    expect(openFile).toHaveBeenCalledWith("\\\\server\\share\\report.md", {
      external_editor: "vscode",
      external_editor_custom_executable: null,
    });
    expect(openFile).not.toHaveBeenCalledWith("C:\\repo\\server\\share\\report.md", expect.anything());
    expect(onOpenError).not.toHaveBeenCalled();
  });

  it("rejects unsupported OSC 8 schemes without invoking a URL or file opener", () => {
    const openUrl = vi.fn(async () => {});
    const openFile = vi.fn(async () => {});
    const onOpenError = vi.fn();
    const term = {
      options: {},
      registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
      buffer: {
        active: {
          getLine: () => undefined,
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      onOpenError,
      openFile,
      openUrl,
    });

    term.options.linkHandler.activate(
      new MouseEvent("click"),
      "javascript:alert(1)",
      { start: { x: 1, y: 1 }, end: { x: 22, y: 1 } },
    );

    expect(openUrl).not.toHaveBeenCalled();
    expect(openFile).not.toHaveBeenCalled();
    expect(onOpenError).toHaveBeenCalledWith("Failed to open terminal link: unsupported URL scheme");
  });

  it("limits remote terminal linkification to HTTP URLs", async () => {
    const openUrl = vi.fn(async () => {});
    const providers: Array<{ provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }> = [];
    const term = {
      options: {},
      registerLinkProvider: vi.fn((provider) => {
        providers.push(provider);
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: () => ({ translateToString: () => "file:///C:/host/report.md https://wardian.org/docs" }),
        },
      },
    } as any;

    installTerminalLinkProvider(term, { httpOnly: true, openUrl });

    expect(term.options.linkHandler.allowNonHttpProtocols).toBe(false);
    expect(providers).toHaveLength(1);
    const links = await new Promise<any[] | undefined>((resolve) => providers[0].provideLinks(1, resolve));
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(links?.map((link) => link.text)).toEqual(["https://wardian.org/docs"]);
    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/docs");
  });

  it("opens a safe HTTP URL synchronously through the browser path", async () => {
    const openWindow = vi.spyOn(window, "open").mockReturnValue({} as Window);

    await openHttpUrlInBrowser("https://wardian.org/docs");

    expect(openWindow).toHaveBeenCalledWith("https://wardian.org/docs", "_blank", "noopener,noreferrer");
  });

  it("opens URLs that wrap across physical terminal rows", async () => {
    const openUrl = vi.fn(async () => {});
    const providers: Array<{ provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }> = [];
    const rows = [
      { isWrapped: false, translateToString: (trimRight?: boolean) => trimRight ? "prefix https://wardi" : "prefix https://wardi" },
      { isWrapped: true, translateToString: (trimRight?: boolean) => trimRight ? "an.org/docs suffix" : "an.org/docs suffix" },
    ];
    const term = {
      cols: 20,
      registerLinkProvider: vi.fn((nextProvider) => {
        providers.push(nextProvider);
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: (index: number) => rows[index],
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openFile: vi.fn(async () => {}),
      openUrl,
    });

    const firstRowLinks = await new Promise<any[] | undefined>((resolve) => {
      providers[0].provideLinks(1, resolve);
    });
    const secondRowLinks = await new Promise<any[] | undefined>((resolve) => {
      providers[0].provideLinks(2, resolve);
    });

    expect(firstRowLinks?.[0]).toMatchObject({
      range: {
        start: { x: 8, y: 1 },
        end: { x: 11, y: 2 },
      },
      text: "https://wardian.org/docs",
    });
    expect(secondRowLinks?.[0]).toMatchObject({
      range: firstRowLinks?.[0].range,
      text: firstRowLinks?.[0].text,
    });

    secondRowLinks?.[0].activate(new MouseEvent("click"), secondRowLinks[0].text);

    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/docs");
  });

  it("opens URLs that provider TUIs hard-wrap onto indented continuation rows", async () => {
    const openUrl = vi.fn(async () => {});
    const providers: Array<{ provideLinks: (line: number, callback: (links: any[] | undefined) => void) => void }> = [];
    const rows = [
      { isWrapped: false, translateToString: () => "› Terminal link smoke https://wardian.org/terminal-link-" },
      { isWrapped: false, translateToString: () => "  validation/123/wrapped-segment-wrapped-" },
      { isWrapped: false, translateToString: () => "  segment" },
    ];
    const term = {
      cols: 80,
      registerLinkProvider: vi.fn((nextProvider) => {
        providers.push(nextProvider);
        return { dispose: vi.fn() };
      }),
      buffer: {
        active: {
          getLine: (index: number) => rows[index],
        },
      },
    } as any;

    installTerminalLinkProvider(term, {
      getExternalEditor: () => ({
        external_editor: "system",
        external_editor_custom_executable: null,
      }),
      openFile: vi.fn(async () => {}),
      openUrl,
    });

    const links = await new Promise<any[] | undefined>((resolve) => {
      providers[0].provideLinks(2, resolve);
    });

    expect(links?.[0]).toMatchObject({
      range: {
        start: { x: 23, y: 1 },
        end: { x: 9, y: 3 },
      },
      text: "https://wardian.org/terminal-link-validation/123/wrapped-segment-wrapped-segment",
    });

    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/terminal-link-validation/123/wrapped-segment-wrapped-segment");
  });
});
