import { createContext, useContext } from "react";
import type { PropsWithChildren } from "react";
import {
  useAgentResourceController,
  type AgentResourceController,
  type AgentResourceControllerOptions,
} from "./useAgentResourceController";

export const AgentResourceContext = createContext<AgentResourceController | null>(null);

export type AgentResourceProviderProps = PropsWithChildren<AgentResourceControllerOptions>;

/**
 * Instantiates the authoritative controller once at the application boundary.
 * Descendants consume the same resource snapshot and lifecycle operations.
 */
export function AgentResourceProvider({
  children,
  ...options
}: AgentResourceProviderProps) {
  const controller = useAgentResourceController(options);
  return (
    <AgentResourceContext.Provider value={controller}>
      {children}
    </AgentResourceContext.Provider>
  );
}

export function useAgentResources(): AgentResourceController {
  const controller = useContext(AgentResourceContext);
  if (!controller) {
    throw new Error("useAgentResources must be used within AgentResourceProvider");
  }
  return controller;
}
