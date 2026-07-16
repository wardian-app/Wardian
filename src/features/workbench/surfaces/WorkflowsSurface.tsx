import {
  WorkflowsView,
  type WorkflowsViewProps,
} from "../../../views/WorkflowsView";

export interface WorkflowsSurfaceProps extends WorkflowsViewProps {
  surface_id: string;
}

/** Typed workbench presentation wrapper; the builder store remains the resource owner. */
export function WorkflowsSurface({ surface_id, ...viewProps }: WorkflowsSurfaceProps) {
  return (
    <section
      className="h-full min-h-0 min-w-0"
      data-surface-id={surface_id}
      data-testid="workflows-surface"
    >
      <WorkflowsView {...viewProps} />
    </section>
  );
}
