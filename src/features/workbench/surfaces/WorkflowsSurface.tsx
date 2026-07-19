import {
  WorkflowsView,
  type WorkflowsViewProps,
} from "../../../views/WorkflowsView";
import type { SurfaceVisibility } from "./coreSurfaceMetadata";

export interface WorkflowsSurfaceProps extends WorkflowsViewProps {
  surface_id: string;
  visibility?: SurfaceVisibility;
}

/** Typed workbench presentation wrapper; the builder store remains the resource owner. */
export function WorkflowsSurface({ surface_id, visibility = "visible", ...viewProps }: WorkflowsSurfaceProps) {
  const hidden = visibility === "hidden";

  return (
    <section
      aria-hidden={hidden}
      className="h-full min-h-0 min-w-0"
      data-surface-id={surface_id}
      data-surface-visibility={visibility}
      data-testid="workflows-surface"
      style={hidden ? { display: "none" } : undefined}
    >
      <WorkflowsView {...viewProps} />
    </section>
  );
}
