import { lazy, type ComponentProps } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
    resource_id: "file:C:/work/docs/report.pdf",
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

const SourceRenderer = lazy(async () => ({
  default: () => <div data-testid="source-renderer">Source</div>,
}));

const UnsupportedPreview = lazy(() => import("./UnsupportedRenderer"));

function definition(
  renderer_id: string,
  renderComponent: FileRendererDefinition["render"] = PreviewRenderer,
  sourceComponent?: FileRendererDefinition["render"],
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
    source: sourceComponent
      ? { render: sourceComponent, create_renderer: () => sourceComponent }
      : undefined,
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
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
  });

  it("switches Markdown between rendered and source presentations", async () => {
    const user = userEvent.setup();
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(descriptor({
        display_name: "notes.md",
        extension: "md",
        mime_type: "text/markdown",
        encoding: "utf-8",
        renderer_kind: "markdown",
        line_count: 2,
        capabilities: { preview: true, changes: true, draft: true, stream: false },
      })),
      error: null,
      retry: vi.fn(),
    });
    const markdownRegistry = new RendererRegistry([
      definition("markdown", PreviewRenderer, SourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    render(<FilesSurface {...props({ registry: markdownRegistry })} />);

    const viewSource = screen.getByRole("button", { name: "View source" });
    expect(viewSource).toHaveAttribute("aria-pressed", "false");
    expect(viewSource).toHaveAttribute("title", "View source");
    expect(viewSource.querySelector("svg")).toHaveClass("lucide-book-open");
    fireEvent.click(viewSource);
    expect(await screen.findByTestId("source-renderer")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View rendered" }))
      .toHaveAttribute("aria-pressed", "true");
    const viewRendered = screen.getByRole("button", { name: "View rendered" });
    expect(viewRendered.querySelector("svg")).toHaveClass("lucide-pencil");
    viewRendered.focus();
    expect(viewRendered).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(await screen.findByTestId("preview-renderer")).toBeInTheDocument();
  });

  it("omits the presentation control for plain text without source", () => {
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(descriptor({
        display_name: "notes.txt",
        extension: "txt",
        mime_type: "text/plain",
        encoding: "utf-8",
        renderer_kind: "text",
        line_count: 2,
        capabilities: { preview: true, changes: true, draft: true, stream: false },
      })),
      error: null,
      retry: vi.fn(),
    });
    const textRegistry = new RendererRegistry([
      definition("text"),
      definition("unsupported", UnsupportedPreview),
    ]);

    render(<FilesSurface {...props({ registry: textRegistry })} />);

    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
  });

  it("preserves source while hidden and resets it when the resource changes", async () => {
    const markdownDescriptor = descriptor({
      canonical_path: "C:/work/docs/notes.md",
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "markdown",
      line_count: 2,
      capabilities: { preview: true, changes: true, draft: true, stream: false },
    });
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: {
        ...snapshot(markdownDescriptor),
        resource_id: "file:C:/work/docs/notes.md",
      },
      error: null,
      retry: vi.fn(),
    });
    const markdownRegistry = new RendererRegistry([
      definition("markdown", PreviewRenderer, SourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    const visibleProps = props({
      resource_key: "file:C:/work/docs/notes.md",
      registry: markdownRegistry,
    });
    const view = render(<FilesSurface {...visibleProps} />);

    fireEvent.click(screen.getByRole("button", { name: "View source" }));
    expect(await screen.findByTestId("source-renderer")).toBeInTheDocument();
    view.rerender(<FilesSurface {...visibleProps} lifecycle={{ visible: false }} />);
    expect(screen.getByRole("status")).toHaveTextContent(/preview suspended/i);
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
    view.rerender(<FilesSurface {...visibleProps} lifecycle={{ visible: true }} />);
    expect(await screen.findByTestId("source-renderer")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View rendered" })).toBeInTheDocument();

    const nextDescriptor = {
      ...markdownDescriptor,
      canonical_path: "C:/work/docs/next.md",
      display_name: "next.md",
      content_hash: "sha256:next",
    };
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: {
        ...snapshot(nextDescriptor),
        resource_id: "file:C:/work/docs/next.md",
      },
      error: null,
      retry: vi.fn(),
    });
    view.rerender(<FilesSurface
      {...visibleProps}
      resource_key="file:C:/work/docs/next.md"
      lifecycle={{ visible: true }}
    />);

    expect(await screen.findByTestId("preview-renderer")).toHaveTextContent("next.md");
    expect(screen.getByRole("button", { name: "View source" }))
      .toHaveAttribute("aria-pressed", "false");
  });

  it("contains source renderer errors and clears them when returning to rendered", async () => {
    const FailingSourceRenderer = lazy(async () => ({
      default: () => {
        throw new Error("Source renderer failed");
      },
    }));
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(descriptor({
        display_name: "notes.md",
        extension: "md",
        mime_type: "text/markdown",
        encoding: "utf-8",
        renderer_kind: "markdown",
        line_count: 2,
        capabilities: { preview: true, changes: true, draft: true, stream: false },
      })),
      error: null,
      retry: vi.fn(),
    });
    const markdownRegistry = new RendererRegistry([
      definition("markdown", PreviewRenderer, FailingSourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    render(<FilesSurface {...props({ registry: markdownRegistry })} />);

    fireEvent.click(screen.getByRole("button", { name: "View source" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Source renderer failed");
    fireEvent.click(screen.getByRole("button", { name: "View rendered" }));
    expect(await screen.findByTestId("preview-renderer")).toBeInTheDocument();
    expect(screen.queryByText("Source renderer failed")).toBeNull();
    consoleError.mockRestore();
  });

  it("requests trusted backend restore without persisting authorization identifiers", () => {
    render(<FilesSurface {...props()} />);

    expect(useFileResourceMock).toHaveBeenCalledWith({
      path: "C:/work/docs/report.pdf",
      agent_id: null,
      user_file_capability_id: null,
    }, expect.any(FileResourceClient));
  });

  it("preserves literal backslashes when decoding a POSIX file resource key", () => {
    render(<FilesSurface {...props({ resource_key: "file:/tmp/a\\b.md" })} />);

    expect(useFileResourceMock).toHaveBeenCalledWith({
      path: "/tmp/a\\b.md",
      agent_id: null,
      user_file_capability_id: null,
    }, expect.any(FileResourceClient));
  });

  it("reports backend canonical identity for a restored alias without rewriting it locally", async () => {
    const onCanonicalResource = vi.fn().mockResolvedValue(undefined);
    render(<FilesSurface {...props({
      resource_key: "file:C:/work/link/report.pdf",
      on_canonical_resource: onCanonicalResource,
    })} />);

    await waitFor(() => expect(onCanonicalResource).toHaveBeenCalledWith(
      "file:C:/work/docs/report.pdf",
    ));
    expect(useFileResourceMock).toHaveBeenCalledWith({
      path: "C:/work/link/report.pdf",
      agent_id: null,
      user_file_capability_id: null,
    }, expect.any(FileResourceClient));
  });

  it("acknowledges an already-canonical snapshot once so duplicate provenance is released", async () => {
    const onCanonicalResource = vi.fn().mockResolvedValue(undefined);
    const view = render(<FilesSurface {...props({
      on_canonical_resource: onCanonicalResource,
    })} />);
    await waitFor(() => expect(onCanonicalResource).toHaveBeenCalledOnce());
    expect(onCanonicalResource).toHaveBeenCalledWith("file:C:/work/docs/report.pdf");

    view.rerender(<FilesSurface {...props({
      on_canonical_resource: onCanonicalResource,
    })} />);
    expect(onCanonicalResource).toHaveBeenCalledOnce();
  });

  it("retries an interrupted canonicalization instead of permanently marking it handled", async () => {
    const onCanonicalResource = vi.fn()
      .mockResolvedValueOnce("cancel")
      .mockResolvedValueOnce("allow");
    render(<FilesSurface {...props({
      on_canonical_resource: onCanonicalResource,
    })} />);

    await waitFor(() => expect(onCanonicalResource).toHaveBeenCalledTimes(2));
    expect(onCanonicalResource).toHaveBeenNthCalledWith(1, "file:C:/work/docs/report.pdf");
    expect(onCanonicalResource).toHaveBeenNthCalledWith(2, "file:C:/work/docs/report.pdf");
    await waitFor(() => expect(screen.queryByText(/identity update was interrupted/i)).toBeNull());
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
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
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
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
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
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
  });

  it("keeps active HTML metadata-only with the live renderer reason and no text read", async () => {
    const client = new FileResourceClient();
    const readText = vi.spyOn(client, "readText");
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(descriptor({
        display_name: "demo.html",
        extension: "html",
        mime_type: "text/html",
        encoding: "utf-8",
        renderer_kind: "text",
        capabilities: { preview: true, changes: true, draft: true, stream: false },
      })),
      error: null,
      retry: vi.fn(),
    });

    render(<FilesSurface {...props({ client })} />);

    expect(await screen.findByText("live_renderer_not_activated")).toBeInTheDocument();
    expect(readText).not.toHaveBeenCalled();
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
