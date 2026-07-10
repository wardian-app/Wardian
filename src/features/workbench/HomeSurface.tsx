import type { ClosedSurfaceV1 } from "../../types";

export type HomeSurfaceProps = {
  group_id: string;
  recently_closed?: readonly ClosedSurfaceV1[];
  on_open_surface: (groupId: string) => void;
  on_reopen_closed?: () => void;
};

function titleForType(surfaceType: string): string {
  return surfaceType
    .split("-")
    .map((part) => part.length === 0 ? part : `${part[0].toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

/** Derived discovery UI for an empty group; Home is never a persisted surface. */
export function HomeSurface({
  group_id,
  recently_closed = [],
  on_open_surface,
  on_reopen_closed,
}: HomeSurfaceProps) {
  return (
    <section className="wardian-workbench-home" data-group-id={group_id}>
      <div>
        <p>Wardian workbench</p>
        <h2>New Surface</h2>
        <p>Open a view here without leaving the task in front of you.</p>
        <button type="button" onClick={() => on_open_surface(group_id)}>
          Open Surface
        </button>
      </div>
      {recently_closed[0] && (
        <div aria-label="Recently closed surfaces">
          <h3>Recent</h3>
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
