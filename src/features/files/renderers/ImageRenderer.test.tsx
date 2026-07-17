import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import ImageRenderer from "./ImageRenderer";

function snapshot(revision = 1, preview = true): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/figure.png",
    subscription_id: "subscription-1",
    revision,
    descriptor: {
      schema: 1,
      canonical_path: "C:/work/figure.png",
      display_name: "figure.png",
      extension: "png",
      mime_type: "image/png",
      encoding: null,
      renderer_kind: "image",
      size_bytes: preview ? 1024 : 64 * 1024 * 1024 + 1,
      line_count: null,
      content_hash: `hash-${revision}`,
      modified_at_ms: revision,
      capabilities: { preview, changes: false, draft: false, stream: preview },
      unavailable_reason: preview ? null : "image_limit_exceeded",
    },
  };
}

function props(client: FileResourceClient, nextSnapshot = snapshot()) {
  return {
    snapshot: nextSnapshot,
    client,
    lifecycle: { visible: true },
    on_open_file: vi.fn(),
    on_open_with: vi.fn(),
    on_reveal: vi.fn(),
  };
}

describe("ImageRenderer", () => {
  it("uses a revision ticket URL directly and releases its renderer lease", async () => {
    const client = {
      issueTicket: vi.fn().mockImplementation(async (_resource, revision, lease) => ({
        schema: 1,
        ticket_id: `ticket-${revision}`,
        url: `wardian-resource://localhost/ticket-${revision}`,
        resource_id: snapshot().resource_id,
        revision,
        renderer_lease_id: lease,
        expires_at_ms: Date.now() + 60_000,
      })),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<ImageRenderer {...props(client)} />);

    const image = await screen.findByRole("img", { name: "figure.png" });
    expect(image).toHaveAttribute("src", "wardian-resource://localhost/ticket-1");
    expect(client.issueTicket).toHaveBeenCalledWith(snapshot().resource_id, 1, expect.any(String));
    fireEvent.click(screen.getByRole("button", { name: "100%" }));
    expect(image).toHaveStyle({ transform: "scale(1)" });
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(image.getAttribute("style")).toContain("scale(1.25)");

    view.rerender(<ImageRenderer {...props(client, snapshot(2))} />);
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledTimes(1));
    await screen.findByRole("img", { name: "figure.png" });
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledTimes(2));
  });

  it("does not issue a ticket for an oversized, pixel-rejected, or suspended image", () => {
    const client = {
      issueTicket: vi.fn(),
      closeRendererLease: vi.fn(),
    } as unknown as FileResourceClient;
    const oversized = render(<ImageRenderer {...props(client, snapshot(1, false))} />);
    expect(screen.getByRole("status")).toHaveTextContent("image_limit_exceeded");
    expect(client.issueTicket).not.toHaveBeenCalled();
    oversized.unmount();
    render(<ImageRenderer {...props(client)} lifecycle={{ visible: false }} />);
    expect(client.issueTicket).not.toHaveBeenCalled();
  });
});
