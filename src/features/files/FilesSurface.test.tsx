import { lazy, type ComponentProps } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { FileContentDescriptorV1, FileResourceSnapshotV1 } from "../../types";
import { FileResourceClient } from "./fileResourceClient";
import { FilesSurface } from "./FilesSurface";
import {
  RendererRegistry,
  type FileRendererDefinition,
} from "./rendererRegistry";

const useFileResourceMock = vi.hoisted(() => vi.fn());

vi.mock("./useFileResource", () => ({
  useFileResource: useFileResourceMock,
}));

function descriptor(
  overrides: Partial<FileContentDescriptorV1> = {},
): FileContentDescriptorV1 {
  return {
    schema: 1,
    canonical_path: "C:/work/docs/report.pdf",
    display_name: "report.pdf",
    extension: "pdf",
    mime_type: "application/pdf",
    encoding: null,
    renderer_kind: "pdf",
    size_bytes: 2_048,
    line_count: null,
    content_hash: "sha256:report",
    modified_at_ms: 1,
    capabilities: { preview: true, changes: false, draft: false, stream: true },
    unavailable_reason: null,
    ...overrides,
  };
}

function snapshot(
  descriptorValue: FileContentDescriptorV1 = descriptor(),
): FileResourceSnapshotV1 {
  return {
    resource_id: "resource-1",
    subscription_id: "subscription-1",
    revision: 1,
    descriptor: descriptorValue,
  };
}

const PreviewRenderer = lazy(async () => ({
  default: ({ snapshot: value }: { snapshot: FileResourceSnapshotV1 }) => (
    <div data-testid="preview-renderer">{value.descriptor.display_name}</div>
  ),
}));

const UnsupportedPreview = lazy(() => import("./UnsupportedRenderer"));

function definition(
  renderer_id: string,
  renderComponent: FileRendererDefinition["render"] = PreviewRenderer,
): FileRendererDefinition {
  return {
    renderer_id,
    matches: ({ renderer_kind }) => renderer_kind === renderer_id,
    capabilities: {
      preview: true,
      changes: renderer_id === "pdf" ? "version" : "none",
      draft: false,
      annotations: "general",
    },
    render: renderComponent,
    create_renderer: () => renderComponent,
  };
}

function registry(renderComponent = PreviewRenderer) {
  return new RendererRegistry([
    definition("pdf", renderComponent),
    definition("unsupported", UnsupportedPreview),
  ]);
}

function props(
  overrides: Partial<ComponentProps<typeof FilesSurface>> = {},
): ComponentProps<typeof FilesSurface> {
  return {
    surface_id: "files-1",
    resource_key: "file:C:/work/docs/report.pdf",
    state: {
      resource_kind: "file",
      mode: "preview",
      transient_preview: false,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    },
    lifecycle: { visible: true },
    client: new FileResourceClient(),
    registry: registry(),
    on_open_with: vi.fn().mockResolvedValue(undefined),
    on_reveal: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

describe("FilesSurface", () => {
  beforeEach(() => {
    useFileResourceMock.mockReset();
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(),
      error: null,
      retry: vi.fn().mockResolvedValue(undefined),
    });
  });

  it("renders one compact mode bar and a remaining-space preview region", async () => {
    const view = render(<FilesSurface {...props()} />);

    expect(view.container.querySelectorAll(".files-mode-bar")).toHaveLength(1);
    expect(view.container.querySelectorAll('[role="tablist"]')).toHaveLength(1);
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Draft" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Changes" })).toHaveAccessibleDescription(
      /not available in this foundation/i,
    );
    expect(screen.getByRole("tab", { name: "Changes" })).not.toBeDisabled();
    expect(screen.getByRole("tab", { name: "Changes" })).toHaveAttribute(
      "title",
      expect.stringMatching(/not available/i),
    );
    expect(screen.getByRole("tab", { name: "Changes" })).toHaveAttribute(
      "aria-describedby",
      expect.not.stringContaining("file-C--work-docs-report-pdf"),
    );
    screen.getByRole("tab", { name: "Changes" }).focus();
    expect(screen.getByRole("tab", { name: "Changes" })).toHaveFocus();
    fireEvent.click(screen.getByRole("tab", { name: "Changes" }));
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId("files-content-region")).toHaveClass("files-content-region");
    expect(await screen.findByTestId("preview-renderer")).toHaveTextContent("report.pdf");
  });

  it("requests trusted backend restore without persisting authorization identifiers", () => {
    render(<FilesSurface {...props()} />);

    expect(useFileResourceMock).toHaveBeenCalledWith({
      path: "C:/work/docs/report.pdf",
      agent_id: null,
      user_file_capability_id: null,
    }, expect.any(FileResourceClient));
  });

  it("keeps breadcrumb identity visible while metadata and actions live in overflow", () => {
    const view = render(<FilesSurface {...props()} />);

    const breadcrumb = screen.getByRole("navigation", { name: "File location" });
    expect(breadcrumb).toHaveTextContent("C:");
    expect(breadcrumb).toHaveTextContent("report.pdf");
    expect(breadcrumb).toHaveClass("files-breadcrumb");
    const overflow = screen.getByRole("button", { name: "File actions" });
    expect(overflow).toBeInTheDocument();
    expect(view.container.querySelector(".files-mode-bar")).toHaveClass("files-mode-bar");
    fireEvent.click(overflow);
    const menu = screen.getByRole("menu", { name: "File actions" });
    expect(within(menu).getByText("application/pdf")).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Open With" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Reveal" })).toBeInTheDocument();
  });

  it("does not acquire a resource while the surface lifecycle is suspended", () => {
    render(<FilesSurface {...props({ lifecycle: { visible: false } })} />);

    expect(screen.getByRole("status")).toHaveTextContent(/preview suspended/i);
    expect(useFileResourceMock).not.toHaveBeenCalled();
  });

  it("contains resource load errors and offers applicable recovery actions", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const onOpenWith = vi.fn().mockResolvedValue(undefined);
    const onReveal = vi.fn().mockResolvedValue(undefined);
    useFileResourceMock.mockReturnValue({
      status: "error",
      snapshot: null,
      error: new Error("Access was revoked"),
      retry,
    });
    render(<FilesSurface {...props({
      on_open_with: onOpenWith,
      on_reveal: onReveal,
    })} />);

    expect(screen.getByRole("alert")).toHaveTextContent("Access was revoked");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    fireEvent.click(screen.getByRole("button", { name: "Open With" }));
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await waitFor(() => {
      expect(retry).toHaveBeenCalledOnce();
      expect(onOpenWith).toHaveBeenCalledWith("C:/work/docs/report.pdf");
      expect(onReveal).toHaveBeenCalledWith("C:/work/docs/report.pdf");
    });
  });

  it("shows unsupported metadata without attempting the renderer", async () => {
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(descriptor({
        renderer_kind: "unsupported",
        mime_type: "application/octet-stream",
        unavailable_reason: "unsupported_encoding",
      })),
      error: null,
      retry: vi.fn(),
    });

    render(<FilesSurface {...props()} />);

    expect(await screen.findByText("unsupported_encoding")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(/preview unavailable/i);
    expect(screen.getByRole("button", { name: "Open With" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reveal" })).toBeInTheDocument();
  });

  it("retries a rejected lazy renderer with a fresh loader attempt", async () => {
    let attempt = 0;
    const retryingDefinition: FileRendererDefinition = {
      ...definition("pdf"),
      create_renderer: () => lazy(async () => {
        attempt += 1;
        if (attempt === 1) throw new Error("PDF worker failed to load");
        return { default: () => <div data-testid="recovered-renderer">Recovered</div> };
      }),
    };
    const retryRegistry = new RendererRegistry([
      retryingDefinition,
      definition("unsupported", UnsupportedPreview),
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(<FilesSurface {...props({ registry: retryRegistry })} />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/renderer could not display report.pdf/i);
    expect(alert).toHaveTextContent("PDF worker failed to load");
    expect(within(alert).getByRole("button", { name: "Reset Renderer" })).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: "Open With" })).toBeInTheDocument();
    fireEvent.click(within(alert).getByRole("button", { name: "Reset Renderer" }));
    expect(await screen.findByTestId("recovered-renderer")).toHaveTextContent("Recovered");
    expect(attempt).toBe(2);
    consoleError.mockRestore();
  });
});
