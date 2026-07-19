import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { WorkbenchNavigationService } from "../workbench/navigationService";
import { artifactResourceKey, createArtifactSurfaceState } from "./fileResourceKey";
import { useFilesPresentationStore } from "./filesPresentationStore";

export const ARTIFACT_PRESENTED_EVENT = "artifact://presented";

export type ArtifactPresentationEventV1 = {
  schema: number;
  presentation_id: string;
  artifact_id: string;
  version_id: string;
  canonical_path: string;
  title: string;
  origin_agent_id: string;
  origin_agent_name: string;
  reused_thread: boolean;
};

type ArtifactPresentationAckV1 = {
  presentation_id: string;
  routed: boolean;
  error: string | null;
};

async function acknowledgeArtifactPresentation(ack: ArtifactPresentationAckV1): Promise<void> {
  await invoke("ack_artifact_presentation", { ack });
}

/** Routes durable agent presentations into a background Files tab before acknowledging delivery. */
export function useArtifactEvents(
  navigation: WorkbenchNavigationService,
  enabled: boolean,
): void {
  useEffect(() => {
    if (!enabled) return undefined;
    let disposed = false;
    const subscription = listen<ArtifactPresentationEventV1>(
      ARTIFACT_PRESENTED_EVENT,
      (event) => {
        if (disposed) return;
        const presentation = event.payload;
        let routed = false;
        let error: string | null = null;
        try {
          const resourceKey = artifactResourceKey(presentation.artifact_id);
          const surfaceId = navigation.open_background({
            surface_type: "files",
            resource_key: resourceKey,
            state: createArtifactSurfaceState(presentation.version_id),
          });
          const current = useFilesPresentationStore.getState().presentations[surfaceId];
          useFilesPresentationStore.getState().setPresentation(surfaceId, {
            resource_key: resourceKey,
            descriptor: current?.resource_key === resourceKey ? current.descriptor : null,
            dirty: current?.resource_key === resourceKey ? current.dirty : false,
            attention: true,
          });
          routed = true;
        } catch (cause) {
          error = cause instanceof Error ? cause.message : String(cause);
        }
        void acknowledgeArtifactPresentation({
          presentation_id: presentation.presentation_id,
          routed,
          error,
        }).catch((cause) => {
          console.error("Failed to acknowledge artifact presentation", cause);
        });
      },
    );

    void subscription.catch((cause) => {
      console.error("Failed to listen for artifact presentations", cause);
    });
    return () => {
      disposed = true;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [enabled, navigation]);
}
