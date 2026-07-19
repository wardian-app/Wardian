import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Event, EventCallback } from "@tauri-apps/api/event";

import type { WorkbenchNavigationService } from "../workbench/navigationService";
import { useFilesPresentationStore } from "./filesPresentationStore";
import {
  ARTIFACT_PRESENTED_EVENT,
  type ArtifactPresentationEventV1,
  useArtifactEvents,
} from "./useArtifactEvents";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
let listener: EventCallback<ArtifactPresentationEventV1> | undefined;

const presentation: ArtifactPresentationEventV1 = {
  schema: 1,
  presentation_id: "presentation-1",
  artifact_id: "artifact-1",
  version_id: "version-2",
  canonical_path: "C:/workspace/report.md",
  title: "Report",
  origin_agent_id: "agent-1",
  origin_agent_name: "Writer",
  reused_thread: true,
};

function navigationWith(openBackground: WorkbenchNavigationService["open_background"]) {
  return { open_background: openBackground } as WorkbenchNavigationService;
}

function emitPresentation(): void {
  listener?.({
    event: ARTIFACT_PRESENTED_EVENT,
    id: 1,
    payload: presentation,
  } as Event<ArtifactPresentationEventV1>);
}

beforeEach(() => {
  listener = undefined;
  mockInvoke.mockReset().mockResolvedValue(undefined);
  mockListen.mockReset().mockImplementation((_event, callback) => {
    listener = callback as EventCallback<ArtifactPresentationEventV1>;
    return Promise.resolve(vi.fn());
  });
  useFilesPresentationStore.getState().reset();
});

describe("useArtifactEvents", () => {
  it("routes a presentation in the background, marks attention, then acknowledges", async () => {
    const openBackground = vi.fn(() => "surface-artifact");
    renderHook(() => useArtifactEvents(navigationWith(openBackground), true));
    await waitFor(() => expect(listener).toBeDefined());

    act(() => emitPresentation());

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "ack_artifact_presentation",
      {
        ack: {
          presentation_id: "presentation-1",
          routed: true,
          error: null,
        },
      },
    ));
    expect(openBackground).toHaveBeenCalledWith({
      surface_type: "files",
      resource_key: "artifact:artifact-1",
      state: expect.objectContaining({
        resource_kind: "artifact",
        selected_version_id: "version-2",
        transient_preview: false,
      }),
    });
    expect(useFilesPresentationStore.getState().presentations["surface-artifact"])
      .toMatchObject({ resource_key: "artifact:artifact-1", attention: true });
  });

  it("returns the routing failure instead of acknowledging false success", async () => {
    const openBackground = vi.fn(() => {
      throw new Error("workbench command rejected");
    });
    renderHook(() => useArtifactEvents(navigationWith(openBackground), true));
    await waitFor(() => expect(listener).toBeDefined());

    act(() => emitPresentation());

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith(
      "ack_artifact_presentation",
      {
        ack: {
          presentation_id: "presentation-1",
          routed: false,
          error: "workbench command rejected",
        },
      },
    ));
    expect(useFilesPresentationStore.getState().presentations).toEqual({});
  });
});
