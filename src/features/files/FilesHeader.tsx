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
import { BookOpen, FileDiff, Pencil } from "lucide-react";

import { CompactOverflowButton } from "../../components/CompactOverflowButton";
import type { FileContentDescriptorV1 } from "../../types";
import { formatExplorerPathForDisplay } from "../../utils/displayPath";
import { decodeFileResourceKey } from "./fileResourceKey";
import type { FileContentPresentation } from "./rendererRegistry";
import type { FileDiffSummary } from "./fileDiffModel";

export type FilesHeaderProps = {
  resource_key: string;
  descriptor: FileContentDescriptorV1 | null;
  presentation: FileContentPresentation;
  presentation_toggle_available: boolean;
  dirty: boolean;
  save_available: boolean;
  save_disabled?: boolean;
  save_as_available: boolean;
  saving: boolean;
  changes?: FileDiffSummary | null;
  comparison_open?: boolean;
  resource_actions_available?: boolean;
  on_presentation_change: (presentation: FileContentPresentation) => void;
  on_comparison_toggle?: () => void;
  on_save: () => Promise<void> | void;
  on_save_as: () => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

type MenuAction = {
  label: string;
  disabled?: boolean;
  run: () => Promise<void> | void;
};

/** Compact conventional file context and action header. */
export function FilesHeader({
  resource_key,
  descriptor,
  presentation,
  presentation_toggle_available,
  dirty,
  save_available,
  save_disabled = false,
  save_as_available,
  saving,
  changes = null,
  comparison_open = false,
  resource_actions_available = true,
  on_presentation_change,
  on_comparison_toggle = () => undefined,
  on_save,
  on_save_as,
  on_open_with,
  on_reveal,
}: FilesHeaderProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuId = `${useId()}-actions`;
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
  const closeMenu = useCallback((restoreTrigger = false) => {
    setMenuOpen(false);
    if (restoreTrigger) triggerRef.current?.focus();
  }, []);
  const menuActions = useMemo(() => {
    const actions: MenuAction[] = [];
    if (save_available) {
      actions.push({ label: "Save", disabled: saving || save_disabled, run: on_save });
    }
    if (save_as_available) actions.push({ label: "Save As", disabled: saving, run: on_save_as });
    actions.push(
      { label: "Open With", run: () => on_open_with(actionPath) },
      { label: "Reveal", run: () => on_reveal(actionPath) },
    );
    return actions;
  }, [
    actionPath,
    on_open_with,
    on_reveal,
    on_save,
    on_save_as,
    save_as_available,
    save_available,
    save_disabled,
    saving,
  ]);

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
    const items = itemRefs.current.filter((item): item is HTMLButtonElement => (
      item !== null && !item.disabled
    ));
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
  const runAction = (action: MenuAction) => {
    closeMenu(true);
    try {
      void Promise.resolve(action.run()).catch(() => undefined);
    } catch {
      // The owning Files surface reports resource-local action failures.
    }
  };
  const editorActive = presentation === "editor";
  const presentationAction = editorActive ? "View rendered" : "Edit source";
  const PresentationIcon = editorActive ? Pencil : BookOpen;
  const changesLabel = changes
    ? `${comparison_open ? "Close" : "Open"} comparison: ${changes.regions} change ${
      changes.regions === 1 ? "region" : "regions"
    }, ${changes.added_lines} added, ${changes.modified_lines} modified, ${
      changes.deleted_lines
    } deleted against Saved file`
    : "";
  const visibleChangeCount = changes && changes.regions > 99 ? "99+" : changes?.regions;

  return (
    <header className="files-header">
      <nav className="files-breadcrumb" aria-label="File location" title={displayPath}>
        <span className="files-breadcrumb-path">{displayPath}</span>
        {dirty ? (
          <span
            className="files-breadcrumb-dirty"
            aria-label="Unsaved changes"
            title="Unsaved changes"
          />
        ) : null}
      </nav>
      <div className="files-header-actions">
        {presentation_toggle_available ? (
          <button
            type="button"
            className="files-presentation-toggle"
            aria-label={presentationAction}
            aria-pressed={editorActive}
            title={presentationAction}
            onClick={() => on_presentation_change(editorActive ? "rendered" : "editor")}
          >
            <PresentationIcon size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
        ) : null}
        {changes && changes.regions > 0 ? (
          <button
            type="button"
            className="files-diff-toggle"
            aria-label={changesLabel}
            aria-pressed={comparison_open}
            title={changesLabel}
            onClick={on_comparison_toggle}
          >
            <FileDiff size={15} strokeWidth={1.75} aria-hidden="true" />
            <span className="files-diff-count" aria-hidden="true">{visibleChangeCount}</span>
          </button>
        ) : null}
        {resource_actions_available ? (
          <div ref={overflowRef} className="files-overflow" onBlur={onOverflowBlur}>
            <CompactOverflowButton
              ref={triggerRef}
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
                initialMenuItemRef.current = event.key === "ArrowUp"
                  ? Math.max(0, menuActions.length - 1)
                  : 0;
                setMenuOpen(true);
              }}
            />
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
                {menuActions.map((action, index) => (
                  <button
                    key={action.label}
                    ref={(node) => { itemRefs.current[index] = node; }}
                    type="button"
                    role="menuitem"
                    tabIndex={-1}
                    disabled={action.disabled}
                    onClick={() => runAction(action)}
                  >
                    {action.label}
                  </button>
                ))}
              </div>
            ) : null}
          </div>
        ) : null}
      </div>
    </header>
  );
}
