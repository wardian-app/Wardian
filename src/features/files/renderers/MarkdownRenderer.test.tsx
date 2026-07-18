import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import MarkdownRenderer, { resolveLocalMarkdownTarget } from "./MarkdownRenderer";

const mockOpenUrl = vi.mocked(openUrl);

function snapshot(revision = 1): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/docs/readme.md",
    subscription_id: "subscription-1",
    revision,
    descriptor: {
      schema: 1,
      canonical_path: "C:/work/docs/readme.md",
      display_name: "readme.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "markdown",
      size_bytes: 120,
      line_count: 5,
      content_hash: `hash-${revision}`,
      modified_at_ms: revision,
      capabilities: { preview: true, changes: true, draft: true, stream: false },
      unavailable_reason: null,
    },
  };
}

function props(client: FileResourceClient, on_open_file = vi.fn(), revision = 1) {
  return {
    snapshot: snapshot(revision),
    client,
    lifecycle: { visible: true },
    on_open_file,
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  };
}

describe("MarkdownRenderer", () => {
  it("renders the immutable dirty working buffer without rereading disk", async () => {
    const client = { readText: vi.fn() } as unknown as FileResourceClient;
    render(<MarkdownRenderer
      {...props(client)}
      buffer_snapshot={Object.freeze({
        resource_id: snapshot().resource_id,
        revision: 1,
        buffer_generation: 3,
        text: "# Unsaved heading",
        dirty: true,
        read_only: false,
      })}
    />);

    expect(await screen.findByRole("heading", { name: "Unsaved heading" })).toBeInTheDocument();
    expect(client.readText).not.toHaveBeenCalled();
  });

  it("disables raw HTML, rejects unsafe schemes, and routes local links through Wardian", async () => {
    const onOpenFile = vi.fn();
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1,
        resource_id: snapshot().resource_id,
        revision: 1,
        text: [
          "<script>window.__unsafe = true</script>",
          "[Sibling](../other.md)",
          "[Web](https://example.test/docs)",
          "[Unsafe](javascript:alert(1))",
        ].join("\n\n"),
      }),
    } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client, onOpenFile)} />);

    const local = await screen.findByRole("link", { name: "Sibling" });
    expect(document.querySelector("script")).toBeNull();
    expect(screen.getByText("Unsafe").closest("a")).toBeNull();
    fireEvent.click(local);
    expect(onOpenFile).toHaveBeenCalledWith("C:/work/other.md");
    const web = screen.getByRole("link", { name: "Web" });
    const auxiliary = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    fireEvent(web, auxiliary);
    expect(auxiliary.defaultPrevented).toBe(false);
    expect(mockOpenUrl).not.toHaveBeenCalled();
    fireEvent.click(web);
    await waitFor(() => expect(mockOpenUrl).toHaveBeenCalledWith("https://example.test/docs"));
  });

  it("ignores a stale revision read that resolves after a newer revision", async () => {
    let resolveFirst: ((value: object) => void) | undefined;
    const first = new Promise<object>((resolve) => { resolveFirst = resolve; });
    const client = {
      readText: vi.fn()
        .mockReturnValueOnce(first)
        .mockResolvedValueOnce({
          schema: 1,
          resource_id: snapshot(2).resource_id,
          revision: 2,
          text: "New revision",
        }),
    } as unknown as FileResourceClient;
    const view = render(<MarkdownRenderer {...props(client)} />);
    view.rerender(<MarkdownRenderer {...props(client, vi.fn(), 2)} />);
    expect(await screen.findByText("New revision")).toBeInTheDocument();
    resolveFirst?.({
      schema: 1,
      resource_id: snapshot().resource_id,
      revision: 1,
      text: "Stale revision",
    });
    await Promise.resolve();
    expect(screen.queryByText("Stale revision")).toBeNull();
  });

  it("does not read unavailable Markdown", () => {
    const client = { readText: vi.fn() } as unknown as FileResourceClient;
    const unavailable = snapshot();
    unavailable.descriptor.capabilities.preview = false;
    unavailable.descriptor.unavailable_reason = "monaco_line_limit_exceeded";
    render(<MarkdownRenderer {...props(client)} snapshot={unavailable} />);
    expect(client.readText).not.toHaveBeenCalled();
  });

  it("accepts the registry's validated Markdown MIME fallback", async () => {
    const fallback = snapshot();
    fallback.descriptor.renderer_kind = "unsupported";
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1, resource_id: fallback.resource_id, revision: 1, text: "Fallback",
    }) } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client)} snapshot={fallback} />);
    expect(await screen.findByText("Fallback")).toBeInTheDocument();
  });

  it("preserves Windows roots and keeps document fragments inside the preview", async () => {
    const onOpenFile = vi.fn();
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1,
        resource_id: snapshot().resource_id,
        revision: 1,
        text: "# Heading\n\n[Root](/docs/a.md) [Bare](other.md) [Jump](#heading)",
      }),
    } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client, onOpenFile)} />);
    const heading = await screen.findByRole("heading", { name: "Heading" });
    heading.scrollIntoView = vi.fn();
    fireEvent.click(screen.getByRole("link", { name: "Root" }));
    fireEvent.click(screen.getByRole("link", { name: "Bare" }));
    fireEvent.click(screen.getByRole("link", { name: "Jump" }));
    expect(onOpenFile).toHaveBeenNthCalledWith(1, "C:/docs/a.md");
    expect(onOpenFile).toHaveBeenNthCalledWith(2, "C:/work/docs/other.md");
    expect(onOpenFile).toHaveBeenCalledTimes(2);
    expect(heading).toHaveAttribute("id", "heading");
    expect(heading.scrollIntoView).toHaveBeenCalled();
    expect(mockOpenUrl).not.toHaveBeenCalledWith("#heading");
  });

  it("keeps POSIX root-relative targets rooted and rejects unsafe schemes", () => {
    expect(resolveLocalMarkdownTarget("/work/readme.md", "/docs/a.md")).toBe("/docs/a.md");
    expect(resolveLocalMarkdownTarget("C:/work/readme.md", "/docs/a.md")).toBe("C:/docs/a.md");
    expect(resolveLocalMarkdownTarget("//?/C:/work/readme.md", "/docs/a.md")).toBe("//?/C:/docs/a.md");
    expect(resolveLocalMarkdownTarget("//server/share/work/readme.md", "/docs/a.md")).toBe("//server/share/docs/a.md");
    expect(resolveLocalMarkdownTarget("//?/UNC/server/share/work/readme.md", "/docs/a.md"))
      .toBe("//?/UNC/server/share/docs/a.md");
    expect(resolveLocalMarkdownTarget(
      "C:/work/readme.md",
      "file://server/share/folder%20name/report.md",
    )).toBe("//server/share/folder name/report.md");
    expect(resolveLocalMarkdownTarget(
      "C:/work/readme.md",
      "file:///C:/work/folder%20name/report.md",
    )).toBe("C:/work/folder name/report.md");
    expect(resolveLocalMarkdownTarget(
      "C:/work/readme.md",
      "file://LOCALHOST/C:/work/report.md",
    )).toBe("C:/work/report.md");
    expect(resolveLocalMarkdownTarget("C:/work/readme.md", "other.md#part")).toBe("C:/work/other.md");
  });

  it("preserves literal backslashes in POSIX source filenames and directories", () => {
    expect(resolveLocalMarkdownTarget("/work/docs/read\\me.md", "other.md"))
      .toBe("/work/docs/other.md");
    expect(resolveLocalMarkdownTarget("/work/dir\\name/readme.md", "other.md"))
      .toBe("/work/dir\\name/other.md");
  });

  it("preserves a literal backslash in a POSIX relative target", () => {
    expect(resolveLocalMarkdownTarget("/work/docs/readme.md", "draft\\notes.md"))
      .toBe("/work/docs/draft\\notes.md");
  });

  it("treats backslashes as separators for a Windows relative target", () => {
    expect(resolveLocalMarkdownTarget("C:\\work\\docs\\readme.md", "..\\other\\report.md"))
      .toBe("C:/work/other/report.md");
    expect(resolveLocalMarkdownTarget("/work/docs/readme.md", "D:\\other\\report.md"))
      .toBe("D:/other/report.md");
    expect(resolveLocalMarkdownTarget("/work/docs/readme.md", "\\\\server\\share\\report.md"))
      .toBe("//server/share/report.md");
  });

  it("preserves an encoded POSIX backslash in a file URL target", () => {
    expect(resolveLocalMarkdownTarget("/work/docs/readme.md", "file:///tmp/a%5Cb.md"))
      .toBe("/tmp/a\\b.md");
  });

  it("routes file URL UNC authorities through the authorized local-file callback", async () => {
    mockOpenUrl.mockClear();
    const onOpenFile = vi.fn();
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1, resource_id: snapshot().resource_id, revision: 1,
      text: "[Shared report](file://server/share/reports/agent%20output.md)",
    }) } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client, onOpenFile)} />);

    fireEvent.click(await screen.findByRole("link", { name: "Shared report" }));
    expect(onOpenFile).toHaveBeenCalledWith("//server/share/reports/agent output.md");
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it("keeps local targets non-navigable while routing primary and auxiliary activation", async () => {
    const onOpenFile = vi.fn();
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1, resource_id: snapshot().resource_id, revision: 1,
      text: "[Sibling](../other.md) [Local file](file:///C:/work/report.md)",
    }) } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client, onOpenFile)} />);

    const sibling = await screen.findByRole("link", { name: "Sibling" });
    const fileUrl = screen.getByRole("link", { name: "Local file" });
    expect(sibling).not.toHaveAttribute("href");
    expect(fileUrl).not.toHaveAttribute("href");
    expect(sibling).toHaveAttribute("tabindex", "0");

    const auxiliary = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    fireEvent(sibling, auxiliary);
    expect(auxiliary.defaultPrevented).toBe(true);
    expect(onOpenFile).toHaveBeenCalledWith("C:/work/other.md");

    const enter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" });
    const space = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: " " });
    fireEvent(sibling, enter);
    fireEvent(fileUrl, space);
    expect(enter.defaultPrevented).toBe(true);
    expect(space.defaultPrevented).toBe(true);
    expect(onOpenFile).toHaveBeenNthCalledWith(2, "C:/work/other.md");
    expect(onOpenFile).toHaveBeenNthCalledWith(3, "C:/work/report.md");

    fireEvent.click(fileUrl);
    expect(onOpenFile).toHaveBeenCalledWith("C:/work/report.md");

    const context = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    fireEvent(fileUrl, context);
    expect(context.defaultPrevented).toBe(true);
    expect(onOpenFile).toHaveBeenCalledTimes(4);
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it("keeps fragment-only links in-pane for primary, middle, and context semantics", async () => {
    mockOpenUrl.mockClear();
    const openWindow = vi.spyOn(window, "open").mockImplementation(() => null);
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1, resource_id: snapshot().resource_id, revision: 1,
      text: "# Heading\n\n[Jump](#heading)",
    }) } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client)} />);
    const heading = await screen.findByRole("heading", { name: "Heading" });
    heading.scrollIntoView = vi.fn();
    const link = await screen.findByRole("link", { name: "Jump" });
    expect(link).not.toHaveAttribute("href");
    expect(link).not.toHaveAttribute("target", "_blank");
    const auxiliary = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    const context = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    fireEvent(link, auxiliary);
    fireEvent(link, context);
    expect(auxiliary.defaultPrevented).toBe(true);
    expect(context.defaultPrevented).toBe(true);
    expect(mockOpenUrl).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();
  });
});
