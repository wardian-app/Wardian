import { createE2eConfig } from "./playwright.config";

/** Workbench-focused runs use a fresh isolated server for deterministic state. */
export default createE2eConfig({ isolated_server: true });
