import type { ArtifactResourceV1 } from "../../types";

export type ArtifactDetailsProps = {
  resource: ArtifactResourceV1;
  current_content_hash: string | null;
  on_select_version: (versionId: string) => void;
};

function statusLabel(status: ArtifactResourceV1["manifest"]["status"]): string {
  return status.replace(/_/g, " ");
}

/** Concise provenance for the artifact thread attached to the ordinary Files editor. */
export function ArtifactDetails({
  resource,
  current_content_hash,
  on_select_version,
}: ArtifactDetailsProps) {
  const changed = current_content_hash !== null
    && current_content_hash !== resource.selected_version.content_hash;
  return (
    <aside className="artifact-details" aria-label="Artifact details">
      <div className="artifact-details-primary">
        <strong>{resource.manifest.title}</strong>
        <span>from {resource.manifest.origin.agent_name}</span>
        <span>{new Date(resource.selected_version.presented_at_ms).toLocaleString()}</span>
        <span className="artifact-details-status">{statusLabel(resource.manifest.status)}</span>
        {changed ? <span className="artifact-details-changed">Changed since presented</span> : null}
      </div>
      <label className="artifact-version-select">
        <span>Version</span>
        <select
          value={resource.selected_version.version_id}
          onChange={(event) => on_select_version(event.target.value)}
        >
          {resource.manifest.versions.map((version) => (
            <option key={version.version_id} value={version.version_id}>
              {version.sequence} of {resource.manifest.versions.length}
            </option>
          ))}
        </select>
      </label>
    </aside>
  );
}
