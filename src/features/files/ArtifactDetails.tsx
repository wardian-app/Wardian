import type { ArtifactResourceV1 } from "../../types";

export type ArtifactDetailsProps = {
  resource: ArtifactResourceV1;
  current_content_hash: string | null;
  on_select_version: (versionId: string) => void;
};

/** Concise provenance for the artifact thread attached to the ordinary Files editor. */
export function ArtifactDetails({
  resource,
  current_content_hash,
  on_select_version,
}: ArtifactDetailsProps) {
  const changed = current_content_hash !== null
    && current_content_hash !== resource.selected_version.content_hash;
  const versionCount = resource.manifest.versions.length;
  const presentedAt = new Date(resource.selected_version.presented_at_ms).toLocaleString();
  return (
    <aside
      className="artifact-details"
      aria-label="Artifact details"
      title={`${resource.manifest.title} presented ${presentedAt}`}
    >
      <div className="artifact-details-primary">
        <span className="artifact-details-origin">
          Presented by <strong>{resource.manifest.origin.agent_name}</strong>
        </span>
        {changed ? <span className="artifact-details-changed">Changed since presented</span> : null}
      </div>
      {versionCount > 1 ? (
        <label className="artifact-version-select" title="Artifact version">
          <span className="files-visually-hidden">Artifact version</span>
          <select
            aria-label="Artifact version"
            value={resource.selected_version.version_id}
            onChange={(event) => on_select_version(event.target.value)}
          >
            {resource.manifest.versions.map((version) => (
              <option key={version.version_id} value={version.version_id}>
                {version.sequence} / {versionCount}
              </option>
            ))}
          </select>
        </label>
      ) : null}
    </aside>
  );
}
