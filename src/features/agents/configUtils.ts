import { AgentConfig } from "../../types";

/**
 * Checks if the changes between two AgentConfigs require an agent restart.
 * Restarts are required if any field OTHER than session_name or session_id is changed.
 */
export function requiresRestart(oldConfig: AgentConfig, newConfig: AgentConfig): boolean {
  const keys = Object.keys(newConfig) as (keyof AgentConfig)[];
  
  for (const key of keys) {
    if (key === "session_name" || key === "session_id") continue;
    
    const oldVal = oldConfig[key];
    const newVal = newConfig[key];
    
    // Deep comparison for arrays/objects
    if (JSON.stringify(oldVal) !== JSON.stringify(newVal)) {
      return true;
    }
  }
  
  return false;
}
