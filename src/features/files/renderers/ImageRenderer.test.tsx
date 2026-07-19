import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FileResourceSnapshotV1 } from "../../../types";
import type { FileResourceClient } from "../fileResourceClient";
import ImageRenderer from "./ImageRenderer";

function snapshot(
  revision = 1,
  preview = true,
  subscriptionId = "subscription-1",
): FileResourceSnapshotV1 {
  return {
    resource_id: "file:C:/work/figure.png",
    subscription_id: subscriptionId,
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
  it("consumes the client's converted revision ticket URL and releases its renderer lease", async () => {
    const client = {
      issueTicket: vi.fn().mockImplementation(async (owner: FileResourceSnapshotV1, lease: string) => ({
        schema: 1,
        ticket_id: `ticket-${owner.revision}`,
        url: `http://wardian-resource.localhost/ticket-${owner.revision}`,
        resource_id: owner.resource_id,
        revision: owner.revision,
        renderer_lease_id: lease,
        expires_at_ms: Date.now() + 60_000,
      })),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<ImageRenderer {...props(client)} />);

    const image = await screen.findByRole("img", { name: "figure.png" });
    expect(image).toHaveAttribute("src", "http://wardian-resource.localhost/ticket-1");
    expect(client.issueTicket).toHaveBeenCalledWith(snapshot(), expect.any(String));
    Object.defineProperty(image, "naturalWidth", { value: 800 });
    Object.defineProperty(image, "naturalHeight", { value: 600 });
    fireEvent.load(image);
    fireEvent.click(screen.getByRole("button", { name: "100%" }));
    expect(image.parentElement).toHaveStyle({ width: "800px", height: "600px" });
    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(image.parentElement).toHaveStyle({ width: "1000px", height: "750px" });

    view.rerender(<ImageRenderer {...props(client, snapshot(2))} />);
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledTimes(1));
    await screen.findByRole("img", { name: "figure.png" });
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledTimes(2));
  });

  it("rolls the renderer lease when the subscription changes at the same revision", async () => {
    const issuedLeases: string[] = [];
    const client = {
      issueTicket: vi.fn().mockImplementation(async (
        owner: FileResourceSnapshotV1,
        lease: string,
      ) => {
        issuedLeases.push(lease);
        return {
          schema: 1,
          ticket_id: `${owner.subscription_id}:${lease}`,
          url: `wardian-resource://localhost/${owner.subscription_id}`,
          resource_id: owner.resource_id,
          revision: owner.revision,
          renderer_lease_id: lease,
          expires_at_ms: Date.now() + 60_000,
        };
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const firstSnapshot = snapshot(1, true, "subscription-a");
    const secondSnapshot = snapshot(1, true, "subscription-b");
    const view = render(<ImageRenderer {...props(client, firstSnapshot)} />);

    await screen.findByRole("img", { name: "figure.png" });
    view.rerender(<ImageRenderer {...props(client, secondSnapshot)} />);

    await waitFor(() => expect(client.issueTicket).toHaveBeenCalledTimes(2));
    expect(client.closeRendererLease).toHaveBeenCalledWith(firstSnapshot, issuedLeases[0]);
    expect(client.issueTicket).toHaveBeenLastCalledWith(secondSnapshot, issuedLeases[1]);
    view.unmount();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledWith(
      secondSnapshot,
      issuedLeases[1],
    ));
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

  it("accepts the registry's validated image MIME fallback but never active SVG", async () => {
    const fallback = snapshot();
    fallback.descriptor.renderer_kind = "unsupported";
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: fallback.resource_id, revision: 1, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    const view = render(<ImageRenderer {...props(client, fallback)} />);
    await screen.findByRole("img", { name: "figure.png" });
    view.unmount();
    const svg = snapshot();
    svg.descriptor.renderer_kind = "unsupported";
    svg.descriptor.mime_type = "image/svg+xml";
    render(<ImageRenderer {...props(client, svg)} />);
    expect(client.issueTicket).toHaveBeenCalledTimes(1);
  });

  it("supports keyboard panning, pointer cancellation, and reachable zoomed layout", async () => {
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: snapshot().resource_id, revision: 1, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<ImageRenderer {...props(client)} />);
    const image = await screen.findByRole("img", { name: "figure.png" });
    Object.defineProperty(image, "naturalWidth", { value: 1600 });
    Object.defineProperty(image, "naturalHeight", { value: 900 });
    fireEvent.load(image);
    fireEvent.click(screen.getByRole("button", { name: "100%" }));
    const viewport = screen.getByRole("region", { name: "Image pan viewport" });
    expect(viewport).toHaveAttribute("tabindex", "0");
    Object.defineProperty(viewport, "scrollLeft", { value: 0, writable: true });
    Object.defineProperty(viewport, "scrollTop", { value: 0, writable: true });
    fireEvent.keyDown(viewport, { key: "ArrowRight" });
    fireEvent.keyDown(viewport, { key: "ArrowDown" });
    expect(viewport.scrollLeft).toBeGreaterThan(0);
    expect(viewport.scrollTop).toBeGreaterThan(0);
    const layout = image.parentElement;
    expect(layout).toHaveStyle({ width: "1600px", height: "900px" });
    fireEvent.pointerDown(viewport, { pointerId: 1, clientX: 20, clientY: 20 });
    fireEvent.pointerCancel(viewport, { pointerId: 1 });
    const left = viewport.scrollLeft;
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 0, clientY: 0 });
    expect(viewport.scrollLeft).toBe(left);
  });

  it("marks the complete image control set as horizontally reachable in narrow panes", async () => {
    const client = {
      issueTicket: vi.fn().mockResolvedValue({
        schema: 1, ticket_id: "ticket", url: "wardian-resource://localhost/ticket",
        resource_id: snapshot().resource_id, revision: 1, renderer_lease_id: "lease",
        expires_at_ms: Date.now() + 60_000,
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<ImageRenderer {...props(client)} />);

    await screen.findByRole("img", { name: "figure.png" });
    const toolbar = screen.getByRole("toolbar", { name: "Image controls" });
    expect(toolbar).toHaveClass("files-renderer-toolbar", "files-image-toolbar");
    expect(screen.getByRole("button", { name: "Fit" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "100%" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Zoom out" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Zoom in" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "Image zoom" })).toBeInTheDocument();
  });

  it("releases decode failures immediately and retries with a fresh lease", async () => {
    const issuedLeases: string[] = [];
    const client = {
      issueTicket: vi.fn().mockImplementation(async (owner: FileResourceSnapshotV1, lease: string) => {
        issuedLeases.push(lease);
        return {
          schema: 1, ticket_id: lease, url: `wardian-resource://localhost/${lease}`,
          resource_id: owner.resource_id, revision: owner.revision, renderer_lease_id: lease,
          expires_at_ms: Date.now() + 60_000,
        };
      }),
      closeRendererLease: vi.fn().mockResolvedValue(undefined),
    } as unknown as FileResourceClient;
    render(<ImageRenderer {...props(client)} />);
    fireEvent.error(await screen.findByRole("img", { name: "figure.png" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Image preview unavailable");
    expect(screen.queryByRole("img", { name: "figure.png" })).toBeNull();
    await waitFor(() => expect(client.closeRendererLease).toHaveBeenCalledWith(
      snapshot(),
      issuedLeases[0],
    ));
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByRole("img", { name: "figure.png" });
    expect(issuedLeases).toHaveLength(2);
    expect(issuedLeases[1]).not.toBe(issuedLeases[0]);
  });
});
