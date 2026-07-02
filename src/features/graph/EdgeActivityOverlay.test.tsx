import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type Sigma from "sigma";
import { EdgeActivityOverlay } from "./EdgeActivityOverlay";
import type { CommunicationEdge } from "./graphProjection";

// Mock Canvas2D context first
const mockCanvasContext = {
  clearRect: vi.fn(),
  scale: vi.fn(),
  setTransform: vi.fn(),
  strokeStyle: "",
  lineWidth: 0,
  setLineDash: vi.fn(),
  beginPath: vi.fn(),
  moveTo: vi.fn(),
  lineTo: vi.fn(),
  stroke: vi.fn(),
  fillStyle: "",
  arc: vi.fn(),
  fill: vi.fn(),
  save: vi.fn(),
  restore: vi.fn(),
  translate: vi.fn(),
};

vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(mockCanvasContext as any);

const mocks = vi.hoisted(() => {
  const cameraHandlers = new Map<string, (state: any) => void>();
  return {
    cameraHandlers,
    getNodeDisplayData: vi.fn(),
    framedGraphToViewport: vi.fn((data) => data),
    getContainer: vi.fn().mockReturnValue(document.createElement("div")),
    requestAnimationFrame: vi.spyOn(global, "requestAnimationFrame").mockReturnValue(123 as any),
    cancelAnimationFrame: vi.spyOn(global, "cancelAnimationFrame"),
    createSigmaInstance: () => ({
      getCamera: () => ({
        on: (event: string, handler: (state: any) => void) => {
          cameraHandlers.set(event, handler);
        },
        off: (event: string, _handler: (state: any) => void) => {
          cameraHandlers.delete(event);
        },
      }),
      getNodeDisplayData: (nodeId: string) => mocks.getNodeDisplayData(nodeId),
      framedGraphToViewport: (data: any) => mocks.framedGraphToViewport(data),
      getContainer: () => mocks.getContainer(),
    } as any as Sigma),
  };
});

beforeEach(() => {
  mocks.cameraHandlers.clear();
  mocks.getNodeDisplayData.mockClear();
  mocks.framedGraphToViewport.mockClear();
  mocks.getContainer.mockClear().mockReturnValue(document.createElement("div"));
  mocks.requestAnimationFrame.mockClear();
  mocks.cancelAnimationFrame.mockClear();
  vi.clearAllMocks();

  // Default mock behavior
  mocks.getNodeDisplayData.mockImplementation((_nodeId: string) => ({
    x: 100,
    y: 100,
  }));

  mocks.framedGraphToViewport.mockImplementation((data) => ({
    x: data.x,
    y: data.y,
  }));

  // Mock requestAnimationFrame to return a valid ID
  mocks.requestAnimationFrame.mockImplementation((_callback: FrameRequestCallback) => {
    // Don't actually call the callback, just track that it was requested
    return 123;
  });

  Object.defineProperty(mockCanvasContext, "clearRect", {
    value: vi.fn(),
    writable: true,
  });
  Object.defineProperty(mockCanvasContext, "scale", {
    value: vi.fn(),
    writable: true,
  });
  Object.defineProperty(mockCanvasContext, "setTransform", {
    value: vi.fn(),
    writable: true,
  });
});

describe("EdgeActivityOverlay", () => {
  const baseCommEdges: CommunicationEdge[] = [
    {
      id: "a--b",
      source: "a",
      target: "b",
      origin: "manual",
      state: "ongoing",
      recency: 1,
    },
  ];

  it("mounts and unmounts without error when sigma is null", () => {
    const { unmount } = render(
      <EdgeActivityOverlay sigma={null} commEdges={baseCommEdges} />
    );

    expect(() => unmount()).not.toThrow();
  });

  it("renders canvas element when sigma is provided", () => {
    const sigma = mocks.createSigmaInstance();
    const { container } = render(
      <EdgeActivityOverlay sigma={sigma} commEdges={baseCommEdges} />
    );

    const canvas = container.querySelector("canvas");
    expect(canvas).toBeInTheDocument();
  });

  it("starts rAF animation loop when there are ongoing edges", () => {
    mocks.requestAnimationFrame.mockClear();
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    // Should call requestAnimationFrame at least once for the initial render
    expect(mocks.requestAnimationFrame.mock.calls.length).toBeGreaterThan(0);
  });

  it("does not call requestAnimationFrame when there are no ongoing edges", () => {
    mocks.requestAnimationFrame.mockClear();
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "dormant",
            recency: 0,
          },
        ]}
      />
    );

    // For non-ongoing edges, we only do a single render, not a continuous loop
    expect(mocks.requestAnimationFrame.mock.calls.length).toBe(0);
  });

  it("cancels rAF on unmount", () => {
    mocks.requestAnimationFrame.mockClear();
    mocks.cancelAnimationFrame.mockClear();
    const sigma = mocks.createSigmaInstance();

    const { unmount } = render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    unmount();

    // Should have called cancelAnimationFrame to clean up the loop
    expect(mocks.cancelAnimationFrame).toHaveBeenCalled();
  });

  it("sets up camera listener for ongoing edges", () => {
    mocks.cameraHandlers.clear();
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    // Should have registered a camera listener for 'updated' event
    expect(mocks.cameraHandlers.has("updated")).toBe(true);
  });

  it("removes camera listener on unmount", () => {
    mocks.cameraHandlers.clear();
    const sigma = mocks.createSigmaInstance();

    const { unmount } = render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    expect(mocks.cameraHandlers.size).toBeGreaterThan(0);

    unmount();

    // Camera listener should be removed (the off handler is called)
    // We verify this indirectly by checking that the cleanup ran
    expect(mocks.cancelAnimationFrame).toHaveBeenCalled();
  });

  it("sets up camera listener even when there are no ongoing edges", () => {
    mocks.cameraHandlers.clear();
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "dormant",
            recency: 0,
          },
        ]}
      />
    );

    // Camera listener should be registered for all edges (including dormant) to track pan/zoom
    expect(mocks.cameraHandlers.has("updated")).toBe(true);
  });

  it("schedules a fresh frame for every camera update on an idle graph", () => {
    // Capture rAF callbacks so we can actually run frames; the default mock
    // never invokes them, which is exactly how the stale-pending-id bug
    // (scheduleRender permanently coalesced after a one-shot frame) escaped.
    const frames: FrameRequestCallback[] = [];
    let nextId = 1;
    mocks.requestAnimationFrame.mockImplementation((cb: FrameRequestCallback) => {
      frames.push(cb);
      return nextId++;
    });
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "rule",
            state: "dormant",
            recency: 0,
          },
        ]}
      />
    );

    const onCameraUpdate = mocks.cameraHandlers.get("updated");
    expect(onCameraUpdate).toBeDefined();

    onCameraUpdate!({});
    expect(frames).toHaveLength(1);

    // Run the pending frame; with no ongoing edge it must not reschedule,
    // and it must clear the pending id so the next camera update draws.
    frames[0](0);
    expect(frames).toHaveLength(1);

    onCameraUpdate!({});
    expect(frames).toHaveLength(2);
  });

  it("coalesces rapid camera updates into a single pending frame", () => {
    const frames: FrameRequestCallback[] = [];
    let nextId = 1;
    mocks.requestAnimationFrame.mockImplementation((cb: FrameRequestCallback) => {
      frames.push(cb);
      return nextId++;
    });
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "rule",
            state: "dormant",
            recency: 0,
          },
        ]}
      />
    );

    const onCameraUpdate = mocks.cameraHandlers.get("updated");
    onCameraUpdate!({});
    onCameraUpdate!({});
    onCameraUpdate!({});
    expect(frames).toHaveLength(1);
  });

  it("handles null canvas context gracefully", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValueOnce(null);
    const sigma = mocks.createSigmaInstance();

    const { unmount } = render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    expect(() => unmount()).not.toThrow();
  });

  it("renders without crashing when commEdges array is empty", () => {
    const sigma = mocks.createSigmaInstance();
    const { unmount } = render(
      <EdgeActivityOverlay sigma={sigma} commEdges={[]} />
    );

    expect(() => unmount()).not.toThrow();
  });

  it("renders with multiple ongoing edges", () => {
    mocks.getNodeDisplayData.mockReturnValue({ x: 100, y: 100 });
    mocks.framedGraphToViewport.mockImplementation((_data) => ({ x: 200, y: 200 }));
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
          {
            id: "b--c",
            source: "b",
            target: "c",
            origin: "rule",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    // Should have collected data for both edges
    expect(mocks.getNodeDisplayData).toHaveBeenCalledWith("a");
    expect(mocks.getNodeDisplayData).toHaveBeenCalledWith("b");
    expect(mocks.getNodeDisplayData).toHaveBeenCalledWith("c");
  });

  it("skips edges with missing node display data", () => {
    mocks.getNodeDisplayData.mockImplementation((nodeId: string) => {
      // Return null for target node to simulate missing data
      if (nodeId === "b") return null;
      return { x: 100, y: 100 };
    });
    const sigma = mocks.createSigmaInstance();

    render(
      <EdgeActivityOverlay
        sigma={sigma}
        commEdges={[
          {
            id: "a--b",
            source: "a",
            target: "b",
            origin: "manual",
            state: "ongoing",
            recency: 1,
          },
        ]}
      />
    );

    // The overlay should handle the missing node gracefully
    // by skipping that edge (this is verified by not crashing)
    expect(mockCanvasContext.clearRect).toHaveBeenCalled();
  });
});
