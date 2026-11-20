import { useAgentTelemetryStore } from "../../agents/useAgentTelemetryStore";
import { deriveCurrentThought } from "../../../utils/statusUtils";
import type { WorkbenchNavigationService } from "../navigationService";
import { AgentSessionSurface, type AgentSessionSurfaceProps } from "./AgentSessionSurface";

/** Connects an App-owned session surface to its canonical status and navigation. */
export function WorkbenchAgentSessionSurface({
  is_off,
  navigation,
  ...props
}: Omit<AgentSessionSurfaceProps, "status" | "telemetry" | "on_rebind_agent" | "on_close_surface"> & {
  is_off: boolean;
  navigation: WorkbenchNavigationService;
}) {
  // Subscribe per presentation so live telemetry does not re-render all of App.
  const metrics = useAgentTelemetryStore((state) => state.telemetry[props.resource_key]);
  const title = useAgentTelemetryStore((state) => state.terminal_titles[props.resource_key] ?? "");
  const thought = useAgentTelemetryStore((state) => state.current_thoughts[props.resource_key] ?? "");
  const { status } = deriveCurrentThought(title, thought, metrics, is_off);
  return (
    <AgentSessionSurface
      {...props}
      status={status}
      telemetry={metrics}
      on_rebind_agent={(nextAgentId) => {
        void navigation.rebind_resource(props.surface_id, {
          surface_type: "agent-session", resource_key: nextAgentId,
        });
      }}
      on_close_surface={() => { void navigation.close(props.surface_id); }}
    />
  );
}
