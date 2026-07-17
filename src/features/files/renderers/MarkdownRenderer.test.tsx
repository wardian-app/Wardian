import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import MarkdownRenderer from "./MarkdownRenderer";

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
    fireEvent.click(screen.getByRole("link", { name: "Web" }));
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
});
