import { type PropsWithChildren, type ReactNode, useEffect, useState } from "react";

import {
  DashboardView,
  type DashboardViewProps,
} from "../../../views/DashboardView";
import { GardenView, type GardenViewProps } from "../../../views/GardenView";
import { GraphView, type GraphViewProps } from "../../../views/GraphView";
import { InboxView, type InboxViewProps } from "../../../views/InboxView";
import {
  CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION,
  HEAVY_SURFACE_HIDDEN_GRACE_MS,
  type EmptyCoreViewSurfaceState,
  type CoreViewSurfaceType,
  type GardenSurfaceState,
  type GraphSurfaceState,
  type SurfaceVisibility,
} from "./coreSurfaceMetadata";

export * from "./coreSurfaceMetadata";

type SurfaceFrameProps = PropsWithChildren<{
  surface_id: string;
  surface_type: CoreViewSurfaceType;
  visibility?: SurfaceVisibility;
}>;

function SurfaceFrame({
  surface_id,
  surface_type,
  visibility = "visible",
  children,
}: SurfaceFrameProps) {
  return (
    <section
      aria-hidden={visibility === "hidden"}
      className="flex h-full min-h-0 min-w-0 flex-col bg-[var(--color-wardian-bg)]"
      data-surface-id={surface_id}
      data-surface-state-schema-version={CORE_VIEW_SURFACE_STATE_SCHEMA_VERSION}
      data-surface-type={surface_type}
      data-testid={`${surface_type}-surface`}
    >
      {children}
    </section>
  );
}

export type SuspendedSurfaceRendererProps = {
  visibility: SurfaceVisibility;
  hidden_grace_ms?: number;
  children: ReactNode | ((rendererMounted: boolean) => ReactNode);
};

/**
 * Retains the logical surface host while releasing a heavy renderer after a
 * bounded hidden grace period. The Dockview panel must remain mounted and
 * feed its canonical visibility into this component for the grace to apply.
 */
export function SuspendedSurfaceRenderer({
  visibility,
  hidden_grace_ms = HEAVY_SURFACE_HIDDEN_GRACE_MS,
  children,
}: SuspendedSurfaceRendererProps) {
  const [rendererMounted, setRendererMounted] = useState(true);

  useEffect(() => {
    if (visibility === "visible") {
      setRendererMounted(true);
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setRendererMounted(false);
    }, hidden_grace_ms);
    return () => window.clearTimeout(timeoutId);
  }, [hidden_grace_ms, visibility]);

  return (
    <div
      className="flex h-full min-h-0 min-w-0 flex-col"
      data-heavy-renderer-state={rendererMounted ? "mounted" : "released"}
    >
      {typeof children === "function"
        ? children(rendererMounted)
        : rendererMounted ? children : null}
    </div>
  );
}

type ManagedSurfaceProps<TState> = {
  surface_id: string;
  state: TState;
  visibility?: SurfaceVisibility;
};

export interface DashboardSurfaceProps
  extends DashboardViewProps, ManagedSurfaceProps<EmptyCoreViewSurfaceState> {}

export function DashboardSurface({
  surface_id,
  state: _state,
  visibility = "visible",
  ...viewProps
}: DashboardSurfaceProps) {
  return (
    <SurfaceFrame surface_id={surface_id} surface_type="dashboard" visibility={visibility}>
      <DashboardView {...viewProps} />
    </SurfaceFrame>
  );
}

export interface InboxSurfaceProps
  extends InboxViewProps, ManagedSurfaceProps<EmptyCoreViewSurfaceState> {}

export function InboxSurface({
  surface_id,
  state: _state,
  visibility = "visible",
  ...viewProps
}: InboxSurfaceProps) {
  return (
    <SurfaceFrame surface_id={surface_id} surface_type="inbox" visibility={visibility}>
      <InboxView {...viewProps} />
    </SurfaceFrame>
  );
}

export interface GraphSurfaceProps
  extends Omit<GraphViewProps,
    "onOpenAgentInGrid" | "visibility" | "rendererActive" | "initialSurfaceState" | "onSurfaceStateChange">,
    ManagedSurfaceProps<GraphSurfaceState> {
  onOpenAgent: (agentId: string) => void;
  on_state_change: (state: GraphSurfaceState) => void;
}

export function GraphSurface({
  surface_id,
  state: _state,
  on_state_change,
  visibility = "visible",
  ...viewProps
}: GraphSurfaceProps) {
  return (
    <SurfaceFrame surface_id={surface_id} surface_type="graph" visibility={visibility}>
      <SuspendedSurfaceRenderer visibility={visibility}>
        {(rendererMounted) => (
          <GraphView
            {...viewProps}
            visibility={visibility}
            rendererActive={rendererMounted}
            initialSurfaceState={_state}
            onSurfaceStateChange={(state) => {
              if (JSON.stringify(state) !== JSON.stringify(_state)) on_state_change(state);
            }}
          />
        )}
      </SuspendedSurfaceRenderer>
    </SurfaceFrame>
  );
}

export interface GardenSurfaceProps
  extends Omit<GardenViewProps,
    "onOpenAgentInGrid" | "visibility" | "rendererActive" | "initialSurfaceState" | "onSurfaceStateChange">,
    ManagedSurfaceProps<GardenSurfaceState> {
  onOpenAgent: (agentId: string) => void;
  on_state_change: (state: GardenSurfaceState) => void;
}

export function GardenSurface({
  surface_id,
  state: _state,
  on_state_change,
  visibility = "visible",
  ...viewProps
}: GardenSurfaceProps) {
  return (
    <SurfaceFrame surface_id={surface_id} surface_type="garden" visibility={visibility}>
      <SuspendedSurfaceRenderer visibility={visibility}>
        {(rendererMounted) => (
          <GardenView
            {...viewProps}
            visibility={visibility}
            rendererActive={rendererMounted}
            initialSurfaceState={_state}
            onSurfaceStateChange={(state) => {
              if (JSON.stringify(state) !== JSON.stringify(_state)) on_state_change(state);
            }}
          />
        )}
      </SuspendedSurfaceRenderer>
    </SurfaceFrame>
  );
}
