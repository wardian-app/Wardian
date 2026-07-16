import { createContext, type ReactNode, useContext } from "react";
import type { RosterController } from "./useRosterController";

const RosterContext = createContext<RosterController | null>(null);

export interface RosterProviderProps {
  value: RosterController;
  children: ReactNode;
}

export function RosterProvider({ value, children }: RosterProviderProps) {
  return <RosterContext.Provider value={value}>{children}</RosterContext.Provider>;
}

export function useRosterContext(): RosterController {
  const controller = useContext(RosterContext);
  if (!controller) {
    throw new Error("useRosterContext must be used within a RosterProvider");
  }
  return controller;
}
