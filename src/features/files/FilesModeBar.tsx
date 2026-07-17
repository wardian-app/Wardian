import { useMemo, useState } from "react";
import type { FileContentDescriptorV1, FilesSurfaceStateV1 } from "../../types";

type FilesModeBarProps = {
  resource_key: string;
  state: FilesSurfaceStateV1;
  descriptor: FileContentDescriptorV1 | null;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

function resourcePath(resourceKey: string) {
  return resourceKey.slice(resourceKey.indexOf(":") + 1).replace(/\\/g, "/");
}

function breadcrumbParts(path: string) {
  const drive = /^[A-Za-z]:\//.exec(path)?.[0].slice(0, 2);
  const rest = drive ? path.slice(3) : path.replace(/^\/+/, "");
  return [...(drive ? [drive] : path.startsWith("/") ? ["/"] : []), ...rest.split("/").filter(Boolean)];
}

export function FilesModeBar({
  resource_key,
  state,
  descriptor,
  on_open_with,
  on_reveal,
}: FilesModeBarProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const path = descriptor?.canonical_path ?? resourcePath(resource_key);
  const parts = useMemo(() => breadcrumbParts(path), [path]);
  const changesReasonId = `files-changes-unavailable-${resource_key.replace(/[^a-zA-Z0-9]/g, "-")}`;
  const draftReasonId = `files-draft-unavailable-${resource_key.replace(/[^a-zA-Z0-9]/g, "-")}`;

  return (
    <header className="files-mode-bar" data-restored-mode={state.mode}>
      <nav className="files-breadcrumb" aria-label="File location" title={path}>
        {parts.map((part, index) => (
          <span className="files-breadcrumb-part" key={`${part}-${index}`}>
            {index > 0 ? <span aria-hidden="true">/</span> : null}
            <span>{part}</span>
          </span>
        ))}
      </nav>
      <div className="files-mode-tabs" role="tablist" aria-label="File mode">
        <button type="button" role="tab" aria-selected="true">Preview</button>
        <button
          type="button"
          role="tab"
          aria-selected="false"
          aria-disabled="true"
          aria-describedby={changesReasonId}
          disabled
        >Changes</button>
        <button
          type="button"
          role="tab"
          aria-selected="false"
          aria-disabled="true"
          aria-describedby={draftReasonId}
          disabled
        >Draft</button>
        <span id={changesReasonId} className="files-visually-hidden">
          Changes is not available in this foundation.
        </span>
        <span id={draftReasonId} className="files-visually-hidden">
          Draft is not available in this foundation.
        </span>
      </div>
      <div className="files-overflow">
        <button
          type="button"
          className="files-overflow-trigger"
          aria-label="File actions"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={() => setMenuOpen((open) => !open)}
        >•••</button>
        {menuOpen ? (
          <div className="files-overflow-menu" role="menu" aria-label="File actions">
            {descriptor ? (
              <div className="files-overflow-metadata">
                <span>{descriptor.mime_type}</span>
                <span>{descriptor.size_bytes.toLocaleString()} bytes</span>
              </div>
            ) : null}
            <button type="button" role="menuitem" onClick={() => void on_open_with(path)}>
              Open With
            </button>
            <button type="button" role="menuitem" onClick={() => void on_reveal(path)}>
              Reveal
            </button>
          </div>
        ) : null}
      </div>
    </header>
  );
}
