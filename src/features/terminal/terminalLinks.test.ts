import { describe, expect, it, vi } from "vitest";
import { findTerminalLinks, findValidatedTerminalLinks, installTerminalLinkProvider } from "./terminalLinks";

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
    const getLine = vi.fn(() => ({ translateToString: () => "https://wardian.org/docs" }));
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
    });

    const links = await new Promise<any[] | undefined>((resolve) => {
      provider?.provideLinks(7, resolve);
    });

    expect(getLine).toHaveBeenCalledWith(6);
    expect(links?.[0].range).toEqual({
      start: { x: 1, y: 7 },
      end: { x: "https://wardian.org/docs".length, y: 7 },
    });
  });

  it("opens URL links through the URL opener", async () => {
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

    const links = await new Promise<any[] | undefined>((resolve) => {
      provider?.provideLinks(1, resolve);
    });
    links?.[0].activate(new MouseEvent("click"), links[0].text);

    expect(openUrl).toHaveBeenCalledWith("https://wardian.org/docs");
    expect(openFile).not.toHaveBeenCalled();
  });
});
