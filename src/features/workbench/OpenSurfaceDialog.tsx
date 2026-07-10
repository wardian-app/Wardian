import { useCallback, useLayoutEffect, useRef } from "react";

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
}: OpenSurfaceDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);

  useLayoutEffect(() => {
    if (open && !wasOpenRef.current) {
      returnFocusRef.current = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
      dialogRef.current?.focus();
    }
    wasOpenRef.current = open;
  }, [open]);

  const requestClose = useCallback(() => {
    on_close();
    returnFocusRef.current?.focus();
  }, [on_close]);

  if (!open) return null;

  const requestFor = (surfaceType: string): OpenSurfaceRequest => ({
    surface_type: surfaceType,
    group_id,
    ...(resource_key === undefined ? {} : { resource_key }),
  });

  const openChoice = (contribution: CoreSurfaceContribution): void => {
    if (choiceDisabled(contribution, registry, resource_key)) return;
    navigation.open(requestFor(contribution.surface_type));
    requestClose();
  };

  const openChoiceToSide = (contribution: CoreSurfaceContribution): void => {
    if (choiceDisabled(contribution, registry, resource_key)) return;
    navigation.open_to_side(requestFor(contribution.surface_type));
    requestClose();
  };

  return (
    <div
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-label="Open Surface"
      tabIndex={-1}
      className="wardian-open-surface-dialog"
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        requestClose();
      }}
    >
      <header>
        <div>
          <p>Workbench</p>
          <h2>Open Surface</h2>
        </div>
        <button type="button" aria-label="Close Open Surface" onClick={requestClose}>×</button>
      </header>
      {recently_closed[0] && (
        <section aria-labelledby="open-surface-recent-heading">
          <h3 id="open-surface-recent-heading">Recent</h3>
          <button
            type="button"
            onClick={() => {
              on_reopen_closed?.();
              requestClose();
            }}
          >
            Reopen {titleForType(recently_closed[0].surface.surface_type)}
          </button>
        </section>
      )}
      {CONTRIBUTION_GROUPS.map((group) => (
        <section key={group} aria-labelledby={`open-surface-${group.replace(" ", "-").toLowerCase()}`}>
          <h3 id={`open-surface-${group.replace(" ", "-").toLowerCase()}`}>{group}</h3>
          <div role="listbox" aria-label={group}>
            {CORE_SURFACE_CONTRIBUTIONS.filter((choice) => choice.group === group).map((choice) => {
              const disabled = choiceDisabled(choice, registry, resource_key);
              return (
                <div key={choice.surface_type} className="wardian-open-surface-choice">
                  <button
                    type="button"
                    role="option"
                    aria-label={choice.title}
                    aria-selected="false"
                    aria-disabled={disabled}
                    data-surface-type={choice.surface_type}
                    onClick={() => openChoice(choice)}
                  >
                    <span>{choice.title}</span>
                    <small>{choice.description}</small>
                  </button>
                  {!choice.reserved && (
                    <button
                      type="button"
                      aria-label={`Open ${choice.title} to Side`}
                      disabled={disabled}
                      onClick={() => openChoiceToSide(choice)}
                    >
                      Open to Side
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </section>
      ))}
    </div>
  );
}
