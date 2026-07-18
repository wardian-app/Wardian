import { lazy, type ComponentProps } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  FileContentDescriptorV1,
  FileRecoverySummaryV1,
  FileRecoveryV1,
  FileResourceSnapshotV1,
  FilesSurfaceStateV2,
} from "../../types";
import { FileResourceClient } from "./fileResourceClient";
import { FileEditorControllerRegistry } from "./fileEditorController";
import { FilesSurface } from "./FilesSurface";
import {
  filesPresentationBadges,
  useFilesPresentationStore,
} from "./filesPresentationStore";
import {
  RendererRegistry,
  type FileRendererDefinition,
} from "./rendererRegistry";

const useFileResourceMock = vi.hoisted(() => vi.fn());

vi.mock("./useFileResource", () => ({
  useFileResource: useFileResourceMock,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

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
  const client = overrides.client ?? new FileResourceClient();
  if (!vi.isMockFunction(client.readText)) {
    vi.spyOn(client, "readText").mockImplementation(async (resource) => ({
      schema: 1,
      resource_id: resource.resource_id,
      revision: resource.revision,
      text: "base\n",
    }));
  }
  if (!vi.isMockFunction(client.listRecoveries)) {
    vi.spyOn(client, "listRecoveries").mockResolvedValue([]);
  }
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
    client,
    editor_registry: overrides.editor_registry
      ?? new FileEditorControllerRegistry(client, { checkpoint_debounce_ms: 60_000 }),
    registry: registry(),
    on_open_with: vi.fn().mockResolvedValue(undefined),
    on_reveal: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

describe("FilesSurface", () => {
  beforeEach(() => {
    useFilesPresentationStore.getState().reset();
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

  it("normalizes V2 presentation intent after renderer discovery", async () => {
    const textDescriptor = descriptor({
      canonical_path: "C:/work/docs/report.txt",
      display_name: "report.txt",
      extension: "txt",
      mime_type: "text/plain",
      encoding: "utf-8",
      renderer_kind: "text",
      capabilities: { preview: true, changes: true, draft: true, stream: true },
    });
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(textDescriptor),
      error: null,
      retry: vi.fn(),
    });
    const textRegistry = new RendererRegistry([
      definition("text"),
      definition("unsupported", UnsupportedPreview),
    ]);
    const onStateChange = vi.fn();
    render(<FilesSurface {...props({
      resource_key: "file:C:/work/docs/report.txt",
      registry: textRegistry,
      state: {
        resource_kind: "file",
        transient_preview: true,
        presentation: "rendered",
        comparison_open: false,
        comparison_layout_preference: "auto",
        comparison_baseline: null,
        review_drawer_open: false,
        selected_version_id: null,
        optional_checkpoint_id: null,
      },
      on_state_change: onStateChange,
    })} />);

    await waitFor(() => expect(onStateChange).toHaveBeenCalledWith(expect.objectContaining({
      presentation: "editor",
      transient_preview: true,
    })));
  });

  it("commits the legacy renderer default once when stale props still request migration", async () => {
    const textDescriptor = descriptor({
      canonical_path: "C:/work/docs/report.txt",
      display_name: "report.txt",
      extension: "txt",
      mime_type: "text/plain",
      encoding: "utf-8",
      renderer_kind: "text",
      capabilities: { preview: true, changes: true, draft: true, stream: true },
    });
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: snapshot(textDescriptor),
      error: null,
      retry: vi.fn(),
    });
    const textRegistry = new RendererRegistry([
      definition("text", PreviewRenderer, SourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    const legacyRestoredState: FilesSurfaceStateV2 = {
      resource_kind: "file",
      transient_preview: true,
      presentation: "rendered",
      comparison_open: false,
      comparison_layout_preference: "auto",
      comparison_baseline: null,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    };
    const onStateChange = vi.fn();
    const staleProps = props({
      resource_key: "file:C:/work/docs/report.txt",
      registry: textRegistry,
      state: legacyRestoredState,
      legacy_presentation_intent: "renderer_default",
      on_state_change: onStateChange,
    });
    const view = render(<FilesSurface {...staleProps} />);

    await waitFor(() => expect(onStateChange).toHaveBeenCalledOnce());
    expect(onStateChange).toHaveBeenCalledWith({
      ...legacyRestoredState,
      presentation: "editor",
    });

    view.rerender(<FilesSurface {...staleProps} state={{ ...legacyRestoredState }} />);
    await waitFor(() => expect(onStateChange).toHaveBeenCalledOnce());
  });

  it("preserves a historical comparison while baseline availability is unknown", async () => {
    const onStateChange = vi.fn();
    render(<FilesSurface {...props({
      state: {
        resource_kind: "file",
        transient_preview: true,
        presentation: "rendered",
        comparison_open: true,
        comparison_layout_preference: "auto",
        comparison_baseline: {
          kind: "prompt_checkpoint",
          checkpoint_id: "checkpoint-1",
        },
        review_drawer_open: false,
        selected_version_id: null,
        optional_checkpoint_id: "checkpoint-1",
      },
      on_state_change: onStateChange,
    })} />);

    expect(await screen.findByTestId("preview-renderer")).toBeInTheDocument();
    expect(onStateChange).not.toHaveBeenCalled();
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

  it("retains the native resource and editor session while the renderer is suspended", async () => {
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
    const client = new FileResourceClient();
    const editorRegistry = new FileEditorControllerRegistry(client);
    const view = render(<FilesSurface {...props({
      resource_key: "file:C:/work/docs/notes.md",
      lifecycle: { visible: false },
      client,
      editor_registry: editorRegistry,
      registry: new RendererRegistry([
        definition("markdown", PreviewRenderer, SourceRenderer),
        definition("unsupported", UnsupportedPreview),
      ]),
    })} />);

    expect(screen.getByRole("status")).toHaveTextContent(/preview suspended/i);
    expect(useFileResourceMock).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
    await waitFor(() => expect(
      editorRegistry.getExisting("file:C:/work/docs/notes.md")?.getSnapshot().status,
    ).toBe("ready"));

    view.rerender(<FilesSurface {...props({
      resource_key: "file:C:/work/docs/notes.md",
      lifecycle: { visible: true },
      client,
      editor_registry: editorRegistry,
      registry: new RendererRegistry([
        definition("markdown", PreviewRenderer, SourceRenderer),
        definition("unsupported", UnsupportedPreview),
      ]),
    })} />);
    expect(editorRegistry.getExisting("file:C:/work/docs/notes.md")?.getSnapshot())
      .toMatchObject({ status: "ready", presentation_ids: ["files-1"] });
  });

  it("shares one hydrated controller, dirty badges, and first-mutation pinning across panes", async () => {
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
    const client = new FileResourceClient();
    const readText = vi.spyOn(client, "readText").mockResolvedValue({
      schema: 1,
      resource_id: "file:C:/work/docs/notes.md",
      revision: 1,
      text: "base\n",
    });
    vi.spyOn(client, "listRecoveries").mockResolvedValue([]);
    const editorRegistry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const markdownRegistry = new RendererRegistry([
      definition("markdown", PreviewRenderer, SourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    const state: FilesSurfaceStateV2 = {
      resource_kind: "file",
      transient_preview: true,
      presentation: "rendered",
      comparison_open: false,
      comparison_layout_preference: "auto",
      comparison_baseline: null,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    };
    const firstStateChange = vi.fn();
    const secondStateChange = vi.fn();
    render(<>
      <FilesSurface {...props({
        surface_id: "files-a",
        resource_key: "file:C:/work/docs/notes.md",
        state,
        client,
        editor_registry: editorRegistry,
        registry: markdownRegistry,
        on_state_change: firstStateChange,
      })} />
      <FilesSurface {...props({
        surface_id: "files-b",
        resource_key: "file:C:/work/docs/notes.md",
        state,
        client,
        editor_registry: editorRegistry,
        registry: markdownRegistry,
        on_state_change: secondStateChange,
      })} />
    </>);

    const controller = editorRegistry.forResource("file:C:/work/docs/notes.md");
    await waitFor(() => expect(controller.getSnapshot()).toMatchObject({
      status: "ready",
      presentation_ids: ["files-a", "files-b"],
    }));
    expect(readText).toHaveBeenCalledOnce();

    act(() => { controller.mutate("shared edit\n"); });
    await waitFor(() => {
      expect(filesPresentationBadges("files-a", "file:C:/work/docs/notes.md")).toEqual([
        { badge_id: "dirty", label: "Unsaved changes" },
      ]);
      expect(filesPresentationBadges("files-b", "file:C:/work/docs/notes.md")).toEqual([
        { badge_id: "dirty", label: "Unsaved changes" },
      ]);
    });
    expect(firstStateChange).toHaveBeenCalledWith({ ...state, transient_preview: false });
    expect(secondStateChange).toHaveBeenCalledWith({ ...state, transient_preview: false });

    act(() => {
      controller.applyAuthoritative({
        ...snapshot(markdownDescriptor),
        resource_id: "file:C:/work/docs/notes.md",
        revision: 2,
        descriptor: { ...markdownDescriptor, content_hash: "hash-external" },
      }, "external\n");
    });
    await waitFor(() => expect(
      filesPresentationBadges("files-a", "file:C:/work/docs/notes.md"),
    ).toEqual([
      { badge_id: "dirty", label: "Unsaved changes" },
      { badge_id: "attention", label: "Attention requested" },
    ]));
  });

  it("opens comparison from a stale Save using the latest surface state callback", async () => {
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
    const client = new FileResourceClient();
    vi.spyOn(client, "readText").mockResolvedValue({
      schema: 1,
      resource_id: "file:C:/work/docs/notes.md",
      revision: 1,
      text: "base\n",
    });
    vi.spyOn(client, "listRecoveries").mockResolvedValue([]);
    vi.spyOn(client, "saveText").mockResolvedValue({
      status: "stale_conflict",
      revision: 2,
      content_hash: "hash-external",
    });
    const editorRegistry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    const markdownRegistry = new RendererRegistry([
      definition("markdown", PreviewRenderer, SourceRenderer),
      definition("unsupported", UnsupportedPreview),
    ]);
    const initialState: FilesSurfaceStateV2 = {
      resource_kind: "file",
      transient_preview: false,
      presentation: "editor",
      comparison_open: false,
      comparison_layout_preference: "auto",
      comparison_baseline: null,
      review_drawer_open: false,
      selected_version_id: null,
      optional_checkpoint_id: null,
    };
    const initialStateChange = vi.fn();
    const latestStateChange = vi.fn();
    const sharedProps = props({
      resource_key: "file:C:/work/docs/notes.md",
      state: initialState,
      client,
      editor_registry: editorRegistry,
      registry: markdownRegistry,
      on_state_change: initialStateChange,
    });
    const view = render(<FilesSurface {...sharedProps} />);
    const controller = editorRegistry.forResource("file:C:/work/docs/notes.md");
    await waitFor(() => expect(controller.getSnapshot().status).toBe("ready"));

    const latestState = { ...initialState, review_drawer_open: true };
    view.rerender(<FilesSurface
      {...sharedProps}
      state={latestState}
      on_state_change={latestStateChange}
    />);
    act(() => { controller.mutate("local edit\n"); });
    await act(async () => { await controller.save("files-1"); });

    expect(latestStateChange).toHaveBeenCalledWith({
      ...latestState,
      comparison_open: true,
    });
    expect(initialStateChange).not.toHaveBeenCalledWith(expect.objectContaining({
      comparison_open: true,
    }));
  });

  it("detaches and releases a clean controller only after React unmount", async () => {
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
    const client = new FileResourceClient();
    const editorRegistry = new FileEditorControllerRegistry(client);
    const view = render(<FilesSurface {...props({
      resource_key: "file:C:/work/docs/notes.md",
      client,
      editor_registry: editorRegistry,
      registry: new RendererRegistry([
        definition("markdown", PreviewRenderer, SourceRenderer),
        definition("unsupported", UnsupportedPreview),
      ]),
    })} />);
    await waitFor(() => expect(
      editorRegistry.getExisting("file:C:/work/docs/notes.md")?.getSnapshot().status,
    ).toBe("ready"));

    expect(editorRegistry.getExisting("file:C:/work/docs/notes.md")).toBeDefined();
    view.unmount();
    expect(editorRegistry.getExisting("file:C:/work/docs/notes.md")).toBeUndefined();
  });

  it("surfaces editor synchronization failures as attention and retries in place", async () => {
    const markdownDescriptor = descriptor({
      canonical_path: "C:/work/docs/notes.md",
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "markdown",
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
    const client = new FileResourceClient();
    const readText = vi.spyOn(client, "readText")
      .mockRejectedValueOnce(new Error("read denied"))
      .mockResolvedValueOnce({
        schema: 1,
        resource_id: "file:C:/work/docs/notes.md",
        revision: 1,
        text: "base\n",
      });
    vi.spyOn(client, "listRecoveries").mockResolvedValue([]);
    const editorRegistry = new FileEditorControllerRegistry(client);
    render(<FilesSurface {...props({
      resource_key: "file:C:/work/docs/notes.md",
      client,
      editor_registry: editorRegistry,
      registry: new RendererRegistry([
        definition("markdown", PreviewRenderer, SourceRenderer),
        definition("unsupported", UnsupportedPreview),
      ]),
    })} />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/file editor initialization failed: read denied/i);
    expect(filesPresentationBadges("files-1", "file:C:/work/docs/notes.md")).toContainEqual({
      badge_id: "attention",
      label: "Attention requested",
    });
    fireEvent.click(within(alert).getByRole("button", { name: "Retry Editor" }));

    await waitFor(() => expect(readText).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Retry Editor" })).toBeNull());
    expect(editorRegistry.getExisting("file:C:/work/docs/notes.md")?.getSnapshot().status)
      .toBe("ready");
  });

  it("keeps Retry Editor visible when a first mutation races recovery discovery", async () => {
    const markdownDescriptor = descriptor({
      canonical_path: "C:/work/docs/notes.md",
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "markdown",
      capabilities: { preview: true, changes: true, draft: true, stream: false },
    });
    const markdownSnapshot = {
      ...snapshot(markdownDescriptor),
      resource_id: "file:C:/work/docs/notes.md",
    };
    useFileResourceMock.mockReturnValue({
      status: "ready",
      snapshot: markdownSnapshot,
      error: null,
      retry: vi.fn(),
    });
    const pendingDiscovery = deferred<FileRecoverySummaryV1[]>();
    const recoverySummaryValue: FileRecoverySummaryV1 = {
      schema: 1,
      recovery_id: "recovery-raced",
      resource_key: markdownSnapshot.resource_id,
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      base_content_hash: markdownDescriptor.content_hash,
      base_opaque_revision: "opaque-base",
      recovery_revision: 1,
      created_at_ms: 10,
      updated_at_ms: 20,
    };
    const client = new FileResourceClient();
    vi.spyOn(client, "readText").mockResolvedValue({
      schema: 1,
      resource_id: markdownSnapshot.resource_id,
      revision: markdownSnapshot.revision,
      text: "base\n",
    });
    const listRecoveries = vi.spyOn(client, "listRecoveries")
      .mockReturnValueOnce(pendingDiscovery.promise)
      .mockResolvedValueOnce([recoverySummaryValue]);
    vi.spyOn(client, "getRecovery").mockResolvedValue({
      ...recoverySummaryValue,
      base: "base\n",
      buffer: "older recovered edit\n",
    });
    const editorRegistry = new FileEditorControllerRegistry(client, {
      checkpoint_debounce_ms: 60_000,
    });
    render(<FilesSurface {...props({
      resource_key: markdownSnapshot.resource_id,
      client,
      editor_registry: editorRegistry,
      registry: new RendererRegistry([
        definition("markdown", PreviewRenderer, SourceRenderer),
        definition("unsupported", UnsupportedPreview),
      ]),
    })} />);
    const controller = editorRegistry.forResource(markdownSnapshot.resource_id);
    await waitFor(() => expect(controller.getSnapshot().status).toBe("ready"));

    act(() => { controller.mutate("newer in-memory edit\n"); });
    pendingDiscovery.resolve([recoverySummaryValue]);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/recovery discovery was interrupted/i);
    expect(controller.getSnapshot().working_text).toBe("newer in-memory edit\n");
    fireEvent.click(within(alert).getByRole("button", { name: "Retry Editor" }));
    await waitFor(() => expect(listRecoveries).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(alert).toHaveTextContent(/newer edits were preserved/i));
    expect(controller.getSnapshot().working_text).toBe("newer in-memory edit\n");
    expect(filesPresentationBadges("files-1", markdownSnapshot.resource_id)).toContainEqual({
      badge_id: "attention",
      label: "Attention requested",
    });
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

  it("opens scoped recovery read-only when live file authorization is unavailable", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    useFileResourceMock.mockReturnValue({
      status: "error",
      snapshot: null,
      error: new Error("Access was revoked"),
      retry,
    });
    const resourceKey = "file:C:/work/docs/recovered.md";
    const recoverySummaryValue: FileRecoverySummaryV1 = {
      schema: 1,
      recovery_id: "recovery-read-only",
      resource_key: resourceKey,
      display_name: "recovered.md",
      extension: "md",
      mime_type: "text/markdown",
      base_content_hash: "hash-recovery-base",
      base_opaque_revision: "opaque-recovery-base",
      recovery_revision: 7,
      created_at_ms: 10,
      updated_at_ms: 20,
    };
    const recovered: FileRecoveryV1 = {
      ...recoverySummaryValue,
      base: "saved recovery base\n",
      buffer: "unsaved recovered buffer\n",
    };
    const client = new FileResourceClient();
    const listRecoveries = vi.spyOn(client, "listRecoveries")
      .mockResolvedValue([recoverySummaryValue]);
    const getRecovery = vi.spyOn(client, "getRecovery").mockResolvedValue(recovered);
    const discardRecovery = vi.spyOn(client, "discardRecovery").mockResolvedValue(undefined);
    const readText = vi.spyOn(client, "readText");
    const saveText = vi.spyOn(client, "saveText");
    render(<FilesSurface {...props({
      resource_key: resourceKey,
      client,
      editor_registry: new FileEditorControllerRegistry(client),
    })} />);

    expect(await screen.findByRole("heading", { name: "Recovered unsaved changes" }))
      .toBeInTheDocument();
    expect(screen.getByLabelText("Recovered buffer")).toHaveTextContent(
      "unsaved recovered buffer",
    );
    expect(screen.getByLabelText("Recovered saved base")).toHaveTextContent(
      "saved recovery base",
    );
    expect(listRecoveries).toHaveBeenCalledWith({ resource_key: resourceKey });
    expect(getRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-read-only",
      resource_key: resourceKey,
    });
    expect(readText).not.toHaveBeenCalled();
    expect(saveText).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "File actions" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Restore access" }));
    await waitFor(() => expect(retry).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "Discard recovery" }));
    await waitFor(() => expect(discardRecovery).toHaveBeenCalledWith({
      recovery_id: "recovery-read-only",
      expected_recovery_revision: 7,
      resource_key: resourceKey,
    }));
    await waitFor(() => expect(
      screen.queryByRole("heading", { name: "Recovered unsaved changes" }),
    ).toBeNull());
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
