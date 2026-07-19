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

  it("renders safe raw HTML while rejecting active content and unsafe schemes", async () => {
    const onOpenFile = vi.fn();
    const client = {
      readText: vi.fn().mockResolvedValue({
        schema: 1,
        resource_id: snapshot().resource_id,
        revision: 1,
        text: [
          "<script>window.__unsafe = true</script>",
          "<details open><summary>More context</summary><p>Safe details</p></details>",
          "[Sibling](../other.md)",
          "[Web](https://example.test/docs)",
          "[Unsafe](javascript:alert(1))",
        ].join("\n\n"),
      }),
    } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client, onOpenFile)} />);

    const local = await screen.findByRole("link", { name: "Sibling" });
    expect(document.querySelector("script")).toBeNull();
    expect(screen.getByText("More context").closest("details")).toHaveAttribute("open");
    expect(screen.getByText("Safe details")).toBeInTheDocument();
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

  it("renders core Markdown and GFM document structure", async () => {
    const client = { readText: vi.fn().mockResolvedValue({
      schema: 1,
      resource_id: snapshot().resource_id,
      revision: 1,
      text: [
        "# Document title",
        "",
        "> A quoted note",
        "",
        "- [x] Complete",
        "- [ ] Pending",
        "",
        "| Name | State |",
        "| --- | --- |",
        "| Wardian | Active |",
        "",
        "```ts",
        "const ready = true;",
        "```",
      ].join("\n"),
    }) } as unknown as FileResourceClient;
    render(<MarkdownRenderer {...props(client)} />);

    expect(await screen.findByRole("heading", { level: 1, name: "Document title" }))
      .toBeInTheDocument();
    expect(screen.getByText("A quoted note").closest("blockquote")).not.toBeNull();
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    expect(screen.getByRole("table")).toHaveTextContent("Wardian");
    expect(screen.getByText("const ready = true;").closest("pre")).not.toBeNull();
  });

  it("loads relative images through an authorized file resource ticket", async () => {
    const imageSnapshot = {
      ...snapshot(2),
      resource_id: "file:C:/work/docs/public/icon.png",
      subscription_id: "image-subscription",
      descriptor: {
        ...snapshot(2).descriptor,
        canonical_path: "C:/work/docs/public/icon.png",
        display_name: "icon.png",
        extension: "png",
        mime_type: "image/png",
        encoding: null,
        renderer_kind: "image" as const,
        line_count: null,
        capabilities: { preview: true, changes: true, draft: false, stream: false },
      },
    };
    const client = {
      readText: vi.fn()
        .mockResolvedValueOnce({
          schema: 1,
          resource_id: snapshot().resource_id,
          revision: 1,
          text: "![Wardian icon](public/icon.png)",
        })
        .mockResolvedValueOnce({
          schema: 1,
          resource_id: snapshot(2).resource_id,
          revision: 2,
          text: "![Wardian icon](public/icon.png)",
        }),
      listenForRevisions: vi.fn().mockResolvedValue(vi.fn()),
      open: vi.fn().mockResolvedValue(imageSnapshot),
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1,
        ticket_id: "ticket-1",
        url: "wardian-file://ticket-1",
        resource_id: imageSnapshot.resource_id,
        revision: imageSnapshot.revision,
        renderer_lease_id: "lease-1",
        expires_at_ms: Date.now() + 30_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
      close: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const resourceRequest = {
      path: snapshot().descriptor.canonical_path,
      agent_id: "agent-1",
      user_file_capability_id: null,
    };
    const view = render(<MarkdownRenderer {...props(client)} resource_request={resourceRequest} />);

    const image = await screen.findByRole("img", { name: "Wardian icon" });
    expect(image).toHaveAttribute("src", "wardian-file://ticket-1");
    expect(client.open).toHaveBeenCalledWith({
      path: "C:/work/docs/public/icon.png",
      agent_id: "agent-1",
      user_file_capability_id: null,
    });
    expect(client.issueTicket).toHaveBeenCalledWith(
      imageSnapshot,
      expect.stringMatching(/^markdown-image:/),
    );
    view.rerender(
      <MarkdownRenderer
        {...props(client, vi.fn(), 2)}
        resource_request={{ ...resourceRequest }}
      />,
    );
    await waitFor(() => expect(client.readText).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("img", { name: "Wardian icon" })).toBe(image);
    expect(client.open).toHaveBeenCalledTimes(1);
    expect(client.issueTicket).toHaveBeenCalledTimes(1);
    expect(client.closeRendererLease).not.toHaveBeenCalled();
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalled());
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

  it("retains the previous rendered document while a newer revision read is pending", async () => {
    let resolveSecond: ((value: object) => void) | undefined;
    const second = new Promise<object>((resolve) => { resolveSecond = resolve; });
    const client = {
      readText: vi.fn()
        .mockResolvedValueOnce({
          schema: 1,
          resource_id: snapshot().resource_id,
          revision: 1,
          text: "# Stable title",
        })
        .mockReturnValueOnce(second),
    } as unknown as FileResourceClient;
    const view = render(<MarkdownRenderer {...props(client)} />);
    const stableHeading = await screen.findByRole("heading", { name: "Stable title" });
    stableHeading.setAttribute("data-render-identity", "preserved");

    view.rerender(<MarkdownRenderer {...props(client, vi.fn(), 2)} />);
    expect(screen.getByRole("heading", { name: "Stable title" }))
      .toHaveAttribute("data-render-identity", "preserved");
    expect(screen.queryByText("Loading Markdown…")).toBeNull();

    resolveSecond?.({
      schema: 1,
      resource_id: snapshot(2).resource_id,
      revision: 2,
      text: "# Updated title",
    });
    expect(await screen.findByRole("heading", { name: "Updated title" }))
      .toHaveAttribute("data-render-identity", "preserved");
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
    expect(resolveLocalMarkdownTarget("//?/C:/work/readme.md", "/docs/a.md")).toBe("C:/docs/a.md");
    expect(resolveLocalMarkdownTarget("//server/share/work/readme.md", "/docs/a.md")).toBe("//server/share/docs/a.md");
    expect(resolveLocalMarkdownTarget("//?/UNC/server/share/work/readme.md", "/docs/a.md"))
      .toBe("//server/share/docs/a.md");
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
