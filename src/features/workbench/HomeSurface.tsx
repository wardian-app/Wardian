import type { ClosedSurfaceV1 } from "../../types";
import {
  CORE_SURFACE_CONTRIBUTIONS,
} from "./coreSurfaceRegistry";
import type { WorkbenchSurfaceRegistry } from "./surfaceRegistry";
import { surfaceIconForType } from "./surfaceIcons";

export type HomeSurfaceProps = {
  group_id: string;
  registry: WorkbenchSurfaceRegistry;
  recently_closed?: readonly ClosedSurfaceV1[];
  on_open_surface: (groupId: string) => void;
  on_select_surface: (surfaceType: string, groupId: string) => void;
  on_reopen_closed?: () => void;
};

type LauncherChoice = {
  surface_type: string;
  title: string;
  description: string;
};

function selectableContributions(
  registry: WorkbenchSurfaceRegistry,
): readonly LauncherChoice[] {
  const metadata = new Map(CORE_SURFACE_CONTRIBUTIONS.map((contribution) => (
    [contribution.surface_type, contribution] as const
  )));
  return registry.list()
    .filter((definition) => definition.resource_key === undefined)
    .map((definition) => {
      const contribution = metadata.get(definition.type);
      const title = contribution?.title ?? titleForType(definition.type);
      return {
        surface_type: definition.type,
        title,
        description: contribution?.description ?? `Open the ${title} surface.`,
      };
    });
}

function titleForType(surfaceType: string): string {
  return surfaceType
    .split("-")
    .map((part) => part.length === 0 ? part : `${part[0].toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

/** Derived discovery UI for an empty group; Home is never a persisted surface. */
export function HomeSurface({
  group_id,
  registry,
  recently_closed = [],
  on_open_surface,
  on_select_surface,
  on_reopen_closed,
}: HomeSurfaceProps) {
  const choices = selectableContributions(registry);

  return (
    <section className="wardian-workbench-home" data-group-id={group_id}>
      <div className="wardian-workbench-home__launcher">
        <header className="wardian-workbench-home__header">
          <p className="wardian-workbench-home__eyebrow">New tab</p>
          <h2>Choose a surface</h2>
          <p>Open another part of Wardian in this pane.</p>
        </header>

        <div
          className="wardian-workbench-home__grid"
          aria-label="Available surfaces"
          role="group"
        >
          {choices.map((choice) => {
            const Icon = surfaceIconForType(choice.surface_type);
            return (
              <button
                aria-label={`${choice.title}: ${choice.description}`}
                className="wardian-workbench-home__surface"
                data-surface-type={choice.surface_type}
                key={choice.surface_type}
                type="button"
                onClick={() => on_select_surface(choice.surface_type, group_id)}
              >
                <span className="wardian-workbench-home__icon" aria-hidden="true">
                  <Icon size={21} strokeWidth={1.7} />
                </span>
                <span className="wardian-workbench-home__copy">
                  <strong>{choice.title}</strong>
                  <small>{choice.description}</small>
                </span>
              </button>
            );
          })}
        </div>

        <button
          className="wardian-workbench-home__browse"
          type="button"
          onClick={() => on_open_surface(group_id)}
        >
          Browse all surfaces
        </button>
      </div>
      {recently_closed[0] && (
        <div className="wardian-workbench-home__recent" aria-label="Recently closed surfaces">
          <div>
            <h3>Recently closed</h3>
            <p>Pick up where you left off.</p>
          </div>
          <button
            type="button"
            onClick={on_reopen_closed}
          >
            Reopen {titleForType(recently_closed[0].surface.surface_type)}
          </button>
        </div>
      )}
    </section>
  );
}
