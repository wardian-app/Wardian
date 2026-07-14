import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { ClosedSurfaceV1, OpenSurfaceRequest } from "../../types";
import type { WorkbenchNavigationService } from "./navigationService";
import type { WorkbenchSurfaceRegistry } from "./surfaceRegistry";
import {
  CORE_SURFACE_CONTRIBUTIONS,
  type CoreSurfaceContribution,
  type CoreSurfaceGroup,
} from "./coreSurfaceRegistry";

export { createCoreWorkbenchSurfaceRegistry } from "./coreSurfaceRegistry";

export type OpenSurfaceDialogProps = {
  open: boolean;
  group_id: string;
  resource_key?: string;
  navigation: WorkbenchNavigationService;
  registry: WorkbenchSurfaceRegistry;
  recently_closed?: readonly ClosedSurfaceV1[];
  on_reopen_closed?: () => void;
  on_close: () => void;
  return_focus?: HTMLElement | null;
  placeholder_surface_id?: string;
};

const CONTRIBUTION_GROUPS: readonly CoreSurfaceGroup[] = ["Core views", "Sessions", "Reserved"];

function choiceDisabled(
  contribution: CoreSurfaceContribution,
  registry: WorkbenchSurfaceRegistry,
  resourceKey: string | undefined,
): boolean {
  return contribution.reserved === true
    || registry.get(contribution.surface_type) === undefined
    || (contribution.requires_resource === true && !resourceKey?.trim());
}

function titleForType(surfaceType: string): string {
  return CORE_SURFACE_CONTRIBUTIONS.find((choice) => choice.surface_type === surfaceType)?.title
    ?? surfaceType;
}

export function OpenSurfaceDialog({
  open,
  group_id,
  resource_key,
  navigation,
  registry,
  recently_closed = [],
  on_reopen_closed,
  on_close,
  return_focus,
  placeholder_surface_id,
}: OpenSurfaceDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const primaryModifier = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform)
    ? "⌘"
    : "Ctrl";

  const availableChoices = useMemo(() => CONTRIBUTION_GROUPS.flatMap((group) => (
    CORE_SURFACE_CONTRIBUTIONS.filter(
      (choice) => choice.group === group && !choiceDisabled(choice, registry, resource_key),
    )
  )), [registry, resource_key]);
  const filteredChoices = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return availableChoices;
    return availableChoices.filter((choice) => (
      `${choice.title} ${choice.description} ${choice.group}`.toLocaleLowerCase().includes(normalized)
    ));
  }, [availableChoices, query]);
  const showRecent = Boolean(recently_closed[0] && !query.trim());
  const optionCount = filteredChoices.length + (showRecent ? 1 : 0);

  useLayoutEffect(() => {
    if (open && !wasOpenRef.current) {
      returnFocusRef.current = return_focus ?? (
        document.activeElement instanceof HTMLElement ? document.activeElement : null
      );
      setQuery("");
      setActiveIndex(0);
      window.requestAnimationFrame(() => inputRef.current?.focus());
    }
    wasOpenRef.current = open;
  }, [open, return_focus]);

  const requestClose = useCallback(() => {
    on_close();
    returnFocusRef.current?.focus();
  }, [on_close]);

  if (!open) return null;

  const focusableElements = (): HTMLElement[] => Array.from(
    dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ) ?? [],
  );

  const requestFor = (surfaceType: string): OpenSurfaceRequest => ({
    surface_type: surfaceType,
    group_id,
    ...(resource_key === undefined ? {} : { resource_key }),
  });

  const openChoice = (contribution: CoreSurfaceContribution): void => {
    if (choiceDisabled(contribution, registry, resource_key)) return;
    const request = requestFor(contribution.surface_type);
    if (placeholder_surface_id) {
      navigation.open_from_placeholder(placeholder_surface_id, request);
    } else {
      navigation.open(request);
    }
    requestClose();
  };

  const openChoiceToSide = (contribution: CoreSurfaceContribution): void => {
    if (choiceDisabled(contribution, registry, resource_key)) return;
    navigation.open_to_side(requestFor(contribution.surface_type));
    requestClose();
  };

  const activateIndex = (index: number, toSide: boolean): void => {
    if (showRecent) {
      if (index === 0) {
        on_reopen_closed?.();
        requestClose();
        return;
      }
      index -= 1;
    }
    const choice = filteredChoices[index];
    if (!choice) return;
    if (toSide) openChoiceToSide(choice);
    else openChoice(choice);
  };

  return (
    <div
      className="wardian-palette-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label="Open Surface"
        className="wardian-open-surface-dialog"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            requestClose();
            return;
          }
          if (event.key === "Tab") {
            const focusable = focusableElements();
            const first = focusable[0];
            const last = focusable[focusable.length - 1];
            if (!first || !last) return;
            if (event.shiftKey && document.activeElement === first) {
              event.preventDefault();
              last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first.focus();
            }
            return;
          }
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (optionCount === 0) return;
            setActiveIndex((current) => (
              event.key === "ArrowDown"
                ? (current + 1) % optionCount
                : (current - 1 + optionCount) % optionCount
            ));
            return;
          }
          if (event.key === "Home" || event.key === "End") {
            event.preventDefault();
            setActiveIndex(event.key === "Home" ? 0 : Math.max(0, optionCount - 1));
            return;
          }
          if (event.key === "Enter") {
            event.preventDefault();
            activateIndex(activeIndex, event.ctrlKey || event.metaKey);
          }
        }}
      >
        <input
          ref={inputRef}
          type="search"
          role="combobox"
          aria-label="Open a surface"
          aria-controls="workbench-open-surface-options"
          aria-expanded="true"
          aria-activedescendant={optionCount > 0 ? `workbench-open-option-${activeIndex}` : undefined}
          placeholder="Open a surface…"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
          }}
        />
        <div id="workbench-open-surface-options" role="listbox" aria-label="Available surfaces">
          {showRecent && recently_closed[0] && (
            <button
              id="workbench-open-option-0"
              type="button"
              role="option"
              aria-label={`Reopen ${titleForType(recently_closed[0].surface.surface_type)}`}
              aria-selected={activeIndex === 0}
              className="wardian-palette-option"
              onMouseEnter={() => setActiveIndex(0)}
              onClick={() => activateIndex(0, false)}
            >
              <span>Reopen {titleForType(recently_closed[0].surface.surface_type)}</span>
              <small>Recently closed</small>
            </button>
          )}
          {filteredChoices.map((choice, index) => {
            const optionIndex = index + (showRecent ? 1 : 0);
            return (
              <button
                id={`workbench-open-option-${optionIndex}`}
                key={choice.surface_type}
                type="button"
                role="option"
                aria-label={choice.title}
                aria-selected={activeIndex === optionIndex}
                data-surface-type={choice.surface_type}
                className="wardian-palette-option"
                onMouseEnter={() => setActiveIndex(optionIndex)}
                onClick={(event) => activateIndex(
                  optionIndex,
                  event.ctrlKey || event.metaKey,
                )}
              >
                <span>{choice.title}</span>
                <small>{choice.description}</small>
              </button>
            );
          })}
          {optionCount === 0 && <p className="wardian-palette-empty">No matching surfaces</p>}
        </div>
        <footer>
          <span><kbd>Enter</kbd> Open</span>
          <span><kbd>{primaryModifier}</kbd>+<kbd>Enter</kbd> Open to side</span>
          <span><kbd>Esc</kbd> Close</span>
        </footer>
      </div>
    </div>
  );
}
