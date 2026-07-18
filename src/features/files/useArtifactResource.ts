import { useCallback, useEffect, useMemo, useState } from "react";

import type { ArtifactResourceV1 } from "../../types";
import { type FileResourceClient, fileResourceClient } from "./fileResourceClient";

type ArtifactResourceState =
  | { status: "loading"; resource: null; error: null }
  | { status: "ready"; resource: ArtifactResourceV1; error: null }
  | { status: "error"; resource: null; error: Error };

export type UseArtifactResourceResult = ArtifactResourceState & {
  retry: () => void;
  clearAttention: () => Promise<void>;
};

function asError(cause: unknown): Error {
  if (cause instanceof Error) return cause;
  if (typeof cause === "object" && cause !== null && "message" in cause) {
    return new Error(String(cause.message));
  }
  return new Error(String(cause));
}

/** Loads one selected artifact version without conflating its opaque ID with a file path. */
export function useArtifactResource(
  artifactId: string,
  selectedVersionId: string | null,
  client: FileResourceClient = fileResourceClient,
): UseArtifactResourceResult {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<ArtifactResourceState>({
    status: "loading",
    resource: null,
    error: null,
  });

  useEffect(() => {
    let active = true;
    setState({ status: "loading", resource: null, error: null });
    void client.getArtifactResource(artifactId, selectedVersionId).then((resource) => {
      if (active) setState({ status: "ready", resource, error: null });
    }).catch((cause) => {
      if (active) setState({ status: "error", resource: null, error: asError(cause) });
    });
    return () => { active = false; };
  }, [artifactId, client, generation, selectedVersionId]);

  const retry = useCallback(() => setGeneration((value) => value + 1), []);
  const clearAttention = useCallback(async () => {
    await client.markArtifactAttentionRead(artifactId);
    setState((current) => current.status === "ready"
      ? { ...current, resource: { ...current.resource, attention: false } }
      : current);
  }, [artifactId, client]);

  return useMemo(() => ({ ...state, retry, clearAttention }), [clearAttention, retry, state]);
}
