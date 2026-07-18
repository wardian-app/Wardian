import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent,
} from "react";
import { BookOpen, Pencil } from "lucide-react";
import type { FileContentDescriptorV1, FilesSurfaceState } from "../../types";
import { formatExplorerPathForDisplay } from "../../utils/displayPath";
import { decodeFileResourceKey, isWindowsAbsoluteFilePath } from "./fileResourceKey";
import { legacyFilesMode } from "./filesSurfaceState";
import type { FilePreviewPresentation } from "./rendererRegistry";

type FilesModeBarProps = {
  resource_key: string;
  state: FilesSurfaceState;
  descriptor: FileContentDescriptorV1 | null;
  preview_presentation: FilePreviewPresentation;
  source_available: boolean;
  resource_actions_available?: boolean;
  on_preview_presentation_change: (presentation: FilePreviewPresentation) => void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

type BreadcrumbPart = { separator: string; label: string };

function breadcrumbParts(path: string): BreadcrumbPart[] {
  const windowsPath = isWindowsAbsoluteFilePath(path);
  const separator = windowsPath && path.includes("\\") ? "\\" : "/";
  const drive = windowsPath ? /^[A-Za-z]:[\\/]/.exec(path)?.[0].slice(0, 2) : undefined;
  const unc = windowsPath && (path.startsWith("\\\\") || path.startsWith("//"));
  const rooted = !unc && path.startsWith("/");
  const rest = drive ? path.slice(3) : unc ? path.slice(2) : rooted ? path.slice(1) : path;
  const segments = rest.split(windowsPath ? /[\\/]+/ : /\/+/).filter(Boolean);

  if (drive) {
    return [
      { separator: "", label: drive },
      ...segments.map((label) => ({ separator, label })),
    ];
  }
  if (unc) {
    return [
      { separator: "", label: separator.repeat(2) },
      ...segments.map((label, index) => ({ separator: index === 0 ? "" : separator, label })),
    ];
  }
  if (rooted) {
    return [
      { separator: "", label: "/" },
      ...segments.map((label, index) => ({ separator: index === 0 ? "" : "/", label })),
    ];
  }
  return segments.map((label, index) => ({ separator: index === 0 ? "" : separator, label }));
}

export function FilesModeBar({
  resource_key,
  state,
  descriptor,
  preview_presentation,
  source_available,
  resource_actions_available = true,
  on_preview_presentation_change,
  on_open_with,
  on_reveal,
}: FilesModeBarProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const unavailableReasonId = useId();
  const menuId = `${unavailableReasonId}-actions`;
  const overflowRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const initialMenuItemRef = useRef(0);
  const decodedResource = useMemo(() => decodeFileResourceKey(resource_key), [resource_key]);
  const opaqueFallback = descriptor === null && decodedResource.resource_kind === "artifact";
  const actionPath = descriptor?.canonical_path ?? (decodedResource.resource_kind === "file"
    ? decodedResource.path
    : decodedResource.artifact_id);
  const displayPath = opaqueFallback ? actionPath : formatExplorerPathForDisplay(actionPath);
  const parts = useMemo(() => opaqueFallback
    ? [{ separator: "", label: displayPath }]
    : breadcrumbParts(displayPath), [displayPath, opaqueFallback]);
  const changesReasonId = `${unavailableReasonId}-changes`;
  const draftReasonId = `${unavailableReasonId}-draft`;
  const changesReason = "Changes is not available in this foundation.";
  const draftReason = "Draft is not available in this foundation.";
  const sourceActive = preview_presentation === "source";
  const sourceLabel = sourceActive ? "View rendered" : "View source";
  const SourceIcon = sourceActive ? Pencil : BookOpen;
  const closeMenu = useCallback((restoreTrigger = false) => {
    setMenuOpen(false);
    if (restoreTrigger) triggerRef.current?.focus();
  }, []);
  useEffect(() => {
    if (!menuOpen) return;
    itemRefs.current[initialMenuItemRef.current]?.focus();
    initialMenuItemRef.current = 0;
    const dismissOutside = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Node && !overflowRef.current?.contains(target)) closeMenu();
    };
    document.addEventListener("pointerdown", dismissOutside, true);
    return () => document.removeEventListener("pointerdown", dismissOutside, true);
  }, [closeMenu, menuOpen]);
  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const items = itemRefs.current.filter((item): item is HTMLButtonElement => item !== null);
    if (!items.length) return;
    const currentIndex = Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement));
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % items.length;
    else if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + items.length) % items.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = items.length - 1;
    else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeMenu(true);
      return;
    }
    if (nextIndex === null) return;
    event.preventDefault();
    items[nextIndex]?.focus();
  };
  const onOverflowBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget;
    if (next instanceof Node && event.currentTarget.contains(next)) return;
    closeMenu();
  };
  const revealModeTab = (event: FocusEvent<HTMLButtonElement>) => {
    event.currentTarget.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  };
  const runAction = (action: (path: string) => Promise<void> | void) => {
    closeMenu(true);
    try {
      void Promise.resolve(action(actionPath)).catch(() => undefined);
    } catch {
      // The owning Files surface reports action failures when applicable.
    }
  };

  const restoredMode = "mode" in state ? state.mode : legacyFilesMode(state);

  return (
    <header className="files-mode-bar" data-restored-mode={restoredMode}>
      <nav className="files-breadcrumb" aria-label="File location" title={displayPath}>
        {parts.map((part, index) => (
          <span className="files-breadcrumb-part" key={`${part.separator}${part.label}-${index}`}>
            {part.separator}{part.label}
          </span>
        ))}
      </nav>
      <div className="files-mode-tabs" role="tablist" aria-label="File mode">
        <button type="button" role="tab" aria-selected="true" onFocus={revealModeTab}>Preview</button>
        <button
          type="button"
          role="tab"
          aria-selected="false"
          aria-disabled="true"
          aria-describedby={changesReasonId}
          title={changesReason}
          onFocus={revealModeTab}
        >Changes</button>
        <button
          type="button"
          role="tab"
          aria-selected="false"
          aria-disabled="true"
          aria-describedby={draftReasonId}
          title={draftReason}
          onFocus={revealModeTab}
        >Draft</button>
        <span id={changesReasonId} className="files-visually-hidden">
          {changesReason}
        </span>
        <span id={draftReasonId} className="files-visually-hidden">
          {draftReason}
        </span>
      </div>
      <div className="files-header-actions">
        {source_available ? (
          <button
            type="button"
            className="files-presentation-toggle"
            aria-label={sourceLabel}
            aria-pressed={sourceActive}
            title={sourceLabel}
            onClick={() => on_preview_presentation_change(sourceActive ? "rendered" : "source")}
          >
            <SourceIcon size={15} aria-hidden="true" />
          </button>
        ) : null}
        {resource_actions_available ? (
        <div ref={overflowRef} className="files-overflow" onBlur={onOverflowBlur}>
        <button
          ref={triggerRef}
          type="button"
          className="files-overflow-trigger"
          aria-label="File actions"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-controls={menuOpen ? menuId : undefined}
          onClick={() => {
            initialMenuItemRef.current = 0;
            setMenuOpen((open) => !open);
          }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            event.preventDefault();
            initialMenuItemRef.current = event.key === "ArrowUp" ? 1 : 0;
            setMenuOpen(true);
          }}
        >•••</button>
        {menuOpen ? (
          <div
            id={menuId}
            className="files-overflow-menu"
            role="menu"
            aria-label="File actions"
            onKeyDown={onMenuKeyDown}
          >
            {descriptor ? (
              <div className="files-overflow-metadata">
                <span>{descriptor.mime_type}</span>
                <span>{descriptor.size_bytes.toLocaleString()} bytes</span>
              </div>
            ) : null}
            <button
              ref={(node) => { itemRefs.current[0] = node; }}
              type="button"
              role="menuitem"
              tabIndex={-1}
              onClick={() => runAction(on_open_with)}
            >
              Open With
            </button>
            <button
              ref={(node) => { itemRefs.current[1] = node; }}
              type="button"
              role="menuitem"
              tabIndex={-1}
              onClick={() => runAction(on_reveal)}
            >
              Reveal
            </button>
          </div>
        ) : null}
        </div>
        ) : null}
      </div>
    </header>
  );
}
